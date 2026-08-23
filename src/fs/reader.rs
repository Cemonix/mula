//! The reading side of `fs`. A reader owns one thread, answers one question at
//! a time, and throws away the answers to questions nobody is asking any more.
//! Reads never mutate anything, so they do not queue behind the mutations and
//! the "one worker" rule, which is about the order of writes, is untouched.
//!
//! Its discipline is the opposite of the job queue's. Mutations are FIFO and
//! every one is delivered; a read is *last wins*: typing another character
//! replaces the search that was running, and the results it had left are worth
//! nothing. A generation counter says which request is the live one — it both
//! stops the work and drops whatever it had already sent.
//!
//! What is generic here is that counter, the pair of channels and the thread.
//! What each kind of read does with a message once it is known to be live is
//! not: a search sends hits as a *delta* and every live batch has to be kept,
//! while a preview sends a *snapshot* and only the newest one matters. Folding
//! therefore belongs to the caller, next to the job it folds.

use std::{
    fmt, io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
};

use crate::fs::worker::Health;

/// Which request a message belongs to. Unlike a `JobTag`, which finds *its own*
/// job among several running ones, this exists to throw *older* ones away:
/// only the newest generation is live, and everything else is stale by
/// definition.
pub type Generation = u64;

/// The generation a running job belongs to, and the counter it keeps comparing
/// itself against. A job holds one of these for as long as it runs and is the
/// only thing that can tell it to stop.
#[derive(Debug)]
pub struct Live<'a> {
    latest: &'a AtomicU64,
    generation: Generation,
}

impl<'a> Live<'a> {
    /// A handle over a counter of the test's own, so a job can be run without
    /// a thread behind it.
    #[cfg(test)]
    pub fn at(latest: &'a AtomicU64, generation: Generation) -> Self {
        Self { latest, generation }
    }

    /// `true` once another request has been sent, or the reader cancelled.
    /// A job that runs longer than a few milliseconds is expected to ask
    /// before every expensive step.
    pub fn cancelled(&self) -> bool {
        self.latest.load(Ordering::Relaxed) != self.generation
    }
}

/// Where a running job puts what it has to say. Every message is tagged with
/// the generation of the job that sent it, so the main thread can drop what is
/// stale without the job having to know it was replaced.
#[derive(Debug)]
pub struct Outbox<'a, M> {
    msgs: &'a Sender<(Generation, M)>,
    generation: Generation,
}

impl<M> Outbox<'_, M> {
    /// Sends one message. Returns `false` once the reader handle is gone and
    /// nothing will be received again, which is a job's cue to stop early; a
    /// job that has nothing left to do may ignore it.
    pub fn send(&self, msg: M) -> bool {
        self.msgs.send((self.generation, msg)).is_ok()
    }
}

/// One kind of read: what it needs to run, and what it says while running.
///
/// A job owns a snapshot of everything it reads, taken when it was sent, the
/// same way a `Job` does. `Config` is what describes the *reader* rather than
/// one question asked of it, so it is fixed when the thread starts and handed
/// to every job by reference.
pub trait ReadJob: Send + 'static {
    type Config: Send + 'static;
    type Msg: Send + 'static;

    fn run(self, config: &Self::Config, live: &Live<'_>, out: &Outbox<'_, Self::Msg>);
}

/// Everything the thread sent since the last drain that still belongs to the
/// newest request, in the order it was sent.
#[derive(Debug)]
pub struct Drained<M> {
    pub msgs: Vec<M>,
    pub health: Health,
}

/// The main thread's end of a reader: requests going out, messages coming
/// back, and one counter saying which request those messages are still wanted
/// for.
pub struct Reader<J: ReadJob> {
    jobs: Sender<(Generation, J)>,
    msgs: Receiver<(Generation, J::Msg)>,
    /// The counter every running job compares itself against. Raising it is
    /// the only way to stop a job, and does so whether it has started or is
    /// still in the queue.
    latest: Arc<AtomicU64>,
    /// The main thread's copy of `latest`; the two never disagree, since only
    /// the main thread writes.
    generation: Generation,
    health: Health,
}

/// Prints what the main thread owns rather than the queue behind it, so a
/// reader can sit in a `Debug` struct without its messages having to be
/// printable — an image preview holds a bitmap.
impl<J: ReadJob> fmt::Debug for Reader<J> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("generation", &self.generation)
            .field("health", &self.health)
            .finish_non_exhaustive()
    }
}

impl<J: ReadJob> Reader<J> {
    /// Starts the thread. It lives as long as this handle: dropping the handle
    /// closes the request channel, which ends the loop.
    pub fn start(config: J::Config) -> Self {
        let (job_tx, job_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        // Generation zero is the one no request is ever sent under, so before
        // the first one every message that could arrive is already stale.
        let latest = Arc::new(AtomicU64::new(0));

        let thread_latest = Arc::clone(&latest);
        thread::spawn(move || run::<J>(&job_rx, &msg_tx, &thread_latest, &config));

        Self {
            jobs: job_tx,
            msgs: msg_rx,
            latest,
            generation: 0,
            health: Health::Running,
        }
    }

    /// Replaces whatever was running with `job`, and returns the generation
    /// its messages will arrive under.
    pub fn send(&mut self, job: J) -> io::Result<Generation> {
        let generation = self.bump();
        self.jobs.send((generation, job)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the background reader has stopped",
            )
        })?;

        Ok(generation)
    }

    /// Ends the running job without starting another. It stops at its next
    /// checkpoint and anything already on its way back is dropped.
    pub fn cancel(&mut self) {
        self.bump();
    }

    /// Moves to a generation nothing has been sent under yet, which is what
    /// makes every running job and every message in flight stale at once.
    fn bump(&mut self) -> Generation {
        self.generation += 1;
        self.latest.store(self.generation, Ordering::Relaxed);
        self.generation
    }

    /// Takes everything waiting without blocking, keeping only what belongs to
    /// the newest request.
    pub fn drain(&mut self) -> Drained<J::Msg> {
        let mut msgs = Vec::new();
        let mut health = Health::Running;

        loop {
            match self.msgs.try_recv() {
                // A message of a replaced request is not merely useless:
                // folded in, it would mix two answers into one.
                Ok((generation, _)) if generation != self.generation => {}
                Ok((_, msg)) => msgs.push(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.health == Health::Running {
                        self.health = Health::Stopped;
                        health = Health::Stopped;
                    }
                    break;
                }
            }
        }

        Drained { msgs, health }
    }
}

/// The reader loop. Ends when the handle is dropped.
fn run<J: ReadJob>(
    jobs: &Receiver<(Generation, J)>,
    msgs: &Sender<(Generation, J::Msg)>,
    latest: &AtomicU64,
    config: &J::Config,
) {
    for (generation, job) in jobs {
        // A request replaced while it waited its turn never starts: its
        // messages would be dropped on arrival, and the work they cost is the
        // whole expense of the feature.
        if latest.load(Ordering::Relaxed) != generation {
            continue;
        }

        let live = Live { latest, generation };
        let out = Outbox { msgs, generation };
        job.run(config, &live, &out);
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;

    use std::time::{Duration, Instant};

    /// A job that says the word it was given and then reports that it is done.
    /// Long enough to be replaced mid-flight when `hold` is set, so a stale
    /// message can be produced without a race.
    #[derive(Debug)]
    struct Say {
        word: &'static str,
        /// Held until the test lets go, which is how a job is kept running
        /// while another one replaces it.
        hold: Option<Receiver<()>>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Said {
        Word(&'static str),
        Done,
    }

    impl Say {
        fn new(word: &'static str) -> Self {
            Self { word, hold: None }
        }
    }

    impl ReadJob for Say {
        type Config = &'static str;
        type Msg = Said;

        fn run(self, config: &Self::Config, live: &Live<'_>, out: &Outbox<'_, Self::Msg>) {
            out.send(Said::Word(config));
            if let Some(hold) = self.hold {
                let _ = hold.recv_timeout(Duration::from_secs(5));
            }
            // The checkpoint a long job is expected to keep: a replaced job
            // stops rather than finishing work nobody wants.
            if live.cancelled() {
                return;
            }
            out.send(Said::Word(self.word));
            out.send(Said::Done);
        }
    }

    /// Drains the way the main loop does until `Done` arrives, and gives up
    /// rather than hanging if it never does.
    fn settle(reader: &mut Reader<Say>) -> Vec<Said> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut said = Vec::new();

        loop {
            assert!(Instant::now() < deadline, "the job never reported back");
            said.extend(reader.drain().msgs);
            if said.contains(&Said::Done) {
                return said;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_job_reports_under_the_generation_it_was_sent_with() {
        let mut reader = Reader::<Say>::start("config");

        let first = reader.send(Say::new("a")).unwrap();
        let second = reader.send(Say::new("b")).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn the_config_reaches_every_job() {
        let mut reader = Reader::<Say>::start("config");
        reader.send(Say::new("a")).unwrap();

        assert_eq!(settle(&mut reader)[0], Said::Word("config"));
    }

    #[test]
    fn the_messages_of_a_replaced_job_never_surface() {
        let (release, hold) = mpsc::channel();
        let mut reader = Reader::<Say>::start("config");

        // The first job is held after its first message, so it is certain to
        // be running when the second replaces it.
        reader
            .send(Say {
                word: "stale",
                hold: Some(hold),
            })
            .unwrap();
        // Waits for that first message to actually be in flight.
        let deadline = Instant::now() + Duration::from_secs(5);
        while reader.msgs.try_recv().is_err() {
            assert!(Instant::now() < deadline, "the held job never started");
            thread::sleep(Duration::from_millis(1));
        }

        reader.send(Say::new("live")).unwrap();
        let _ = release.send(());

        assert_eq!(
            settle(&mut reader),
            [Said::Word("config"), Said::Word("live"), Said::Done]
        );
    }

    #[test]
    fn a_job_replaced_before_it_started_never_runs() {
        let mut reader = Reader::<Say>::start("config");

        reader.send(Say::new("first")).unwrap();
        reader.send(Say::new("second")).unwrap();
        let said = settle(&mut reader);

        // Only the live job's own messages are here. The first may have been
        // skipped in the queue or stopped part way; either way nothing of it
        // is delivered.
        assert_eq!(
            said,
            [Said::Word("config"), Said::Word("second"), Said::Done]
        );
    }

    #[test]
    fn cancelling_leaves_nothing_to_be_drained() {
        let mut reader = Reader::<Say>::start("config");

        reader.send(Say::new("a")).unwrap();
        reader.cancel();

        // The job is short enough that it may well have finished already; the
        // point is that its messages are stale either way.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            assert!(reader.drain().msgs.is_empty());
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_job_after_a_cancel_is_live_again() {
        let mut reader = Reader::<Say>::start("config");

        reader.send(Say::new("a")).unwrap();
        reader.cancel();
        reader.send(Say::new("b")).unwrap();

        assert_eq!(settle(&mut reader).last(), Some(&Said::Done));
    }

    #[test]
    fn a_stopped_thread_is_reported_once() {
        let mut reader = Reader::<Say>::start("config");
        // Dropping the sending half from under the handle is what a panicked
        // thread leaves behind.
        let (_, dead) = mpsc::channel();
        reader.msgs = dead;

        assert_eq!(reader.drain().health, Health::Stopped);
        assert_eq!(reader.drain().health, Health::Running);
    }
}
