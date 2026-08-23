//! The reading worker. It never mutates anything, so it does not
//! queue behind the mutations and the "one worker" rule, which is about the
//! order of writes, is untouched.
//!
//! Its discipline is the opposite of the job queue's. Mutations are FIFO and
//! every one is delivered; a read is *last wins*: typing another character
//! replaces the search that was running, and the results it had left are worth
//! nothing. A generation counter says which search is the live one — it both
//! stops the walk and drops whatever it had already sent.

use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::fs::{
    directory::DirEntry,
    find::{self, Ended, Limits},
    worker::Health,
};

/// Which search a message belongs to. Unlike a `JobTag`, which finds *its own*
/// job among several running ones, this exists to throw *older* ones away:
/// only the newest generation is live, and everything else is stale by
/// definition.
pub type Generation = u64;

/// A search on its way to the reader thread. Like a `Job`, it holds a snapshot
/// of what it needs and never reads back.
#[derive(Debug)]
struct Search {
    generation: Generation,
    root: Arc<Path>,
    query: String,
}

/// What the reader thread sends back. Never leaves this module: `drain` folds
/// it into [`Found`] and drops what is stale on the way.
#[derive(Debug)]
enum ReaderMsg {
    Hits {
        generation: Generation,
        hits: Vec<DirEntry>,
    },
    Done {
        generation: Generation,
        ended: Ended,
    },
}

/// Everything the reader sent since the last drain that still belongs to the
/// newest search.
#[derive(Debug, Default)]
pub struct Found {
    /// Hits to append, in the order the walk reported them. A drain that
    /// caught nothing new leaves this empty.
    pub hits: Vec<DirEntry>,
    /// Set once, on the drain that catches the end of the search.
    pub ended: Option<Ended>,
    pub health: Health,
}

/// The main thread's end of the reader: searches going out, hits coming back,
/// and one counter saying which search those hits are still wanted for.
#[derive(Debug)]
pub struct Reader {
    searches: Sender<Search>,
    msgs: Receiver<ReaderMsg>,
    /// The generation the walk keeps comparing itself against. Raising it is
    /// the only way to stop a walk, and does so whether the search has started
    /// or is still in the queue.
    latest: Arc<AtomicU64>,
    /// The main thread's copy of `latest`; the two never disagree, since only
    /// the main thread writes.
    generation: Generation,
    health: Health,
}

impl Reader {
    /// Starts the thread. It lives as long as this handle: dropping the handle
    /// closes the search channel, which ends the loop.
    ///
    /// `limits` are fixed for the life of the reader rather than passed per
    /// search — they describe the searcher, not one question asked of it.
    pub fn start(limits: Limits) -> Self {
        let (search_tx, search_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        // Generation zero is the one no search is ever queued under, so before
        // the first search every message that could arrive is already stale.
        let latest = Arc::new(AtomicU64::new(0));

        let reader_latest = Arc::clone(&latest);
        thread::spawn(move || run(&search_rx, &msg_tx, &reader_latest, &limits));

        Self {
            searches: search_tx,
            msgs: msg_rx,
            latest,
            generation: 0,
            health: Health::Running,
        }
    }

    /// Replaces whatever was running with a search for `query` under `root`,
    /// and returns the generation its hits will arrive under.
    pub fn search(&mut self, root: Arc<Path>, query: String) -> io::Result<Generation> {
        let generation = self.bump();
        self.searches
            .send(Search {
                generation,
                root,
                query,
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the background reader has stopped",
                )
            })?;

        Ok(generation)
    }

    /// Ends the running search without starting another. The walk stops at its
    /// next directory and anything already on its way back is dropped.
    pub fn cancel(&mut self) {
        self.bump();
    }

    /// Moves to a generation nothing has been queued under yet, which is what
    /// makes every walk and every message in flight stale at once.
    fn bump(&mut self) -> Generation {
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Relaxed);
        self.generation
    }

    /// Takes everything waiting without blocking, keeping only what belongs to
    /// the newest search. Unlike `Progress`, a batch of hits is a delta and
    /// not a snapshot, so every batch that is still current is kept: dropping
    /// one would lose the hits it carried for good.
    pub fn drain(&mut self) -> Found {
        let mut found = Found::default();

        loop {
            match self.msgs.try_recv() {
                // A message of a replaced search is not merely useless: appending
                // its hits would mix two result lists into one.
                Ok(ReaderMsg::Hits { generation, .. } | ReaderMsg::Done { generation, .. })
                    if generation != self.generation => {}
                Ok(ReaderMsg::Hits { hits, .. }) => found.hits.extend(hits),
                Ok(ReaderMsg::Done { ended, .. }) => found.ended = Some(ended),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.health == Health::Running {
                        self.health = Health::Stopped;
                        found.health = Health::Stopped;
                    }
                    break;
                }
            }
        }

        found
    }
}

/// The reader loop. Ends when the handle is dropped, or when nobody is left to
/// receive a hit.
fn run(searches: &Receiver<Search>, msgs: &Sender<ReaderMsg>, latest: &AtomicU64, limits: &Limits) {
    for search in searches {
        // A search replaced while it waited its turn never starts: its hits
        // would be dropped on arrival, and the walk they cost is the whole
        // expense of the feature.
        if latest.load(Ordering::Relaxed) != search.generation {
            continue;
        }

        let mut emitter = Emitter::new(msgs, latest, search.generation);
        let ended = find::walk(search.root, &search.query, limits, &mut emitter);
        emitter.flush();

        let done = ReaderMsg::Done {
            generation: search.generation,
            ended,
        };
        if msgs.send(done).is_err() {
            break;
        }
    }
}

/// Turns a running walk into messages. Hits are gathered and sent in batches:
/// the main loop redraws ten times a second, so a message per hit would only
/// fill the channel.
struct Emitter<'a> {
    msgs: &'a Sender<ReaderMsg>,
    latest: &'a AtomicU64,
    generation: Generation,
    buffer: Vec<DirEntry>,
    last_sent: Instant,
}

impl<'a> Emitter<'a> {
    /// The longest a hit sits in the buffer. Below the main loop's own tick,
    /// so a batch is never what the next frame waits for.
    const FLUSH_EVERY: Duration = Duration::from_millis(50);

    /// The buffer length that sends without waiting for the clock, so a query
    /// matching thousands of names does not grow one message.
    const FLUSH_AT: usize = 64;

    fn new(msgs: &'a Sender<ReaderMsg>, latest: &'a AtomicU64, generation: Generation) -> Self {
        Self {
            msgs,
            latest,
            generation,
            buffer: Vec::new(),
            last_sent: Instant::now(),
        }
    }

    /// Sends what has been gathered, if anything. Nothing here may be dropped
    /// the way a stale `Progress` is: a hit that is not sent is a hit the list
    /// never shows.
    fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        self.last_sent = Instant::now();
        let _ = self.msgs.send(ReaderMsg::Hits {
            generation: self.generation,
            hits: std::mem::take(&mut self.buffer),
        });
    }
}

impl find::Observer for Emitter<'_> {
    fn found(&mut self, entry: DirEntry) {
        self.buffer.push(entry);
        if self.buffer.len() >= Self::FLUSH_AT || self.last_sent.elapsed() >= Self::FLUSH_EVERY {
            self.flush();
        }
    }

    fn cancelled(&self) -> bool {
        self.latest.load(Ordering::Relaxed) != self.generation
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    use crate::fs::temp_tree::TempTree;

    /// Drains the way the main loop does until the live search reports its
    /// end, and gives up rather than hanging if it never does.
    fn settle(reader: &mut Reader) -> Found {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = Found::default();

        while found.ended.is_none() {
            assert!(Instant::now() < deadline, "the search never reported back");
            let drained = reader.drain();
            found.hits.extend(drained.hits);
            found.ended = drained.ended;
            found.health = drained.health;
            thread::sleep(Duration::from_millis(1));
        }

        found
    }

    fn names(found: &Found) -> Vec<String> {
        found
            .hits
            .iter()
            .map(|hit| hit.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_search_reports_its_hits_and_then_its_end() {
        let tree = TempTree::of(["target-a", "sub/target-b", "other"]);

        let mut reader = Reader::start(Limits::default());
        reader
            .search(Arc::from(tree.path()), "target".into())
            .unwrap();
        let found = settle(&mut reader);

        assert_eq!(found.ended, Some(Ended::Exhausted));
        assert_eq!(names(&found), ["target-a", "target-b"]);
    }

    #[test]
    fn hits_come_back_in_walk_order_across_batches() {
        // More files than one batch holds, so the list is assembled from
        // several messages and any reordering between them would show.
        let files: Vec<String> = (0..200).map(|i| format!("sub/target-{i:03}")).collect();
        let tree = TempTree::of(&files);

        let mut reader = Reader::start(Limits {
            max_hits: 500,
            ..Limits::default()
        });
        reader
            .search(Arc::from(tree.path()), "target-".into())
            .unwrap();
        let found = settle(&mut reader);

        let mut expected = names(&found);
        expected.sort();
        assert_eq!(found.hits.len(), files.len());
        // One directory is listed sorted, so walk order is name order here.
        assert_eq!(names(&found), expected);
    }

    #[test]
    fn the_results_of_a_replaced_search_never_surface() {
        let tree = TempTree::of(["alpha", "beta"]);

        let mut reader = Reader::start(Limits::default());
        let replaced = reader
            .search(Arc::from(tree.path()), "alpha".into())
            .unwrap();
        let live = reader
            .search(Arc::from(tree.path()), "beta".into())
            .unwrap();
        assert_ne!(replaced, live);

        let found = settle(&mut reader);

        // Nothing of the first search is here, whether it ran before being
        // replaced or was skipped in the queue.
        assert_eq!(names(&found), ["beta"]);
    }

    #[test]
    fn cancelling_leaves_nothing_to_be_drained() {
        let tree = TempTree::of(["target-a"]);

        let mut reader = Reader::start(Limits::default());
        reader
            .search(Arc::from(tree.path()), "target".into())
            .unwrap();
        reader.cancel();

        // The walk is short enough that it may well have finished already; the
        // point is that its messages are stale either way.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            let drained = reader.drain();
            assert!(drained.hits.is_empty());
            assert_eq!(drained.ended, None);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_search_after_a_cancel_is_live_again() {
        let tree = TempTree::of(["target-a"]);

        let mut reader = Reader::start(Limits::default());
        reader
            .search(Arc::from(tree.path()), "target".into())
            .unwrap();
        reader.cancel();
        reader
            .search(Arc::from(tree.path()), "target".into())
            .unwrap();

        assert_eq!(names(&settle(&mut reader)), ["target-a"]);
    }

    #[test]
    fn the_end_says_the_walk_stopped_at_the_hit_limit() {
        let files: Vec<String> = (0..10).map(|i| format!("target-{i}")).collect();
        let tree = TempTree::of(&files);

        let mut reader = Reader::start(Limits {
            max_hits: 3,
            ..Limits::default()
        });
        reader
            .search(Arc::from(tree.path()), "target".into())
            .unwrap();
        let found = settle(&mut reader);

        assert_eq!(found.ended, Some(Ended::HitLimit));
        assert_eq!(found.hits.len(), 3);
    }
}
