//! Walking a tree for names that match. Pure: no threads, no channels. What
//! runs it decides where the hits go and when to stop, the same way `ops`
//! leaves that to its [`Observer`](crate::fs::ops::Observer).

use std::{collections::VecDeque, ffi::OsString, path::Path, sync::Arc};

use crate::fs::directory::{DirEntry, DirEntryKind, Directory};

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
        // `tree_size` returns zero instead of an error.
        let Ok(listing) = Directory::read(Arc::clone(&dir)) else {
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
