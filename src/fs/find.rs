//! Walking a tree for names that match. [`walk`] is pure: no threads, no
//! channels. What runs it decides where the hits go and when to stop, the same
//! way `ops` leaves that to its [`Observer`](crate::fs::ops::Observer).
//!
//! [`Search`] is what runs it on a [`Reader`](crate::fs::reader::Reader), and
//! [`Found`] is how the answer comes back. Both live here rather than beside
//! the reader because batching hits and folding them into a list is the
//! discipline of *this* read and of no other.

use std::{
    collections::VecDeque,
    ffi::OsString,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::fs::{
    directory::{Detail, DirEntry, DirEntryKind, Directory},
    reader::{Drained, Live, Outbox, ReadJob},
    worker::Health,
};

/// Watches a walk while it runs. A hit is handed over the moment it is found
/// rather than collected into a list at the end, which is what lets a caller
/// show results while the tree is still being read.
pub trait Observer {
    /// Called once per entry whose name matches.
    fn found(&mut self, entry: DirEntry);

    /// Checked before every directory. `true` ends the walk.
    fn cancelled(&self) -> bool;
}

/// Why a walk stopped. A caller that shows the hits needs this to word the
/// difference between "that is all of them" and "that is as many as I look
/// for".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The whole tree within the limits was read.
    Exhausted,
    HitLimit,
    Cancelled,
}

/// The bounds a walk keeps to. Without them a walk of `$HOME` reads a tree
/// nobody asked about, for longer than anybody waits.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Hits after which the walk stops. Reached, it leaves the rest of the
    /// tree unread.
    pub max_hits: usize,
    /// How many levels below the root the walk descends. Zero reads the root
    /// directory and nothing under it.
    pub max_depth: usize,
    /// Directory names the walk does not descend into. Matched against the
    /// name alone, so it applies at every level.
    pub skip_dirs: Vec<OsString>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_hits: 200,
            max_depth: 16,
            skip_dirs: [".git", "node_modules", "target"]
                .into_iter()
                .map(OsString::from)
                .collect(),
        }
    }
}

impl Limits {
    fn skips(&self, path: &Path) -> bool {
        path.file_name()
            .is_some_and(|name| self.skip_dirs.iter().any(|skip| skip == name))
    }
}

/// Walks `root` breadth first, reporting every entry whose file name contains
/// `query`, case insensitively. Hits therefore arrive shallowest first, which
/// is the order they are worth showing in and means a list being appended to
/// never reorders under the cursor.
///
/// Nothing is followed into a symlink and no directory named in
/// [`Limits::skip_dirs`] is opened. A directory that cannot be read is stepped
/// over rather than failing the walk.
///
/// An empty `query` matches every entry.
pub fn walk(root: Arc<Path>, query: &str, limits: &Limits, observer: &mut dyn Observer) -> Ended {
    let mut needle = String::new();
    fold_into(query, &mut needle);

    let mut queue = VecDeque::from([(root, 0usize)]);
    let mut hits = 0;
    // One buffer for the whole walk: folding a name allocates, and a walk
    // reads hundreds of thousands of them.
    let mut folded = String::new();

    while let Some((dir, depth)) = queue.pop_front() {
        // Checked once per directory: reading one is the smallest step the
        // walk takes, and a walk that stops half way leaves nothing to undo.
        if observer.cancelled() {
            return Ended::Cancelled;
        }

        // A directory that cannot be listed contributes nothing, the way
        // `tree_size` returns zero instead of an error. Names alone: a walk
        // reads every directory below the root, so a `stat` per entry would
        // cost the whole tree rather than one screen of it.
        let Ok(listing) = Directory::read(Arc::clone(&dir), Detail::NamesOnly) else {
            continue;
        };

        for entry in listing.entries() {
            // `Directory::read` prefixes the listing with the parent, which
            // the walk has already come through; queueing it would send the
            // walk back up and never end.
            if entry.kind == DirEntryKind::Parent {
                continue;
            }

            let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
            fold_into(&name, &mut folded);

            if folded.contains(&needle) {
                observer.found(DirEntry {
                    path: Arc::clone(&entry.path),
                    kind: entry.kind,
                    meta: entry.meta,
                });
                hits += 1;
                if hits >= limits.max_hits {
                    return Ended::HitLimit;
                }
            }

            // Only a real directory is descended into. `kind` reports a
            // symlink as a symlink whatever it points at, which is what keeps
            // a link back up the tree out of the queue.
            if entry.kind == DirEntryKind::Directory
                && depth < limits.max_depth
                && !limits.skips(&entry.path)
            {
                queue.push_back((Arc::clone(&entry.path), depth + 1));
            }
        }
    }

    Ended::Exhausted
}

/// Lower-cases `text` into `buf`, replacing whatever was there. Folds
/// character by character the way `compare_file_names` orders, so a match is
/// case insensitive past ASCII too.
fn fold_into(text: &str, buf: &mut String) {
    buf.clear();
    buf.extend(text.chars().flat_map(char::to_lowercase));
}

/// A search on its way to the reader thread. Like a `Job`, it holds a snapshot
/// of what it needs and never reads back.
#[derive(Debug)]
pub struct Search {
    pub root: Arc<Path>,
    pub query: String,
}

/// What a running search says. [`Found::fold`] turns a drain of these into the
/// shape the overlay draws from.
#[derive(Debug)]
pub enum SearchMsg {
    /// Hits to append, in the order the walk reported them.
    Hits(Vec<DirEntry>),
    Done(Ended),
}

impl ReadJob for Search {
    /// The bounds belong to the searcher rather than to one question asked of
    /// it, so they are fixed when the reader starts.
    type Config = Limits;
    type Msg = SearchMsg;

    fn run(self, limits: &Limits, live: &Live<'_>, out: &Outbox<'_, SearchMsg>) {
        let mut emitter = Emitter::new(live, out);
        let ended = walk(self.root, &self.query, limits, &mut emitter);
        emitter.flush();
        out.send(SearchMsg::Done(ended));
    }
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

impl Found {
    /// Folds a drain into one answer. Unlike a snapshot, a batch of hits is a
    /// delta, so every batch that arrived is kept: dropping one would lose the
    /// hits it carried for good.
    pub fn fold(drained: Drained<SearchMsg>) -> Self {
        let mut found = Found {
            health: drained.health,
            ..Found::default()
        };

        for msg in drained.msgs {
            match msg {
                SearchMsg::Hits(hits) => found.hits.extend(hits),
                SearchMsg::Done(ended) => found.ended = Some(ended),
            }
        }

        found
    }
}

/// Turns a running walk into messages. Hits are gathered and sent in batches:
/// the main loop redraws ten times a second, so a message per hit would only
/// fill the channel.
struct Emitter<'a> {
    live: &'a Live<'a>,
    out: &'a Outbox<'a, SearchMsg>,
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

    fn new(live: &'a Live<'a>, out: &'a Outbox<'a, SearchMsg>) -> Self {
        Self {
            live,
            out,
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
        self.out
            .send(SearchMsg::Hits(std::mem::take(&mut self.buffer)));
    }
}

impl Observer for Emitter<'_> {
    fn found(&mut self, entry: DirEntry) {
        self.buffer.push(entry);
        if self.buffer.len() >= Self::FLUSH_AT || self.last_sent.elapsed() >= Self::FLUSH_EVERY {
            self.flush();
        }
    }

    fn cancelled(&self) -> bool {
        self.live.cancelled()
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;
    use std::path::PathBuf;

    use crate::fs::temp_tree::TempTree;

    /// Collects every hit, and stops the walk once it holds `stop_after` of
    /// them so a cancel can be tested without a race.
    struct Collected {
        hits: Vec<PathBuf>,
        stop_after: usize,
    }

    impl Collected {
        fn new() -> Self {
            Self {
                hits: Vec::new(),
                stop_after: usize::MAX,
            }
        }

        fn stopping_after(hits: usize) -> Self {
            Self {
                hits: Vec::new(),
                stop_after: hits,
            }
        }

        /// The hits as names, in the order they were reported.
        fn names(&self) -> Vec<String> {
            self.hits
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        }
    }

    impl Observer for Collected {
        fn found(&mut self, entry: DirEntry) {
            self.hits.push(entry.path.to_path_buf());
        }

        fn cancelled(&self) -> bool {
            self.hits.len() >= self.stop_after
        }
    }

    fn run(tree: &TempTree, query: &str, limits: &Limits) -> (Collected, Ended) {
        let mut collected = Collected::new();
        let ended = walk(Arc::from(tree.path()), query, limits, &mut collected);
        (collected, ended)
    }

    #[test]
    fn hits_arrive_shallowest_first() {
        let tree = TempTree::of(["target-a", "sub/target-b", "sub/deeper/target-c"]);

        let (collected, ended) = run(&tree, "target", &Limits::default());

        assert_eq!(ended, Ended::Exhausted);
        assert_eq!(collected.names(), ["target-a", "target-b", "target-c"]);
    }

    #[test]
    fn matching_ignores_case_on_both_sides() {
        let tree = TempTree::of(["Downloads/Photo.PNG"]);

        let (collected, _) = run(&tree, "pHoTo", &Limits::default());

        assert_eq!(collected.names(), ["Photo.PNG"]);
    }

    #[test]
    fn a_directory_matches_the_same_way_a_file_does() {
        let tree = TempTree::of(["downloads/note.txt"]);

        let (collected, _) = run(&tree, "download", &Limits::default());

        assert_eq!(collected.names(), ["downloads"]);
    }

    #[test]
    fn the_depth_limit_keeps_the_walk_off_the_lower_levels() {
        let tree = TempTree::of(["target-a", "sub/target-b", "sub/deeper/target-c"]);

        let limits = Limits {
            max_depth: 1,
            ..Limits::default()
        };
        let (collected, ended) = run(&tree, "target", &limits);

        assert_eq!(ended, Ended::Exhausted);
        assert_eq!(collected.names(), ["target-a", "target-b"]);
    }

    #[test]
    fn a_depth_of_zero_reads_the_root_and_nothing_under_it() {
        let tree = TempTree::of(["target-a", "sub/target-b"]);

        let limits = Limits {
            max_depth: 0,
            ..Limits::default()
        };
        let (collected, _) = run(&tree, "target", &limits);

        assert_eq!(collected.names(), ["target-a"]);
    }

    #[test]
    fn the_hit_limit_ends_the_walk_early() {
        let tree = TempTree::of(["target-a", "target-b", "target-c"]);

        let limits = Limits {
            max_hits: 2,
            ..Limits::default()
        };
        let (collected, ended) = run(&tree, "target", &limits);

        assert_eq!(ended, Ended::HitLimit);
        assert_eq!(collected.hits.len(), 2);
    }

    #[test]
    fn a_skipped_directory_is_never_opened() {
        let tree = TempTree::of(["target-a", "node_modules/target-b"]);

        let (collected, _) = run(&tree, "target", &Limits::default());

        // A skip keeps the walk out of the directory; the entry itself is
        // matched like any other and simply does not contain the query here.
        assert_eq!(collected.names(), ["target-a"]);
    }

    #[test]
    fn a_cancelled_walk_stops_and_says_so() {
        let tree = TempTree::of(["target-a", "sub/target-b", "sub/deeper/target-c"]);

        let mut collected = Collected::stopping_after(1);
        let ended = walk(
            Arc::from(tree.path()),
            "target",
            &Limits::default(),
            &mut collected,
        );

        assert_eq!(ended, Ended::Cancelled);
        assert_eq!(collected.names(), ["target-a"]);
    }

    #[test]
    fn the_walk_stays_under_the_root_it_was_given() {
        let tree = TempTree::of(["target-outside", "sub/target-inside"]);

        let mut collected = Collected::new();
        let ended = walk(
            Arc::from(tree.at("sub").as_path()),
            "target",
            &Limits::default(),
            &mut collected,
        );

        // The parent entry of `sub` leads straight back to a match; following
        // it would both report it and loop.
        assert_eq!(ended, Ended::Exhausted);
        assert_eq!(collected.names(), ["target-inside"]);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_loop_is_reported_but_not_walked_into() {
        let tree = TempTree::of(["sub/target-a"]);
        std::os::unix::fs::symlink(tree.path(), tree.at("sub/target-loop")).unwrap();

        let (collected, ended) = run(&tree, "target", &Limits::default());

        assert_eq!(ended, Ended::Exhausted);
        let mut names = collected.names();
        names.sort();
        assert_eq!(names, ["target-a", "target-loop"]);
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    use std::{thread, time::Duration};

    use crate::fs::{reader::Reader, temp_tree::TempTree};

    /// Drains the way the main loop does until the live search reports its
    /// end, and gives up rather than hanging if it never does.
    fn settle(reader: &mut Reader<Search>) -> Found {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = Found::default();

        while found.ended.is_none() {
            assert!(Instant::now() < deadline, "the search never reported back");
            let drained = Found::fold(reader.drain());
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

    fn search(reader: &mut Reader<Search>, tree: &TempTree, query: &str) {
        reader
            .send(Search {
                root: Arc::from(tree.path()),
                query: query.into(),
            })
            .unwrap();
    }

    #[test]
    fn a_search_reports_its_hits_and_then_its_end() {
        let tree = TempTree::of(["target-a", "sub/target-b", "other"]);

        let mut reader = Reader::<Search>::start(Limits::default());
        search(&mut reader, &tree, "target");
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

        let mut reader = Reader::<Search>::start(Limits {
            max_hits: 500,
            ..Limits::default()
        });
        search(&mut reader, &tree, "target-");
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

        let mut reader = Reader::<Search>::start(Limits::default());
        search(&mut reader, &tree, "alpha");
        search(&mut reader, &tree, "beta");
        let found = settle(&mut reader);

        // Nothing of the first search is here, whether it ran before being
        // replaced or was skipped in the queue.
        assert_eq!(names(&found), ["beta"]);
    }

    #[test]
    fn cancelling_leaves_nothing_to_be_drained() {
        let tree = TempTree::of(["target-a"]);

        let mut reader = Reader::<Search>::start(Limits::default());
        search(&mut reader, &tree, "target");
        reader.cancel();

        // The walk is short enough that it may well have finished already; the
        // point is that its messages are stale either way.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            let found = Found::fold(reader.drain());
            assert!(found.hits.is_empty());
            assert_eq!(found.ended, None);
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_search_after_a_cancel_is_live_again() {
        let tree = TempTree::of(["target-a"]);

        let mut reader = Reader::<Search>::start(Limits::default());
        search(&mut reader, &tree, "target");
        reader.cancel();
        search(&mut reader, &tree, "target");

        assert_eq!(names(&settle(&mut reader)), ["target-a"]);
    }

    #[test]
    fn the_end_says_the_walk_stopped_at_the_hit_limit() {
        let files: Vec<String> = (0..10).map(|i| format!("target-{i}")).collect();
        let tree = TempTree::of(&files);

        let mut reader = Reader::<Search>::start(Limits {
            max_hits: 3,
            ..Limits::default()
        });
        search(&mut reader, &tree, "target");
        let found = settle(&mut reader);

        assert_eq!(found.ended, Some(Ended::HitLimit));
        assert_eq!(found.hits.len(), 3);
    }
}
