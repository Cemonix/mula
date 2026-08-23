use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::fs::{
    job::{JobTag, Measure, Outcome, Progress, Work, WorkerMsg},
    ops::{MutationOp, Observer, ProcessedSummary, TransferOp, tree_size},
};

/// The envelope the queue carries: a piece of work and the tag its outcome has
/// to come back under. Never leaves this module — the main loop hands over a
/// `Work` and gets the tag back from `queue`.
#[derive(Debug)]
struct Job {
    tag: JobTag,
    work: Work,
}

/// Whether the worker thread is still there. `Stopped` is returned by the one
/// drain that discovers it is gone; every drain after that is quiet again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    #[default]
    Running,
    Stopped,
}

/// Everything the worker sent since the last drain, folded into the shape the
/// main loop draws from.
#[derive(Debug, Default)]
pub struct Drained {
    pub progress: Option<Progress>,
    pub finished: Vec<Outcome>,
    pub health: Health,
}

/// The main thread's end of the worker: a queue going out, messages coming
/// back, and one flag to stop whatever is running.
#[derive(Debug)]
pub struct Worker {
    jobs: Sender<Job>,
    msgs: Receiver<WorkerMsg>,
    cancel: Arc<AtomicBool>,
    queued: usize,
    next_tag: JobTag,
    health: Health,
}

impl Worker {
    /// Starts the thread. It lives as long as this handle: dropping the handle
    /// closes the job channel, which ends the loop.
    pub fn start() -> Self {
        let (job_tx, job_rx) = mpsc::channel();
        let (msg_tx, msg_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let worker_cancel = Arc::clone(&cancel);
        thread::spawn(move || run(&job_rx, &msg_tx, &worker_cancel));

        Self {
            jobs: job_tx,
            msgs: msg_rx,
            cancel,
            queued: 0,
            next_tag: 0,
            health: Health::Running,
        }
    }

    /// Hands work over and returns the tag its outcome will carry. Jobs run one
    /// at a time in the order they were queued.
    pub fn queue(&mut self, work: Work) -> io::Result<JobTag> {
        let tag = self.next_tag;
        self.jobs.send(Job { tag, work }).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the background worker has stopped",
            )
        })?;

        self.next_tag += 1;
        self.queued += 1;
        Ok(tag)
    }

    /// Asks the queued job to stop at its next entry, whether it has started
    /// yet or not: pressing cancel the instant after queueing has to reach the
    /// job that was queued. Jobs behind it are untouched — the worker spends
    /// the flag on the job that ends — so cancelling three times cancels three
    /// jobs.
    pub fn cancel(&self) {
        // With an empty queue there is nothing to stop, and setting the flag
        // would only reach whatever is queued next.
        if self.is_idle() {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// How many jobs are queued, counting the one that is running.
    pub fn queued(&self) -> usize {
        self.queued
    }

    pub fn is_idle(&self) -> bool {
        self.queued == 0
    }

    /// Takes everything waiting without blocking. Progress is overwritten as it
    /// goes, since only the newest is worth drawing; every outcome is kept.
    pub fn drain(&mut self) -> Drained {
        let mut drained = Drained::default();

        loop {
            match self.msgs.try_recv() {
                Ok(WorkerMsg::Progress(progress)) => drained.progress = Some(progress),
                Ok(WorkerMsg::Done(outcome)) => {
                    self.queued = self.queued.saturating_sub(1);
                    drained.finished.push(outcome);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Nothing queued will ever report back now, so the count
                    // has to go to zero or the bar waits forever.
                    self.queued = 0;
                    if self.health == Health::Running {
                        self.health = Health::Stopped;
                        drained.health = Health::Stopped;
                    }
                    break;
                }
            }
        }

        drained
    }
}

/// The worker loop. Ends when the handle is dropped, or when nobody is left to
/// receive an outcome.
fn run(jobs: &Receiver<Job>, msgs: &Sender<WorkerMsg>, cancel: &AtomicBool) {
    for job in jobs {
        let mut reporter = Reporter::new(msgs, cancel);
        let outcome = execute(job, &mut reporter);

        // Whatever cancel there was has been spent on the job that just ended.
        // Clearing it here rather than before the next job starts is what lets
        // a cancel that arrived before the job began still reach it, and still
        // keeps it from reaching the job behind.
        cancel.store(false, Ordering::Relaxed);
        if msgs.send(WorkerMsg::Done(outcome)).is_err() {
            break;
        }
    }
}

/// Whether an item was dealt with or stepped over. A skip is not a failure and
/// leaves no mark behind to retry.
enum Handled {
    Done,
    Skipped,
}

fn execute(job: Job, reporter: &mut Reporter) -> Outcome {
    let kind = job.work.kind();
    let mut failed = Vec::new();
    let mut reason = None;

    let summary = match job.work {
        Work::Transfer { op, items, to_dir } => {
            // Weighing the tree first is what lets the bar move evenly across
            // items of wildly different sizes. It is a walk of `metadata`
            // calls, no data, and it warms the cache for the copy that follows.
            let sizes: Vec<u64> = items.iter().map(|item| tree_size(item)).collect();
            reporter.begin(
                items.len(),
                Measure::Bytes {
                    done: 0,
                    total: sizes.iter().sum(),
                },
            );

            let mut summary = ProcessedSummary::new(items.len());
            for (item, size) in items.into_iter().zip(sizes) {
                if reporter.cancelled() {
                    break;
                }
                reporter.start_item(&item, size);

                match transfer_item(op, &item, &to_dir, reporter) {
                    Ok(Handled::Done) => summary.process(),
                    Ok(Handled::Skipped) => summary.skip(),
                    // A cancelled transfer fails with whatever error unwound
                    // it; the flag, not the error, is what says it was us.
                    Err(_) if reporter.cancelled() => break,
                    Err(e) => {
                        tracing::error!(path = ?item, error = %e, "transfer failed");
                        reason = Some(e.to_string());
                        summary.fail();
                        failed.push(item);
                    }
                }

                reporter.finish_item();
            }
            summary
        }

        Work::Delete { items } => {
            reporter.begin(items.len(), Measure::Items);

            let mut summary = ProcessedSummary::new(items.len());
            for item in items {
                if reporter.cancelled() {
                    break;
                }
                reporter.start_item(&item, 0);

                match (MutationOp::Delete { path: item.clone() }).execute() {
                    Ok(()) => summary.process(),
                    // A mark can outlive the file it points at: marks survive a
                    // directory change, and a retried batch walks over what the
                    // first pass removed.
                    Err(e) if e.kind() == io::ErrorKind::NotFound => summary.skip(),
                    Err(e) => {
                        tracing::error!(path = ?item, error = %e, "delete failed");
                        reason = Some(e.to_string());
                        summary.fail();
                        failed.push(item);
                    }
                }

                reporter.finish_item();
            }
            summary
        }

        Work::Mutate(op) => {
            reporter.begin(1, Measure::Items);

            let mut summary = ProcessedSummary::new(1);
            match op.execute() {
                Ok(()) => summary.process(),
                Err(e) => {
                    tracing::error!(error = %e, "mutation failed");
                    reason = Some(e.to_string());
                    summary.fail();
                }
            }
            reporter.finish_item();
            summary
        }
    };

    Outcome {
        tag: job.tag,
        kind,
        summary,
        failed,
        reason,
    }
}

fn transfer_item(
    op: TransferOp,
    item: &Path,
    to_dir: &Path,
    watcher: &mut dyn Observer,
) -> io::Result<Handled> {
    // The same directory on both sides is a skip, not a failure. A path without
    // a parent has nowhere to come from and skips too.
    if item.parent().is_none_or(|source| source == to_dir) {
        return Ok(Handled::Skipped);
    }

    let Some(file_name) = item.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name to transfer under", item.display()),
        ));
    };

    op.execute(item, &to_dir.join(file_name), watcher)?;
    Ok(Handled::Done)
}

/// Turns a running job into `Progress` messages. It carries the counters the
/// operations themselves have no reason to know: which item is current, and
/// what the finished ones already weighed.
struct Reporter<'a> {
    msgs: &'a Sender<WorkerMsg>,
    cancel: &'a AtomicBool,
    items_done: usize,
    items_total: usize,
    measure: Measure,
    current: String,
    /// What the finished items weighed, so an item that reports nothing on its
    /// way (a `rename`) still moves the bar once it lands.
    base_bytes: u64,
    item_bytes: u64,
    item_size: u64,
    last_sent: Option<Instant>,
}

impl<'a> Reporter<'a> {
    /// The shortest gap between two progress messages. A tree of small files
    /// finishes thousands of them a second and the main loop redraws ten times
    /// a second, so reporting every one would only fill the channel.
    const REPORT_EVERY: Duration = Duration::from_millis(50);

    fn new(msgs: &'a Sender<WorkerMsg>, cancel: &'a AtomicBool) -> Self {
        Self {
            msgs,
            cancel,
            items_done: 0,
            items_total: 0,
            measure: Measure::Items,
            current: String::new(),
            base_bytes: 0,
            item_bytes: 0,
            item_size: 0,
            last_sent: None,
        }
    }

    fn begin(&mut self, items_total: usize, measure: Measure) {
        self.items_total = items_total;
        self.measure = measure;
    }

    /// The last component of a path, or the whole path when it has no name.
    fn label(path: &Path) -> String {
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }

    fn start_item(&mut self, path: &Path, size: u64) {
        self.current = Self::label(path);
        self.item_size = size;
        self.item_bytes = 0;
        self.send();
    }

    fn finish_item(&mut self) {
        self.items_done += 1;
        self.base_bytes += self.item_size;
        self.item_bytes = 0;
        self.send();
    }

    /// Sends the current state unless one went out moments ago. Dropping a
    /// message costs nothing: the next one carries the whole state, and the
    /// outcome settles the final numbers.
    fn send(&mut self) {
        let now = Instant::now();
        if self
            .last_sent
            .is_some_and(|last| now.duration_since(last) < Self::REPORT_EVERY)
        {
            return;
        }
        self.last_sent = Some(now);

        if let Measure::Bytes { done, .. } = &mut self.measure {
            *done = self.base_bytes + self.item_bytes;
        }

        let _ = self.msgs.send(WorkerMsg::Progress(Progress {
            items_done: self.items_done,
            items_total: self.items_total,
            current: self.current.clone(),
            measure: self.measure,
        }));
    }
}

impl Observer for Reporter<'_> {
    fn entry_copied(&mut self, path: &Path, bytes: u64) {
        self.item_bytes += bytes;
        // Inside a marked directory the name worth showing is the file being
        // written, not the directory the batch counts.
        self.current = Self::label(path);
        self.send();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::{fs, path::PathBuf};

    use crate::fs::{job::JobKind, temp_tree::TempTree};

    /// Drains until every queued job has reported, the way the main loop does,
    /// and gives up rather than hanging if the worker never answers.
    fn settle(worker: &mut Worker) -> Vec<Outcome> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut finished = Vec::new();

        while !worker.is_idle() {
            assert!(Instant::now() < deadline, "the worker never reported back");
            finished.extend(worker.drain().finished);
            thread::sleep(Duration::from_millis(1));
        }

        finished
    }

    #[test]
    fn a_delete_job_reports_what_it_removed() {
        let t = TempTree::new();
        let items = vec![t.make_file("a.txt", "a"), t.make_file("b.txt", "b")];

        let mut worker = Worker::start();
        worker.queue(Work::Delete { items }).unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].summary.processed(), 2);
        assert!(finished[0].failed.is_empty());
        assert!(!t.at("a.txt").exists());
    }

    #[test]
    fn the_outcome_carries_the_tag_it_was_queued_under() {
        let t = TempTree::new();

        let mut worker = Worker::start();
        let first = worker
            .queue(Work::Delete {
                items: vec![t.make_file("a.txt", "a")],
            })
            .unwrap();
        let second = worker
            .queue(Work::Delete {
                items: vec![t.make_file("b.txt", "b")],
            })
            .unwrap();

        let finished = settle(&mut worker);
        let tags: Vec<JobTag> = finished.iter().map(|outcome| outcome.tag).collect();

        // One worker takes the queue in order, so the outcomes come back in the
        // order the jobs went out.
        assert_eq!(tags, vec![first, second]);
    }

    #[test]
    fn a_transfer_copies_and_counts_its_bytes() {
        let t = TempTree::new();
        let items = vec![t.make_file("src/a.txt", "aaaa")];
        let to_dir = t.make_dir("dest");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items,
                to_dir,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].summary.processed(), 1);
        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "aaaa");
    }

    #[test]
    fn an_item_that_fails_comes_back_so_it_can_be_marked_again() {
        let t = TempTree::new();
        let present = t.make_file("src/a.txt", "a");
        let to_dir = t.make_dir("dest");
        // A destination that is already taken is the failure that is easiest to
        // arrange and is exactly what a retry is for.
        t.make_file("dest/a.txt", "in the way");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items: vec![present.clone()],
                to_dir,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].failed, vec![present]);
        assert!(finished[0].reason.is_some());
    }

    #[test]
    fn a_mutation_brings_its_error_back_rather_than_a_count() {
        let t = TempTree::new();

        let mut worker = Worker::start();
        worker
            .queue(Work::Mutate(MutationOp::Rename {
                path: t.at("missing.txt"),
                new_name: "other.txt".to_string(),
            }))
            .unwrap();
        let finished = settle(&mut worker);

        assert!(matches!(finished[0].kind, JobKind::Mutate));
        assert!(finished[0].summary.has_failures());
        assert!(finished[0].reason.is_some());
    }

    #[test]
    fn cancelling_stops_the_batch_and_leaves_the_rest_untried() {
        let t = TempTree::new();
        let items: Vec<PathBuf> = (0..200)
            .map(|i| t.make_file(format!("src/{i}.txt"), "x"))
            .collect();
        let total = items.len();
        let to_dir = t.make_dir("dest");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items,
                to_dir,
            })
            .unwrap();
        worker.cancel();
        let finished = settle(&mut worker);

        let summary = &finished[0].summary;
        // The denominator is fixed when the batch starts, so a run that stopped
        // early still reports against what it set out to do.
        assert_eq!(summary.total(), total);
        assert!(summary.processed() < total, "{summary:?}");
        // Items that were never tried are not failures and leave no mark to
        // retry; only a real error does that.
        assert!(finished[0].failed.is_empty());
    }

    #[test]
    fn cancelling_a_job_that_has_already_started_stops_it() {
        let t = TempTree::new();
        let items: Vec<PathBuf> = (0..2000)
            .map(|i| t.make_file(format!("src/{i}.txt"), "x"))
            .collect();
        let total = items.len();
        let to_dir = t.make_dir("dest");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items,
                to_dir,
            })
            .unwrap();

        // Wait for the job to actually be under way before cancelling, which is
        // the path a keypress during a long copy takes.
        let deadline = Instant::now() + Duration::from_secs(5);
        while worker.drain().progress.is_none() {
            assert!(Instant::now() < deadline, "the job never started");
            thread::sleep(Duration::from_millis(1));
        }
        worker.cancel();

        let finished = settle(&mut worker);
        let summary = &finished[0].summary;
        assert!(summary.processed() < total, "{summary:?}");
    }

    #[test]
    fn a_cancel_between_jobs_does_not_reach_the_next_one() {
        let t = TempTree::new();

        let mut worker = Worker::start();
        worker
            .queue(Work::Delete {
                items: vec![t.make_file("a.txt", "a")],
            })
            .unwrap();
        settle(&mut worker);

        // The flag is set while nothing is running; the worker drops it before
        // it picks the next job up.
        worker.cancel();
        worker
            .queue(Work::Delete {
                items: vec![t.make_file("b.txt", "b")],
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].summary.processed(), 1);
        assert!(!t.at("b.txt").exists());
    }
}
