use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// How a batch of filesystem operations ended. `total` is the size of the
/// batch, fixed when it starts, so a run that stops early is still reported
/// against what it set out to do. A batch never fails as a whole: an item that
/// cannot be handled is counted, and the run carries on to the next one.
#[derive(Debug)]
pub struct ProcessedSummary {
    processed: usize,
    skipped: usize,
    failed: usize,
    collided: usize,
    total: usize,
}

impl ProcessedSummary {
    pub fn new(total: usize) -> Self {
        Self {
            processed: 0,
            skipped: 0,
            failed: 0,
            collided: 0,
            total,
        }
    }

    pub fn process(&mut self) {
        self.processed += 1;
    }

    pub fn skip(&mut self) {
        self.skipped += 1;
    }

    pub fn fail(&mut self) {
        self.failed += 1;
    }

    pub fn collide(&mut self) {
        self.collided += 1;
    }

    pub fn processed(&self) -> usize {
        self.processed
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    pub fn collided(&self) -> usize {
        self.collided
    }

    pub fn total(&self) -> usize {
        self.total
    }

    /// Whether nothing went through, either because nothing was marked or
    /// because every item was skipped or failed.
    pub fn nothing_processed(&self) -> bool {
        self.processed == 0
    }

    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }

    /// Whether the disk may have changed. A failed item counts, and so does a
    /// collided one: either can give up part way through a tree and still
    /// leave something behind.
    pub fn touched_disk(&self) -> bool {
        self.processed > 0 || self.failed > 0 || self.collided > 0
    }
}

/// Watches a transfer while it runs. The transfer reports every entry it
/// writes and asks before each one whether it should carry on, which is the
/// only place a running transfer can be stopped: a single `fs::copy` is one
/// syscall and cannot be interrupted from outside.
pub trait Observer {
    /// Called once per file or symlink written, with the bytes it contributed.
    fn entry_copied(&mut self, path: &Path, bytes: u64);

    /// Checked before every entry. `true` aborts the transfer, which then
    /// removes what it has already written.
    fn cancelled(&self) -> bool;
}

/// Sums the bytes a transfer will have to write, walking directories without
/// following symlinks. An entry that cannot be read contributes nothing
/// instead of failing the walk: the total only feeds a progress bar, and the
/// transfer itself will report the same entry as failed soon enough.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = path.symlink_metadata() else {
        return 0;
    };

    if !metadata.file_type().is_dir() {
        return metadata.len();
    }

    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| tree_size(&entry.path()))
        .sum()
}

/// What a transfer does with an item whose destination is already taken.
/// `Refuse` fails the item and is what a transfer did before there was
/// anything to choose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnCollision {
    #[default]
    Refuse,
    Overwrite,
    Skip,
    KeepBoth,
}

/// One item of a transfer: what moves, and the name it lands under.
///
/// Both sides are named outright rather than a destination directory being
/// joined on later, because a batch flattens — two sources can want one name —
/// and the answer to that has to come back to the pairs it was about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// The answer a job carries into every collision it meets, and the
/// destinations that answer may be applied to.
///
/// Only what was already standing when the job started may be overwritten.
/// Anything that appeared since is the job's own output, and overwriting that
/// would leave one file where two were asked for, with no telling which. Such
/// a pair comes back as a collision instead, and the job answering it takes
/// its own snapshot, in which the first file is one that was already there.
#[derive(Debug)]
pub struct Policy {
    on_collision: OnCollision,
    preexisting: HashSet<PathBuf>,
}

impl Policy {
    /// Reads what is already in the way of `items`, before anything is
    /// written.
    pub fn new(on_collision: OnCollision, items: &[Transfer]) -> Self {
        let mut preexisting = HashSet::new();
        for item in items {
            record_preexisting(&item.from, &item.to, &mut preexisting);
        }

        Self {
            on_collision,
            preexisting,
        }
    }

    /// What to do about the destination `to`.
    fn on(&self, to: &Path) -> OnCollision {
        match self.on_collision {
            OnCollision::Overwrite if !self.preexisting.contains(to) => OnCollision::Refuse,
            answer => answer,
        }
    }
}

/// What one item's transfer left behind. Nothing skipped and nothing collided
/// means the whole tree went through.
#[derive(Debug, Default)]
pub struct Unfinished {
    /// Leaves stepped over, because the answer was to skip them.
    pub skipped: usize,
    /// Leaves whose destination was taken and which this job had no answer
    /// for. They come back so the question can be put once, and carried out
    /// by a job that names them outright.
    pub collided: Vec<Transfer>,
    /// Leaves that landed under a numbered name of their own. Nobody would
    /// find them otherwise: the name they went in under is not the one that
    /// was asked for.
    pub kept: Vec<PathBuf>,
}

impl Unfinished {
    /// Whether the item went through whole and under the names asked for.
    pub fn nothing_left(&self) -> bool {
        self.skipped == 0 && self.collided.is_empty() && self.kept.is_empty()
    }
}

/// Notes every destination of `from` under `to` that is already there.
///
/// The walk stops at the first name the destination does not hold: nothing
/// below a free name can be there either. What cannot be read is left out, and
/// the write that follows reports it.
fn record_preexisting(from: &Path, to: &Path, found: &mut HashSet<PathBuf>) {
    let Ok(destination) = to.symlink_metadata() else {
        return;
    };
    found.insert(to.to_path_buf());

    let Ok(source) = from.symlink_metadata() else {
        return;
    };
    if !(source.is_dir() && destination.is_dir()) {
        return;
    }

    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        record_preexisting(&entry.path(), &to.join(entry.file_name()), found);
    }
}

/// What the destination holds, against the source standing at the same name.
///
/// Both sides are read with `symlink_metadata`, so a symlink in the
/// destination is something in the way rather than a directory to descend
/// into: descending would write outside the tree it points at.
enum Pairing {
    /// Nothing is there, so the whole source goes in as it stands.
    Free,
    /// A directory on a directory. The two merge, name by name, a level down.
    Merge,
    /// Anything else standing where the source wants to be.
    Collision,
}

#[derive(Clone, Copy, Debug)]
pub enum TransferOp {
    Copy,
    Move,
}

impl TransferOp {
    /// Carries out one item of a batch, and reports what it could not finish.
    pub fn execute(
        &self,
        item: &Transfer,
        policy: &Policy,
        watcher: &mut dyn Observer,
    ) -> io::Result<Unfinished> {
        ensure_destination_outside_source(&item.from, &item.to)?;

        let mut unfinished = Unfinished::default();
        self.transfer(&item.from, &item.to, policy, watcher, &mut unfinished)?;
        Ok(unfinished)
    }

    /// Walks the two trees as pairs of names. At every name there is exactly
    /// one pair, and only a pair with something already in the destination
    /// reaches `policy`.
    fn transfer(
        &self,
        from: &Path,
        to: &Path,
        policy: &Policy,
        watcher: &mut dyn Observer,
        unfinished: &mut Unfinished,
    ) -> io::Result<()> {
        match pairing(from, to)? {
            Pairing::Free => self.place(from, to, watcher),
            Pairing::Merge => {
                for entry in fs::read_dir(from)? {
                    let entry = entry?;
                    self.transfer(
                        &entry.path(),
                        &to.join(entry.file_name()),
                        policy,
                        watcher,
                        unfinished,
                    )?;
                }
                self.drop_emptied(from)
            }
            Pairing::Collision => self.resolve(from, to, policy, watcher, unfinished),
        }
    }

    /// Puts the whole source at a destination with nothing in the way.
    fn place(&self, from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
        match self {
            TransferOp::Copy => {
                copy_recursive(from, to, watcher).inspect_err(|_| remove_partial(to))
            }
            TransferOp::Move => match fs::rename(from, to) {
                // rename(2) cannot cross filesystems. Fall back to a full copy,
                // and only delete the source once that copy has fully succeeded.
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                    copy_recursive(from, to, watcher).inspect_err(|_| remove_partial(to))?;
                    remove_recursive(from)
                }
                other => other,
            },
        }
    }

    /// Answers a collision the way `policy` says. An answer this job does not
    /// have is not a failure: the pair is written down and comes back for the
    /// question to be put.
    fn resolve(
        &self,
        from: &Path,
        to: &Path,
        policy: &Policy,
        watcher: &mut dyn Observer,
        unfinished: &mut Unfinished,
    ) -> io::Result<()> {
        match policy.on(to) {
            OnCollision::Refuse => {
                unfinished.collided.push(Transfer {
                    from: from.to_path_buf(),
                    to: to.to_path_buf(),
                });
                Ok(())
            }
            OnCollision::Skip => {
                unfinished.skipped += 1;
                Ok(())
            }
            OnCollision::Overwrite => self.overwrite(from, to, watcher),
            OnCollision::KeepBoth => {
                let kept = self.keep_both(from, to, watcher)?;
                unfinished.kept.push(kept);
                Ok(())
            }
        }
    }

    /// Replaces what stands at `to`, which a directory on either side never
    /// does: `remove_recursive` behind an "overwrite" would take the files
    /// that were only ever in the destination, and the answer was about a file
    /// standing where a file stands.
    fn overwrite(&self, from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
        if from.symlink_metadata()?.is_dir() || to.symlink_metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} and {} are not the same kind of thing",
                    from.display(),
                    to.display()
                ),
            ));
        }

        self.replace(from, to, watcher)
    }

    /// Writes beside `to` and renames over it, so a copy that dies half way
    /// leaves what was there. `remove_partial` cannot stand in here the way it
    /// does for a fresh destination: what it would delete is not ours.
    fn replace(&self, from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
        // A move within one filesystem is already an atomic replacement:
        // rename(2) leaves either the new file or the old one, never half of
        // either, so staging would only add a second moment to die in.
        if let TransferOp::Move = self {
            match fs::rename(from, to) {
                Err(e) if e.kind() == io::ErrorKind::CrossesDevices => (),
                other => return other,
            }
        }

        let staged = staging_path(to);
        self.place(from, &staged, watcher)?;
        fs::rename(&staged, to).inspect_err(|_| remove_partial(&staged))
    }

    /// Puts the source at the first free numbered name beside `to`, and gives
    /// back the name it took.
    ///
    /// Claiming creates the name, so what is left to do is fill what was just
    /// created: an empty directory takes the source's children, and an empty
    /// placeholder is written over. Nothing inside can collide, which is why
    /// no policy travels in here.
    fn keep_both(&self, from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<PathBuf> {
        let claim = Claim::for_source(from)?;
        let free = claim_free_name(to, claim)?;

        match claim {
            Claim::Directory => self.fill(from, &free, watcher),
            Claim::File => self.replace(from, &free, watcher),
        }?;
        Ok(free)
    }

    /// Puts every child of `from` into `to`, which has just been created
    /// empty, so every name inside it is free.
    fn fill(&self, from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            self.place(&entry.path(), &to.join(entry.file_name()), watcher)?;
        }
        self.drop_emptied(from)
    }

    /// Takes away a source directory a move has just emptied. A copy leaves it
    /// alone, and so does a move that skipped one of its children: a directory
    /// still holding something is not one the move is finished with.
    fn drop_emptied(&self, from: &Path) -> io::Result<()> {
        let TransferOp::Move = self else {
            return Ok(());
        };

        match fs::remove_dir(from) {
            Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
            other => other,
        }
    }
}

/// Reads both sides of one name. A destination that cannot be read at all is
/// taken as free, and whatever stopped it being read comes back from the
/// write that follows.
fn pairing(from: &Path, to: &Path) -> io::Result<Pairing> {
    let Ok(destination) = to.symlink_metadata() else {
        return Ok(Pairing::Free);
    };

    Ok(
        if from.symlink_metadata()?.is_dir() && destination.is_dir() {
            Pairing::Merge
        } else {
            Pairing::Collision
        },
    )
}

#[derive(Debug)]
pub enum MutationOp {
    Delete { path: PathBuf },
    Rename { path: PathBuf, new_name: String },
    CreateDir { parent: PathBuf, name: String },
    CreateFile { parent: PathBuf, name: String },
}

impl MutationOp {
    pub fn execute(&self) -> io::Result<()> {
        match self {
            MutationOp::Delete { path } => remove_recursive(path),
            MutationOp::Rename { path, new_name } => {
                fs::rename(path, path.with_file_name(new_name))
            }
            MutationOp::CreateDir { parent, name } => {
                let path = parent.join(name);
                if path.symlink_metadata().is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} already exists", path.display()),
                    ));
                }
                fs::create_dir_all(path)
            }
            MutationOp::CreateFile { parent, name } => {
                let path = parent.join(name);
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                fs::File::create_new(path).map(|_| ())
            }
        }
    }
}

fn copy_recursive(from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
    if watcher.cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "transfer cancelled",
        ));
    }

    // symlink_metadata does not follow links, which is what stops a symlink
    // loop from being walked into.
    let file_type = from.symlink_metadata()?.file_type();

    if file_type.is_symlink() {
        copy_symlink(from, to)?;
        // A recreated link writes only its own target string, which the
        // pre-walk did not count either.
        watcher.entry_copied(from, 0);
        Ok(())
    } else if file_type.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()), watcher)?;
        }
        Ok(())
    } else {
        let bytes = fs::copy(from, to)?;
        watcher.entry_copied(from, bytes);
        Ok(())
    }
}

/// Recreates a symlink at `to` pointing wherever `from` pointed, rather than
/// materialising a copy of whatever it resolved to.
#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(not(unix))]
fn copy_symlink(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copying symlinks is only implemented on unix",
    ))
}

/// How many numbered names are tried before a `KeepBoth` gives up, so a
/// directory already holding every one of them ends the search rather than
/// counting forever.
const KEEP_BOTH_LIMIT: u32 = 1000;

/// What a claimed name is created as, so the name and what goes into it are
/// the same kind of thing. A symlink claims a file: the link is written over
/// the placeholder, and only a directory has to be claimed as one, so its
/// children have somewhere to land.
#[derive(Clone, Copy, Debug)]
enum Claim {
    File,
    Directory,
}

impl Claim {
    fn for_source(from: &Path) -> io::Result<Self> {
        Ok(if from.symlink_metadata()?.is_dir() {
            Claim::Directory
        } else {
            Claim::File
        })
    }

    /// Creates `path`, failing with `AlreadyExists` when something is already
    /// there. Both calls create or fail; neither opens what it finds.
    fn take(self, path: &Path) -> io::Result<()> {
        match self {
            Claim::File => fs::File::create_new(path).map(|_| ()),
            Claim::Directory => fs::create_dir(path),
        }
    }
}

/// Takes the first free numbered name beside `to` by creating it, which asks
/// whether the name is free and takes it in one syscall. Asking first and
/// creating after leaves a gap for another writer to step into.
///
/// The number goes where `Path::extension` splits the name, which is at the
/// last dot: `notes.tar.gz` becomes `notes.tar(1).gz`, and `.bashrc`, whose
/// only dot starts it, becomes `.bashrc(1)`.
fn claim_free_name(to: &Path, claim: Claim) -> io::Result<PathBuf> {
    let name = to
        .file_name()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} has no file name to number", to.display()),
            )
        })?
        .to_string_lossy()
        .into_owned();
    let at = insertion_point(&name);

    for n in 1..=KEEP_BOTH_LIMIT {
        let mut candidate = name.clone();
        candidate.insert_str(at, &format!("({n})"));
        let path = to.with_file_name(candidate);

        match claim.take(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "{} and the {KEEP_BOTH_LIMIT} names after it are taken",
            to.display()
        ),
    ))
}

/// Where a number is inserted into `name`: before the last dot, or at the end
/// when the name has no dot or is one of the dotfiles whose only dot starts
/// it.
///
/// The number goes into the name itself rather than being assembled from a
/// stem and an extension. Assembling has to write the dot back, and a name
/// that never had an extension — `.bashrc`, `archive.` — comes out of that
/// with one more dot than it started with.
fn insertion_point(name: &str) -> usize {
    match name.rfind('.') {
        Some(0) | None => name.len(),
        Some(dot) => dot,
    }
}

/// A name beside `to` for something still being written. Unique per process
/// and per call, so two transfers staging into one directory cannot meet.
///
/// What is left behind by a run that died is not stepped around: the transfer
/// that finds it fails and says so, which is the whole point of writing here
/// rather than over the destination.
fn staging_path(to: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    to.with_file_name(format!(
        ".mula-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn remove_partial(to: &Path) {
    if to.symlink_metadata().is_err() {
        return;
    }
    if let Err(e) = remove_recursive(to) {
        tracing::error!(path = ?to, error = %e, "could not clean up partial transfer");
    }
}

fn remove_recursive(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Rejects copying a directory into its own subtree, which would otherwise
/// recurse into what it is writing until the path length limit is hit.
fn ensure_destination_outside_source(from: &Path, to: &Path) -> io::Result<()> {
    let invalid = |msg: String| io::Error::new(io::ErrorKind::InvalidInput, msg);

    let parent = to
        .parent()
        .ok_or_else(|| invalid(format!("{} has no parent directory", to.display())))?;
    let file_name = to
        .file_name()
        .ok_or_else(|| invalid(format!("{} has no file name", to.display())))?;

    // `to` does not exist yet, so only its parent can be canonicalised.
    // Resolving both sides is what stops the check being sidestepped via a
    // symlink or a `..` component.
    let from = from.canonicalize()?;
    let to = parent.canonicalize()?.join(file_name);

    if to.starts_with(&from) {
        return Err(invalid(format!(
            "cannot transfer {} into itself ({})",
            from.display(),
            to.display()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod ops_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Records what a transfer reports and answers `cancelled` from a switch
    /// the test flips, so both halves of the trait can be driven on their own.
    #[derive(Default)]
    struct Watcher {
        entries: Vec<(PathBuf, u64)>,
        cancel_after: Option<usize>,
    }

    impl Watcher {
        fn bytes(&self) -> u64 {
            self.entries.iter().map(|(_, bytes)| bytes).sum()
        }
    }

    impl Observer for Watcher {
        fn entry_copied(&mut self, path: &Path, bytes: u64) {
            self.entries.push((path.to_path_buf(), bytes));
        }

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|after| self.entries.len() >= after)
        }
    }

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "mula-ops-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn at(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }

        fn root(&self) -> PathBuf {
            self.0.clone()
        }

        fn make_dir(&self, path: &Path) {
            fs::create_dir_all(path).unwrap();
        }

        fn make_file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.at(relative);
            self.make_dir(path.parent().unwrap());
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copies_a_directory_tree_preserving_structure() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");
        t.make_file("src/sub/b.txt", "b");
        t.make_file("src/sub/deep/c.txt", "c");

        transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(t.at("dest/sub/b.txt")).unwrap(), "b");
        assert_eq!(
            fs::read_to_string(t.at("dest/sub/deep/c.txt")).unwrap(),
            "c"
        );
        assert!(t.at("src/a.txt").exists(), "source must be left alone");
    }

    /// With no answer to give, a taken destination is reported rather than
    /// written over or failed: nothing went wrong, there is only a question
    /// nobody has put yet.
    #[test]
    fn a_taken_destination_comes_back_as_a_collision_rather_than_an_error() {
        let t = TempTree::new();
        t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");

        let left = transfer(
            TransferOp::Copy,
            &t.at("src.txt"),
            &t.at("dest.txt"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert_eq!(collided_names(&left), ["dest.txt"]);
        assert_eq!(
            fs::read_to_string(t.at("dest.txt")).unwrap(),
            "original",
            "existing file must not be clobbered"
        );
    }

    /// Runs one transfer with a watcher nobody looks at, and gives back what
    /// it could not finish. The snapshot is taken over the one item, the way a
    /// batch of one would.
    fn transfer(
        op: TransferOp,
        from: &Path,
        to: &Path,
        policy: OnCollision,
    ) -> io::Result<Unfinished> {
        run(op, from, to, policy, &mut Watcher::default())
    }

    /// The same, with a watcher the test goes on to read.
    fn run(
        op: TransferOp,
        from: &Path,
        to: &Path,
        policy: OnCollision,
        watcher: &mut Watcher,
    ) -> io::Result<Unfinished> {
        let item = Transfer {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        };
        let policy = Policy::new(policy, std::slice::from_ref(&item));
        op.execute(&item, &policy, watcher)
    }

    /// The names a collision report points at, for asserting on without
    /// spelling out a temporary directory.
    fn collided_names(unfinished: &Unfinished) -> Vec<String> {
        unfinished
            .collided
            .iter()
            .map(|item| crate::ui::name_of(&item.to))
            .collect()
    }

    #[test]
    fn overwrites_an_existing_file_when_that_is_the_answer() {
        let t = TempTree::new();
        t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");

        transfer(
            TransferOp::Copy,
            &t.at("src.txt"),
            &t.at("dest.txt"),
            OnCollision::Overwrite,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest.txt")).unwrap(), "new");
    }

    #[test]
    fn skipping_leaves_both_sides_as_they_were() {
        let t = TempTree::new();
        t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");

        transfer(
            TransferOp::Copy,
            &t.at("src.txt"),
            &t.at("dest.txt"),
            OnCollision::Skip,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest.txt")).unwrap(), "original");
        assert_eq!(fs::read_to_string(t.at("src.txt")).unwrap(), "new");
    }

    /// The number goes where `Path::extension` splits the name. A name with no
    /// extension to split off must not come back with a dot it never had.
    #[test]
    fn keeping_both_numbers_the_name_where_its_extension_starts() {
        let t = TempTree::new();

        for (name, kept) in [
            ("notes.tar.gz", "notes.tar(1).gz"),
            (".bashrc", ".bashrc(1)"),
            ("archive.", "archive(1)."),
            ("plain", "plain(1)"),
        ] {
            t.make_file(&format!("src/{name}"), "new");
            t.make_file(&format!("dest/{name}"), "original");

            transfer(
                TransferOp::Copy,
                &t.at(&format!("src/{name}")),
                &t.at(&format!("dest/{name}")),
                OnCollision::KeepBoth,
            )
            .unwrap();

            assert_eq!(
                fs::read_to_string(t.at(&format!("dest/{kept}"))).unwrap(),
                "new",
                "{name} should have landed as {kept}"
            );
            assert_eq!(
                fs::read_to_string(t.at(&format!("dest/{name}"))).unwrap(),
                "original",
                "{name} must still be the original"
            );
        }
    }

    #[test]
    fn keeping_both_counts_past_the_numbers_already_taken() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "new");
        t.make_file("dest/a.txt", "original");
        t.make_file("dest/a(1).txt", "first");

        transfer(
            TransferOp::Copy,
            &t.at("src/a.txt"),
            &t.at("dest/a.txt"),
            OnCollision::KeepBoth,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest/a(2).txt")).unwrap(), "new");
        assert_eq!(fs::read_to_string(t.at("dest/a(1).txt")).unwrap(), "first");
    }

    /// A directory reaches `KeepBoth` only against something that is not a
    /// directory — two directories merge and never collide — and the number it
    /// takes has to be claimed as a directory for its tree to land in.
    #[test]
    fn keeping_both_of_a_directory_numbers_it_and_copies_the_whole_tree() {
        let t = TempTree::new();
        t.make_file("src/one/a.txt", "a");
        t.make_file("src/one/sub/b.txt", "b");
        t.make_file("dest/one", "i am a file");

        transfer(
            TransferOp::Copy,
            &t.at("src/one"),
            &t.at("dest/one"),
            OnCollision::KeepBoth,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest/one(1)/a.txt")).unwrap(), "a");
        assert_eq!(
            fs::read_to_string(t.at("dest/one(1)/sub/b.txt")).unwrap(),
            "b"
        );
        assert_eq!(
            fs::read_to_string(t.at("dest/one")).unwrap(),
            "i am a file",
            "what was there must be left alone"
        );
    }

    /// Two directories of the same name whose leaves do not clash merge in
    /// silence — under `Refuse`, which is the policy that asks.
    #[test]
    fn a_directory_merges_into_one_already_there_and_its_own_files_survive() {
        let t = TempTree::new();
        t.make_file("src/new.txt", "new");
        t.make_file("src/sub/deep.txt", "deep");
        t.make_file("dest/theirs.txt", "theirs");
        t.make_file("dest/sub/kept.txt", "kept");

        transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest/new.txt")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(t.at("dest/sub/deep.txt")).unwrap(),
            "deep"
        );
        assert_eq!(
            fs::read_to_string(t.at("dest/theirs.txt")).unwrap(),
            "theirs",
            "what was only in the destination must survive a merge"
        );
        assert_eq!(
            fs::read_to_string(t.at("dest/sub/kept.txt")).unwrap(),
            "kept"
        );
    }

    /// `remove_recursive` behind "overwrite" would take the files that were
    /// only ever in the destination, which the user was never shown.
    #[test]
    fn a_directory_never_replaces_a_file_even_under_overwrite() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");
        t.make_file("dest", "i am a file");

        let err = transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Overwrite,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(t.at("dest")).unwrap(), "i am a file");
    }

    #[test]
    fn a_file_never_replaces_a_directory_even_under_overwrite() {
        let t = TempTree::new();
        t.make_file("src", "i am a file");
        t.make_file("dest/kept.txt", "kept");

        let err = transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Overwrite,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_to_string(t.at("dest/kept.txt")).unwrap(), "kept");
    }

    /// Descending into a symlinked directory writes outside the tree it was
    /// pointed at, so a link in the destination is something in the way.
    #[cfg(unix)]
    #[test]
    fn a_symlink_in_the_destination_is_a_collision_rather_than_a_descent() {
        let t = TempTree::new();
        t.make_file("src/sub/a.txt", "a");
        t.make_file("elsewhere/untouched.txt", "untouched");
        t.make_dir(&t.at("dest"));
        std::os::unix::fs::symlink(t.at("elsewhere"), t.at("dest/sub")).unwrap();

        let left = transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert_eq!(collided_names(&left), ["sub"]);
        assert!(
            !t.at("elsewhere/a.txt").exists(),
            "the copy must not have been written through the link"
        );
        assert_eq!(
            fs::read_to_string(t.at("elsewhere/untouched.txt")).unwrap(),
            "untouched"
        );
    }

    /// The original is the user's own file, so a copy that dies part way
    /// through must not have been writing over it.
    #[cfg(unix)]
    #[test]
    fn a_failed_overwrite_leaves_the_original_where_it_was() {
        use std::os::unix::fs::PermissionsExt;

        let t = TempTree::new();
        let source = t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");
        fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();

        let err = transfer(
            TransferOp::Copy,
            &t.at("src.txt"),
            &t.at("dest.txt"),
            OnCollision::Overwrite,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(t.at("dest.txt")).unwrap(), "original");

        let leftovers: Vec<_> = fs::read_dir(t.root())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".mula-"))
            .collect();
        assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
    }

    #[test]
    fn a_move_that_merges_takes_the_emptied_source_with_it() {
        let t = TempTree::new();
        t.make_file("src/new.txt", "new");
        t.make_file("dest/theirs.txt", "theirs");

        transfer(
            TransferOp::Move,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert!(!t.at("src").exists(), "an emptied source must not be left");
        assert_eq!(fs::read_to_string(t.at("dest/new.txt")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(t.at("dest/theirs.txt")).unwrap(),
            "theirs"
        );
    }

    /// A move that skipped something left it in the source, and the directory
    /// holding it has to stay too.
    #[test]
    fn a_move_that_skipped_a_file_keeps_the_directory_holding_it() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "new");
        t.make_file("src/b.txt", "moved");
        t.make_file("dest/a.txt", "original");

        transfer(
            TransferOp::Move,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Skip,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("src/a.txt")).unwrap(), "new");
        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "original");
        assert_eq!(fs::read_to_string(t.at("dest/b.txt")).unwrap(), "moved");
        assert!(!t.at("src/b.txt").exists());
    }

    #[test]
    fn a_move_replaces_an_existing_file_when_that_is_the_answer() {
        let t = TempTree::new();
        t.make_file("src.txt", "new");
        t.make_file("dest.txt", "original");

        transfer(
            TransferOp::Move,
            &t.at("src.txt"),
            &t.at("dest.txt"),
            OnCollision::Overwrite,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(t.at("dest.txt")).unwrap(), "new");
        assert!(!t.at("src.txt").exists());
    }

    #[test]
    fn refuses_to_copy_a_directory_into_its_own_subtree() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");

        let err = transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("src/nested"),
            OnCollision::Refuse,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!t.at("src/nested").exists());
    }

    #[test]
    fn moves_a_tree_within_one_filesystem() {
        let t = TempTree::new();
        t.make_file("src/sub/a.txt", "a");

        transfer(
            TransferOp::Move,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert!(!t.at("src").exists(), "source must be gone after a move");
        assert_eq!(fs::read_to_string(t.at("dest/sub/a.txt")).unwrap(), "a");
    }

    #[cfg(unix)]
    #[test]
    fn recreates_symlinks_instead_of_following_them() {
        let t = TempTree::new();
        t.make_file("src/real.txt", "real");
        std::os::unix::fs::symlink("real.txt", t.at("src/link.txt")).unwrap();

        transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        let copied = t.at("dest/link.txt");
        assert!(
            copied.symlink_metadata().unwrap().file_type().is_symlink(),
            "link must stay a link, not become a copy of its target"
        );
        assert_eq!(fs::read_link(&copied).unwrap(), Path::new("real.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn does_not_walk_into_a_symlink_loop() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "a");
        // Points back above `src`, so following it would recurse forever.
        std::os::unix::fs::symlink("..", t.at("src/loop")).unwrap();

        transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap();

        assert!(
            t.at("dest/loop")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(t.at("dest/a.txt")).unwrap(), "a");
    }

    #[cfg(unix)]
    #[test]
    fn removes_the_partial_tree_when_a_copy_fails_midway() {
        use std::os::unix::fs::PermissionsExt;

        let t = TempTree::new();
        t.make_file("src/ok.txt", "ok");
        let blocked = t.make_file("src/blocked.txt", "secret");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let err = transfer(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            !t.at("dest").exists(),
            "a failed copy must not leave a partial tree behind"
        );
        assert!(t.at("src/ok.txt").exists(), "source must be untouched");
    }

    #[test]
    fn deletes_file() {
        let t = TempTree::new();
        t.make_file("src/file.txt", "hello world");

        MutationOp::Delete {
            path: t.at("src/file.txt"),
        }
        .execute()
        .unwrap();

        assert!(
            !t.at("src/file.txt").exists(),
            "deleted file must not exists"
        );
    }

    #[test]
    fn deletes_dir() {
        let t = TempTree::new();
        t.make_dir(&t.at("src"));

        MutationOp::Delete { path: t.at("src") }.execute().unwrap();

        assert!(!t.at("src").exists(), "deleted directory must not exists");
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_to_a_file_leaves_its_target_intact() {
        let t = TempTree::new();
        let target = t.make_file("src/target.txt", "precious");
        let link = t.at("src/link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "precious",
            "the target must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_to_a_directory_leaves_its_contents_intact() {
        let t = TempTree::new();
        t.make_file("real/precious.txt", "precious");
        let link = t.at("link");
        std::os::unix::fs::symlink(t.at("real"), &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
        assert_eq!(
            fs::read_to_string(t.at("real/precious.txt")).unwrap(),
            "precious",
            "the directory the link pointed at must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn deletes_dangling_symlink() {
        let t = TempTree::new();
        let link = t.at("link");
        std::os::unix::fs::symlink(t.at("real"), &link).unwrap();

        MutationOp::Delete { path: link.clone() }.execute().unwrap();

        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be gone"
        );
    }

    #[test]
    fn deleting_non_existing_path_should_fail() {
        let t = TempTree::new();
        let err = MutationOp::Delete {
            path: t.at("non_existing"),
        }
        .execute()
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn creates_a_directory_and_any_missing_intermediate_directories() {
        let t = TempTree::new();

        MutationOp::CreateDir {
            parent: t.root(),
            name: "fol/fol2/fol3".to_string(),
        }
        .execute()
        .unwrap();

        assert!(t.at("fol/fol2/fol3").is_dir());
    }

    #[test]
    fn creates_a_directory_through_existing_intermediate_directories() {
        let t = TempTree::new();
        t.make_dir(&t.at("fol/fol2"));

        MutationOp::CreateDir {
            parent: t.root(),
            name: "fol/fol2/fol3".to_string(),
        }
        .execute()
        .unwrap();

        assert!(t.at("fol/fol2/fol3").is_dir());
    }

    #[test]
    fn refuses_to_recreate_an_existing_directory() {
        let t = TempTree::new();
        t.make_dir(&t.at("fol"));

        let err = MutationOp::CreateDir {
            parent: t.root(),
            name: "fol".to_string(),
        }
        .execute()
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn creates_a_file_and_any_missing_parent_directories() {
        let t = TempTree::new();

        MutationOp::CreateFile {
            parent: t.root(),
            name: "fol/fol2/fol3/file.txt".to_string(),
        }
        .execute()
        .unwrap();

        assert!(t.at("fol/fol2/fol3/file.txt").is_file());
    }

    #[test]
    fn creates_a_file_through_existing_intermediate_directories() {
        let t = TempTree::new();
        t.make_dir(&t.at("fol/fol2"));

        MutationOp::CreateFile {
            parent: t.root(),
            name: "fol/fol2/fol3/file.txt".to_string(),
        }
        .execute()
        .unwrap();

        assert!(t.at("fol/fol2/fol3/file.txt").is_file());
    }

    #[test]
    fn refuses_to_overwrite_an_existing_file() {
        let t = TempTree::new();
        t.make_file("file.txt", "original");

        let err = MutationOp::CreateFile {
            parent: t.root(),
            name: "file.txt".to_string(),
        }
        .execute()
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(t.at("file.txt")).unwrap(),
            "original",
            "existing file must not be clobbered"
        );
    }

    #[test]
    fn tree_size_adds_up_every_file_below_a_directory() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/sub/b.txt", "bb");

        assert_eq!(tree_size(&t.at("src")), 6);
    }

    #[test]
    fn tree_size_of_something_that_is_not_there_is_zero() {
        let t = TempTree::new();

        assert_eq!(tree_size(&t.at("missing")), 0);
    }

    #[test]
    fn a_copy_reports_every_file_it_writes() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/sub/b.txt", "bb");

        let mut watcher = Watcher::default();
        run(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
            &mut watcher,
        )
        .unwrap();

        assert_eq!(watcher.entries.len(), 2, "{:?}", watcher.entries);
        // What the observer counted has to match what the pre-walk promised, or
        // the bar would never reach its own end.
        assert_eq!(watcher.bytes(), tree_size(&t.at("src")));
    }

    #[test]
    fn a_cancelled_copy_leaves_nothing_behind() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/b.txt", "bbbb");
        t.make_file("src/c.txt", "cccc");

        let mut watcher = Watcher {
            cancel_after: Some(1),
            ..Watcher::default()
        };
        let err = run(
            TransferOp::Copy,
            &t.at("src"),
            &t.at("dest"),
            OnCollision::Refuse,
            &mut watcher,
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(
            t.at("dest").symlink_metadata().is_err(),
            "the partial copy must be cleaned up"
        );
        assert!(t.at("src/a.txt").exists(), "the source must be untouched");
    }

    #[test]
    fn a_cancelled_move_keeps_the_source() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/b.txt", "bbbb");

        // A rename cannot be interrupted, so only the cross-device path can be
        // cancelled. Reaching it here would need two filesystems; what this
        // pins down is that the copy half refuses before it writes anything.
        let mut watcher = Watcher {
            cancel_after: Some(0),
            ..Watcher::default()
        };
        let err = copy_recursive(&t.at("src"), &t.at("dest"), &mut watcher).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert!(t.at("src/a.txt").exists());
    }
}
