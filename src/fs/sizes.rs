//! What the files below a directory add up to. [`measure`] walks the trees and
//! reports a total for every directory it finishes, the nested ones included;
//! [`Sizing`] runs it on a [`Reader`](crate::fs::reader::Reader), and
//! [`DirSizes`] is where the totals are kept between one listing and the next.

use std::{
    collections::HashMap,
    fs, mem,
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rayon::{ThreadPool, prelude::*};

use crate::fs::reader::{Live, Outbox, ReadJob};

/// Watches a walk while it runs, from every thread of the pool at once.
pub trait Observer: Sync {
    /// Called once per directory whose whole tree has been read, with what
    /// its files add up to.
    fn sized(&self, dir: &Path, total: u64);

    /// Checked before every directory. `true` ends the walk, and no directory
    /// left unfinished is reported.
    fn cancelled(&self) -> bool;
}

/// Measures every directory of `dirs`, spreading the trees over the rayon pool
/// it is called on.
///
/// A total is the sum of `st_size` over every file and symlink below the
/// directory: the size each of them shows in a listing, not the blocks they
/// take. A file with several hard links below the directory counts once, the
/// way `du -A` counts it. Symlinks are not followed, a directory on another
/// device is not entered, and one that cannot be read adds nothing.
pub fn measure(dirs: &[Arc<Path>], observer: &dyn Observer) {
    dirs.par_iter().for_each(|dir| {
        if let Ok(meta) = fs::symlink_metadata(dir)
            && meta.is_dir()
        {
            measure_dir(dir, meta.dev(), observer);
        }
    });
}

/// What a tree holds: the files with one link, summed, and the files with
/// several, by inode, so two subtrees holding the same file count it once when
/// they are put together.
#[derive(Default)]
struct Tally {
    single: u64,
    linked: HashMap<(u64, u64), u64>,
}

impl Tally {
    fn total(&self) -> u64 {
        self.single + self.linked.values().sum::<u64>()
    }

    /// Adds `other` in, moving the smaller set of linked files into the
    /// larger.
    fn merge(mut self, mut other: Tally) -> Tally {
        if self.linked.len() < other.linked.len() {
            mem::swap(&mut self.linked, &mut other.linked);
        }
        self.single += other.single;
        self.linked.extend(other.linked);
        self
    }
}

/// The tally of `dir`, or `None` when the walk was cancelled before the whole
/// tree was read. Subdirectories are measured in parallel.
fn measure_dir(dir: &Path, device: u64, observer: &dyn Observer) -> Option<Tally> {
    if observer.cancelled() {
        return None;
    }

    let mut tally = Tally::default();
    let mut below = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            // `DirEntry::metadata` does not follow a symlink, so a link to a
            // directory is sized as the link.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if meta.dev() == device {
                    below.push(entry.path());
                }
            } else if meta.nlink() > 1 {
                tally.linked.insert((meta.dev(), meta.ino()), meta.len());
            } else {
                tally.single += meta.len();
            }
        }
    }

    let below = below
        .par_iter()
        .map(|dir| measure_dir(dir, device, observer))
        .collect::<Option<Vec<Tally>>>()?;
    let tally = below.into_iter().fold(tally, Tally::merge);

    observer.sized(dir, tally.total());
    Some(tally)
}

/// The directories whose totals a pane wants, taken when it asked.
#[derive(Debug)]
pub struct Sizing {
    pub dirs: Vec<Arc<Path>>,
}

impl ReadJob for Sizing {
    /// Shared by every reader that sizes, so two panels measuring at once
    /// split the same threads rather than doubling them.
    type Config = Arc<ThreadPool>;
    /// Totals in the order their trees finished, nested directories included.
    type Msg = Vec<(Arc<Path>, u64)>;

    fn run(self, pool: &Self::Config, live: &Live<'_>, out: &Outbox<'_, Self::Msg>) {
        let emitter = Emitter {
            live,
            out,
            batch: Mutex::new(Batch {
                sized: Vec::new(),
                last_sent: Instant::now(),
            }),
        };
        pool.install(|| measure(&self.dirs, &emitter));
        emitter.flush();
    }
}

/// Gathers totals and sends them in batches: a walk of a deep tree finishes
/// thousands of directories, and the main loop takes them ten times a second.
struct Emitter<'a> {
    live: &'a Live<'a>,
    out: &'a Outbox<'a, Vec<(Arc<Path>, u64)>>,
    batch: Mutex<Batch>,
}

struct Batch {
    sized: Vec<(Arc<Path>, u64)>,
    last_sent: Instant,
}

impl Emitter<'_> {
    /// The longest a total waits in the batch, below the main loop's tick.
    const FLUSH_EVERY: Duration = Duration::from_millis(50);

    /// The batch length that sends without waiting for the clock.
    const FLUSH_AT: usize = 256;

    fn flush(&self) {
        let mut batch = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        send(&mut batch, self.out);
    }
}

fn send(batch: &mut Batch, out: &Outbox<'_, Vec<(Arc<Path>, u64)>>) {
    if batch.sized.is_empty() {
        return;
    }
    batch.last_sent = Instant::now();
    out.send(mem::take(&mut batch.sized));
}

impl Observer for Emitter<'_> {
    fn sized(&self, dir: &Path, total: u64) {
        let mut batch = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        batch.sized.push((Arc::from(dir), total));
        if batch.sized.len() >= Self::FLUSH_AT || batch.last_sent.elapsed() >= Self::FLUSH_EVERY {
            send(&mut batch, self.out);
        }
    }

    fn cancelled(&self) -> bool {
        self.live.cancelled()
    }
}

/// Every directory total known so far, for both panels.
#[derive(Debug, Default)]
pub struct DirSizes(HashMap<Arc<Path>, u64>);

impl DirSizes {
    pub fn get(&self, dir: &Path) -> Option<u64> {
        self.0.get(dir).copied()
    }

    pub fn insert(&mut self, dir: Arc<Path>, total: u64) {
        self.0.insert(dir, total);
    }

    /// Forgets the total of `path` and of every directory above it, the ones
    /// a change at `path` makes wrong. What is below it is left: a pane
    /// measures a directory again whenever it enters it.
    pub fn forget(&mut self, path: &Path) {
        for dir in path.ancestors() {
            self.0.remove(dir);
        }
    }
}

#[cfg(test)]
mod sizes_tests {
    use super::*;

    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    use crate::fs::temp_tree::TempTree;

    /// Keeps every total reported, and cancels from the start when told to.
    #[derive(Default)]
    struct Collected {
        sized: Mutex<HashMap<PathBuf, u64>>,
        cancelled: bool,
    }

    impl Observer for Collected {
        fn sized(&self, dir: &Path, total: u64) {
            self.sized.lock().unwrap().insert(dir.to_path_buf(), total);
        }

        fn cancelled(&self) -> bool {
            self.cancelled
        }
    }

    fn measured(tree: &TempTree, dirs: &[&str]) -> HashMap<PathBuf, u64> {
        let dirs: Vec<Arc<Path>> = dirs.iter().map(|dir| Arc::from(tree.at(dir))).collect();
        let collected = Collected::default();
        measure(&dirs, &collected);
        collected.sized.into_inner().unwrap()
    }

    #[test]
    fn a_total_adds_up_every_file_below_the_directory() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/sub/b.txt", "bb");
        t.make_file("src/sub/deeper/c.txt", "c");

        assert_eq!(measured(&t, &["src"])[&t.at("src")], 7);
    }

    /// The nested totals are what lets the pane that enters `src/sub` show
    /// its numbers without walking anything again.
    #[test]
    fn every_directory_on_the_way_is_reported_too() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/sub/b.txt", "bb");
        t.make_dir("src/empty");

        let sized = measured(&t, &["src"]);

        assert_eq!(sized[&t.at("src/sub")], 2);
        assert_eq!(sized[&t.at("src/empty")], 0);
    }

    #[test]
    fn a_symlink_counts_as_itself_and_is_not_followed() {
        let t = TempTree::new();
        t.make_file("big/data", "0123456789");
        t.make_dir("src");
        std::os::unix::fs::symlink(t.at("big"), t.at("src/to-big")).unwrap();

        let link = fs::symlink_metadata(t.at("src/to-big")).unwrap().len();

        assert_eq!(measured(&t, &["src"])[&t.at("src")], link);
    }

    /// Cargo links what it builds into two places under `target/`; counting
    /// every link would make a Rust project look a third bigger than it is.
    #[test]
    fn a_hard_linked_file_counts_once_in_every_directory_holding_its_links() {
        let t = TempTree::new();
        t.make_file("src/one/data", "0123456789");
        t.make_dir("src/two");
        fs::hard_link(t.at("src/one/data"), t.at("src/two/data")).unwrap();
        fs::hard_link(t.at("src/one/data"), t.at("src/one/again")).unwrap();

        let sized = measured(&t, &["src"]);

        assert_eq!(sized[&t.at("src")], 10);
        assert_eq!(sized[&t.at("src/one")], 10);
        assert_eq!(sized[&t.at("src/two")], 10);
    }

    #[test]
    fn a_cancelled_walk_reports_nothing() {
        let t = TempTree::new();
        t.make_file("src/sub/a.txt", "a");
        let collected = Collected {
            cancelled: true,
            ..Collected::default()
        };

        measure(&[Arc::from(t.at("src"))], &collected);

        assert!(collected.sized.into_inner().unwrap().is_empty());
    }

    /// Fails when run as root, which reads every directory whatever its mode.
    #[test]
    fn a_directory_that_cannot_be_read_adds_nothing() {
        let t = TempTree::new();
        t.make_file("src/a.txt", "aaaa");
        t.make_file("src/locked/b.txt", "bb");
        fs::set_permissions(t.at("src/locked"), fs::Permissions::from_mode(0o000)).unwrap();

        let sized = measured(&t, &["src"]);
        fs::set_permissions(t.at("src/locked"), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(sized[&t.at("src")], 4);
    }

    #[test]
    fn a_file_where_a_directory_was_asked_for_is_not_measured() {
        let t = TempTree::new();
        t.make_file("a.txt", "aaaa");

        assert!(measured(&t, &["a.txt"]).is_empty());
    }

    #[test]
    fn forgetting_a_path_forgets_what_is_above_it_and_nothing_else() {
        let mut sizes = DirSizes::default();
        for dir in ["/a", "/a/b", "/a/b/c", "/a/other"] {
            sizes.insert(Arc::from(Path::new(dir)), 1);
        }

        sizes.forget(Path::new("/a/b"));

        assert_eq!(sizes.get(Path::new("/a")), None);
        assert_eq!(sizes.get(Path::new("/a/b")), None);
        assert_eq!(sizes.get(Path::new("/a/b/c")), Some(1));
        assert_eq!(sizes.get(Path::new("/a/other")), Some(1));
    }
}
