//! What the main loop and the worker say to each other. Plain data with no
//! threads in sight: the queue going out, the snapshots coming back, and the
//! report at the end.

use std::path::PathBuf;

use crate::fs::{
    archive::format::Format,
    ops::{DeleteMode, MutationOp, OnCollision, ProcessedSummary, Transfer, TransferOp},
};

/// Rides along with a job and comes back on its outcome untouched. The worker
/// never looks inside; it is only how the main loop recognises which of its
/// own jobs has finished.
pub type JobTag = u64;

/// What a job set out to do, kept so the main loop can word the report without
/// remembering what it queued.
#[derive(Clone, Copy, Debug)]
pub enum JobKind {
    Transfer(TransferOp),
    Delete(DeleteMode),
    Pack,
    Unpack,
    Mutate,
}

/// A unit of work holding everything it needs. The paths are taken when the
/// job is queued and never looked up again, so nothing the user does to the
/// panels afterwards can change where it reads or writes.
#[derive(Debug)]
pub enum Work {
    Transfer {
        op: TransferOp,
        /// Both ends of every item, so a batch answering a collision names the
        /// pairs it was asked about rather than flattening them a second time.
        items: Vec<Transfer>,
        /// The one answer the whole batch carries. Asked before the batch or
        /// after it, never in flight.
        policy: OnCollision,
    },
    Delete {
        items: Vec<PathBuf>,
        /// Which deletion this is. Chosen by the key that asked and carried on
        /// the job, never decided while it runs.
        mode: DeleteMode,
    },
    /// Everything marked, into one archive. The format is settled from the
    /// name the user typed, so nothing is sniffed on this side.
    Pack {
        items: Vec<PathBuf>,
        archive: PathBuf,
        format: Format,
    },
    /// Every marked archive, each into a directory of its own under `into`.
    /// What each one is comes from its first bytes, read by the job.
    Unpack {
        items: Vec<PathBuf>,
        into: PathBuf,
    },
    Mutate(MutationOp),
}

impl Work {
    pub fn kind(&self) -> JobKind {
        match self {
            Work::Transfer { op, .. } => JobKind::Transfer(*op),
            Work::Delete { mode, .. } => JobKind::Delete(*mode),
            Work::Pack { .. } => JobKind::Pack,
            Work::Unpack { .. } => JobKind::Unpack,
            Work::Mutate(_) => JobKind::Mutate,
        }
    }

    /// The paths the job may create, change or remove. A directory above one
    /// of them changes with it; one below it is not listed.
    pub fn touched(&self) -> Vec<PathBuf> {
        match self {
            Work::Transfer { op, items, .. } => items
                .iter()
                .flat_map(|item| match op {
                    TransferOp::Copy => vec![item.to.clone()],
                    TransferOp::Move => vec![item.from.clone(), item.to.clone()],
                })
                .collect(),
            Work::Delete { items, .. } => items.clone(),
            Work::Pack { archive, .. } => vec![archive.clone()],
            Work::Unpack { into, .. } => vec![into.clone()],
            Work::Mutate(op) => match op {
                MutationOp::Delete { path, .. } => vec![path.clone()],
                MutationOp::Rename { path, new_name } => {
                    vec![path.clone(), path.with_file_name(new_name)]
                }
                MutationOp::CreateDir { parent, name }
                | MutationOp::CreateFile { parent, name } => {
                    vec![parent.join(name)]
                }
            },
        }
    }
}

/// What the bar fills with. A transfer weighs its tree before it starts, so it
/// can fill by bytes and stay even when the items differ wildly in size.
/// Deleting takes no time proportional to size, so it fills by items.
#[derive(Clone, Copy, Debug)]
pub enum Measure {
    Bytes { done: u64, total: u64 },
    Items,
}

/// A snapshot of the running job. Only the newest one is worth drawing, so the
/// main loop keeps the last of a batch and drops the rest.
#[derive(Clone, Debug)]
pub struct Progress {
    /// Items of the batch, which is what the `12 / 340` text counts. Distinct
    /// from what `measure` counts: a transfer fills its bar with bytes, and the
    /// two reach their end at different moments.
    pub items_done: usize,
    pub items_total: usize,
    pub current: String,
    pub measure: Measure,
}

impl Progress {
    /// How full the bar is, from 0.0 to 1.0. A batch that weighs nothing — a
    /// handful of empty files, or a pre-walk that could read none of them —
    /// reads as full rather than dividing by zero.
    pub fn ratio(&self) -> f64 {
        let (done, total) = match self.measure {
            Measure::Bytes { done, total } => (done, total),
            Measure::Items => (self.items_done as u64, self.items_total as u64),
        };

        if total == 0 {
            return 1.0;
        }

        // The pre-walk weighs the tree before the copy starts, so a file that
        // another process appends to in between lands heavier than promised.
        // Left unclamped that overfills the bar, which underflows the count of
        // cells still to draw.
        (done as f64 / total as f64).clamp(0.0, 1.0)
    }
}

/// How a finished job went.
#[derive(Debug)]
pub struct Outcome {
    pub tag: JobTag,
    pub kind: JobKind,
    pub summary: ProcessedSummary,
    /// The items the job could not handle, so they can be marked again and
    /// retried.
    ///
    /// A cancelled transfer or delete leaves nothing here: what it had
    /// already done stands, and marking the rest again would offer to do it
    /// twice. Packing and unpacking undo the archive they were interrupted
    /// in, so for those a cancel leaves nothing done and every item it
    /// touched comes back.
    pub failed: Vec<PathBuf>,
    /// The pairs whose destination was already taken. Not failures — nothing
    /// was tried and nothing went wrong — so they are never marked again for
    /// a retry. What they need is an answer, and a job of their own carrying
    /// it.
    pub collided: Vec<Transfer>,
    /// Where items landed that took a numbered name of their own. The name
    /// they went in under is not the one that was asked for, so it has to be
    /// said out loud.
    pub kept: Vec<PathBuf>,
    /// Parts of a job that were not carried out and are named rather than
    /// counted: an entry an archive held that could not be written, or an
    /// item an archive has no way to carry. Neither is a failure of the job
    /// it belongs to, which is why they are not in `failed`.
    pub left_out: Vec<String>,
    /// The last error, for a job whose counts alone would not say what went
    /// wrong.
    pub reason: Option<String>,
    /// What [`Work::touched`] said before the job ran, whatever came of it.
    pub touched: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum WorkerMsg {
    Progress(Progress),
    Done(Outcome),
}

#[cfg(test)]
mod job_tests {
    use super::*;

    fn weighing(done: u64, total: u64) -> Progress {
        Progress {
            items_done: 0,
            items_total: 0,
            current: String::new(),
            measure: Measure::Bytes { done, total },
        }
    }

    #[test]
    fn a_batch_that_weighs_nothing_reads_as_full() {
        assert_eq!(weighing(0, 0).ratio(), 1.0);
    }

    #[test]
    fn a_file_that_grew_since_the_pre_walk_cannot_overfill_the_bar() {
        assert_eq!(weighing(9, 4).ratio(), 1.0);
    }

    #[test]
    fn counting_items_falls_back_to_the_item_totals() {
        let progress = Progress {
            items_done: 3,
            items_total: 4,
            current: String::new(),
            measure: Measure::Items,
        };

        assert_eq!(progress.ratio(), 0.75);
    }
}
