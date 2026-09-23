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
    archive::{pack::pack, unpack::unpack},
    job::{JobTag, Measure, Outcome, Progress, Work, WorkerMsg},
    ops::{
        MutationError, MutationOp, Observer, Policy, ProcessedSummary, Transfer, TransferOp,
        Unfinished, tree_size,
    },
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

fn execute(job: Job, reporter: &mut Reporter) -> Outcome {
    let kind = job.work.kind();
    let touched = job.work.touched();
    let mut failed = Vec::new();
    let mut collided = Vec::new();
    let mut kept = Vec::new();
    let mut left_out = Vec::new();
    let mut reason = None;

    let summary = match job.work {
        Work::Transfer { op, items, policy } => {
            // Weighing the tree first is what lets the bar move evenly across
            // items of wildly different sizes. It is a walk of `metadata`
            // calls, no data, and it warms the cache for the copy that follows.
            let sizes: Vec<u64> = items.iter().map(|item| tree_size(&item.from)).collect();
            reporter.begin(
                items.len(),
                Measure::Bytes {
                    done: 0,
                    total: sizes.iter().sum(),
                },
            );

            // Read in the same breath as the weighing and before a byte is
            // written, so what the batch writes itself is never something it
            // is then allowed to overwrite.
            let policy = Policy::new(policy, &items);

            let mut summary = ProcessedSummary::new(items.len());
            for (item, size) in items.into_iter().zip(sizes) {
                if reporter.cancelled() {
                    break;
                }
                reporter.start_item(&item.from, size);

                match transfer_item(op, &item, &policy, reporter) {
                    // An item is counted by what it left behind rather than by
                    // what went through: a tree that clashed at one leaf is not
                    // a tree the batch is finished with.
                    Ok(left) if left.nothing_left() => summary.process(),
                    Ok(mut left) if !left.collided.is_empty() => {
                        summary.collide();
                        collided.append(&mut left.collided);
                        kept.append(&mut left.kept);
                    }
                    Ok(mut left) => {
                        // Something landed, only under a name of its own.
                        if left.kept.is_empty() {
                            summary.skip();
                        } else {
                            summary.process();
                            kept.append(&mut left.kept);
                        }
                    }
                    // A cancelled transfer fails with whatever error unwound
                    // it; the flag, not the error, is what says it was us.
                    Err(_) if reporter.cancelled() => break,
                    Err(e) => {
                        tracing::error!(path = ?item.from, error = %e, "transfer failed");
                        reason = Some(e.to_string());
                        summary.fail();
                        failed.push(item.from);
                    }
                }

                reporter.finish_item();
            }
            summary
        }

        // One item at a time for both modes. `trash::delete_all` would leave a
        // single entry in the trash on macOS and Windows instead of one per
        // item, but it takes the whole batch in one call — and with it the
        // cancellation check between items and the counts a summary is made of.
        Work::Delete { items, mode } => {
            reporter.begin(items.len(), Measure::Items);

            let mut summary = ProcessedSummary::new(items.len());
            for item in items {
                if reporter.cancelled() {
                    break;
                }
                reporter.start_item(&item, 0);

                match (MutationOp::Delete {
                    path: item.clone(),
                    mode,
                })
                .execute()
                {
                    Ok(()) => summary.process(),
                    // A mark can outlive the file it points at: marks survive a
                    // directory change, and a retried batch walks over what the
                    // first pass removed.
                    Err(MutationError::IO(e)) if e.kind() == io::ErrorKind::NotFound => {
                        summary.skip()
                    }
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

        // One archive, however many items go into it, so the batch is the
        // archive and the bar fills with what the items weigh on disk.
        Work::Pack {
            items,
            archive,
            format,
        } => {
            let total = items.iter().map(|item| tree_size(item)).sum();
            reporter.begin(1, Measure::Bytes { done: 0, total });
            reporter.start_item(&archive, total);

            let mut summary = ProcessedSummary::new(1);
            match pack(&items, &archive, format, reporter) {
                Ok(packed) => {
                    summary.process();
                    left_out = packed.refused;
                }
                // A cancelled archive was removed with the run, so nothing
                // came of any of the items. They go back on the panel for the
                // key to be pressed again.
                Err(_) if reporter.cancelled() => failed = items,
                Err(e) => {
                    tracing::error!(path = ?archive, error = %e, "packing failed");
                    reason = Some(e.to_string());
                    summary.fail();
                    failed = items;
                }
            }
            reporter.finish_item();
            summary
        }

        // The archives are the batch, and each is weighed by its own size on
        // disk: what moves the bar is how far into the file the job has read.
        Work::Unpack { items, into } => {
            let sizes: Vec<u64> = items
                .iter()
                .map(|item| item.metadata().map(|meta| meta.len()).unwrap_or_default())
                .collect();
            reporter.begin(
                items.len(),
                Measure::Bytes {
                    done: 0,
                    total: sizes.iter().sum(),
                },
            );

            let mut summary = ProcessedSummary::new(items.len());
            let mut queue = items.into_iter().zip(sizes);
            while let Some((item, size)) = queue.next() {
                // An unpack leaves nothing behind when it is cancelled, so the
                // archive that was interrupted and the ones behind it are in
                // the same position: they all go back on the panel.
                if reporter.cancelled() {
                    failed.push(item);
                    failed.extend(queue.map(|(item, _)| item));
                    break;
                }
                reporter.start_item(&item, size);

                match unpack(&item, &into, reporter) {
                    Ok(unpacked) => {
                        summary.process();
                        kept.push(unpacked.into);
                        left_out.extend(unpacked.refused);
                    }
                    Err(_) if reporter.cancelled() => {
                        failed.push(item);
                        failed.extend(queue.by_ref().map(|(item, _)| item));
                        break;
                    }
                    Err(e) => {
                        tracing::error!(path = ?item, error = %e, "unpacking failed");
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
        collided,
        kept,
        left_out,
        reason,
        touched,
    }
}

fn transfer_item(
    op: TransferOp,
    item: &Transfer,
    policy: &Policy,
    watcher: &mut dyn Observer,
) -> io::Result<Unfinished> {
    // The same directory on both sides is a skip, not a failure. A path without
    // a parent has nowhere to come from and skips too.
    if item
        .from
        .parent()
        .is_none_or(|source| Some(source) == item.to.parent())
    {
        return Ok(Unfinished {
            skipped: 1,
            ..Unfinished::default()
        });
    }

    op.execute(item, policy, watcher)
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
    use crate::fs::ops::{DeleteMode, OnCollision};
    use std::{fs, path::PathBuf};

    use crate::fs::{archive::format::Format, job::JobKind, temp_tree::TempTree};

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
        worker
            .queue(Work::Delete {
                items,
                mode: DeleteMode::Permanent,
            })
            .unwrap();
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
                mode: DeleteMode::Permanent,
            })
            .unwrap();
        let second = worker
            .queue(Work::Delete {
                items: vec![t.make_file("b.txt", "b")],
                mode: DeleteMode::Permanent,
            })
            .unwrap();

        let finished = settle(&mut worker);
        let tags: Vec<JobTag> = finished.iter().map(|outcome| outcome.tag).collect();

        // One worker takes the queue in order, so the outcomes come back in the
        // order the jobs went out.
        assert_eq!(tags, vec![first, second]);
    }

    /// Packing and unpacking are one round trip through the queue, which is
    /// also the only way to see that the worker drives both halves.
    #[test]
    fn a_pack_job_and_an_unpack_job_bring_a_tree_back() {
        let t = TempTree::new();
        t.make_file("notes/one.txt", "one");
        let archive = t.at("notes.zip");
        let back = t.make_dir("back");

        let mut worker = Worker::start();
        worker
            .queue(Work::Pack {
                items: vec![t.at("notes")],
                archive: archive.clone(),
                format: Format::Zip,
            })
            .unwrap();
        worker
            .queue(Work::Unpack {
                items: vec![archive.clone()],
                into: back.clone(),
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert!(archive.exists(), "the archive was not written");
        assert_eq!(finished[0].summary.processed(), 1);
        assert!(matches!(finished[0].kind, JobKind::Pack));

        assert_eq!(finished[1].summary.processed(), 1);
        assert!(matches!(finished[1].kind, JobKind::Unpack));
        // The archive holds one directory, so it is lifted out of its wrapper
        // rather than unpacking as notes/notes.
        assert_eq!(finished[1].kept, vec![back.join("notes")]);
        assert_eq!(
            fs::read_to_string(back.join("notes/one.txt")).unwrap(),
            "one"
        );
    }

    /// Cancelling takes the archive with it, so nothing was done and every
    /// item goes back on the panel.
    #[test]
    fn a_cancelled_pack_gives_every_mark_back() {
        let t = TempTree::new();
        let items = vec![
            t.make_file("one.txt", "one"),
            t.make_file("two.txt", "two"),
            t.make_file("three.txt", "three"),
        ];
        let archive = t.at("notes.zip");

        let mut worker = Worker::start();
        worker
            .queue(Work::Pack {
                items: items.clone(),
                archive: archive.clone(),
                format: Format::Zip,
            })
            .unwrap();
        worker.cancel();
        let finished = settle(&mut worker);

        assert!(!archive.exists(), "a cancelled pack left an archive");
        assert_eq!(finished[0].failed, items);
    }

    /// A name that is taken fails the job, and the marks come back so the key
    /// can be pressed again once it is not.
    #[test]
    fn a_pack_onto_a_name_that_is_taken_gives_the_marks_back() {
        let t = TempTree::new();
        let item = t.make_file("one.txt", "one");
        let archive = t.make_file("notes.zip", "not an archive");

        let mut worker = Worker::start();
        worker
            .queue(Work::Pack {
                items: vec![item.clone()],
                archive: archive.clone(),
                format: Format::Zip,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert!(finished[0].summary.has_failures());
        assert_eq!(finished[0].failed, vec![item]);
        assert_eq!(fs::read_to_string(&archive).unwrap(), "not an archive");
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
                items: into_dir(items, &to_dir),
                policy: OnCollision::Refuse,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].summary.processed(), 1);
        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "aaaa");
    }

    /// The pairs a batch of `items` into `to_dir` flattens to, which is what
    /// the main loop hands over.
    fn into_dir(items: Vec<PathBuf>, to_dir: &Path) -> Vec<Transfer> {
        items
            .into_iter()
            .map(|from| Transfer {
                to: to_dir.join(from.file_name().unwrap()),
                from,
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn an_item_that_fails_comes_back_so_it_can_be_marked_again() {
        use std::os::unix::fs::PermissionsExt;

        let t = TempTree::new();
        let present = t.make_file("src/a.txt", "a");
        let to_dir = t.make_dir("dest");
        fs::set_permissions(&present, fs::Permissions::from_mode(0o000)).unwrap();

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items: into_dir(vec![present.clone()], &to_dir),
                policy: OnCollision::Refuse,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].failed, vec![present]);
        assert!(finished[0].reason.is_some());
    }

    /// A taken destination is a question, not a failure. Marking it again
    /// would offer a retry that would land in exactly the same place.
    #[test]
    fn a_taken_destination_comes_back_to_be_asked_about_rather_than_marked() {
        let t = TempTree::new();
        let present = t.make_file("src/a.txt", "a");
        let to_dir = t.make_dir("dest");
        t.make_file("dest/a.txt", "in the way");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items: into_dir(vec![present], &to_dir),
                policy: OnCollision::Refuse,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].summary.collided(), 1);
        assert_eq!(finished[0].collided.len(), 1);
        assert_eq!(finished[0].collided[0].to, t.at("dest/a.txt"));
        assert!(finished[0].failed.is_empty());
        assert_eq!(
            fs::read_to_string(t.at("dest/a.txt")).unwrap(),
            "in the way"
        );
    }

    /// The rule a single `execute` cannot keep on its own: two sources
    /// flattened onto one name are two jobs of the walk, and the second sees a
    /// live disk in which the first has already landed.
    #[test]
    fn a_batch_that_collides_with_itself_does_not_overwrite_its_own_output() {
        let t = TempTree::new();
        let first = t.make_file("src/x.txt", "first");
        let second = t.make_file("src/sub/x.txt", "second");
        let to_dir = t.make_dir("dest");

        let mut worker = Worker::start();
        worker
            .queue(Work::Transfer {
                op: TransferOp::Copy,
                items: into_dir(vec![first, second], &to_dir),
                policy: OnCollision::Overwrite,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(
            fs::read_to_string(t.at("dest/x.txt")).unwrap(),
            "first",
            "the second item overwrote what the batch had just written"
        );
        assert_eq!(finished[0].collided.len(), 1);
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
                items: into_dir(items, &to_dir),
                policy: OnCollision::Refuse,
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
                items: into_dir(items, &to_dir),
                policy: OnCollision::Refuse,
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
                mode: DeleteMode::Permanent,
            })
            .unwrap();
        settle(&mut worker);

        // The flag is set while nothing is running; the worker drops it before
        // it picks the next job up.
        worker.cancel();
        worker
            .queue(Work::Delete {
                items: vec![t.make_file("b.txt", "b")],
                mode: DeleteMode::Permanent,
            })
            .unwrap();
        let finished = settle(&mut worker);

        assert_eq!(finished[0].summary.processed(), 1);
        assert!(!t.at("b.txt").exists());
    }
}
