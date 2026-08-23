//! A throwaway directory tree for tests. Anything that has to meet a real
//! filesystem — a listing, a transfer, a walk — builds one instead of reaching
//! for a fixed path, so tests can run beside each other and leave nothing
//! behind.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

/// Tells two trees of one run apart; the process id tells two runs apart.
static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A directory under the system temp directory, removed when it drops. Every
/// path it hands out is inside it.
#[derive(Debug)]
pub struct TempTree(PathBuf);

impl TempTree {
    /// An empty tree.
    pub fn new() -> Self {
        let mut root = std::env::temp_dir();
        root.push(format!(
            "mula-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("the temp directory can be written to");
        Self(root)
    }

    /// A tree of empty files, every directory on the way created with them.
    pub fn of<P: AsRef<Path>>(files: impl IntoIterator<Item = P>) -> Self {
        let tree = Self::new();
        for file in files {
            tree.make_file(file, "");
        }
        tree
    }

    /// The root of the tree.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Where `relative` sits inside the tree. Creates nothing.
    pub fn at(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    /// Writes a file, creating the directories above it.
    pub fn make_file(&self, relative: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.at(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("the tree can be written to");
        }
        fs::write(&path, contents).expect("the tree can be written to");
        path
    }

    /// Creates a directory and every directory above it.
    pub fn make_dir(&self, relative: impl AsRef<Path>) -> PathBuf {
        let path = self.at(relative);
        fs::create_dir_all(&path).expect("the tree can be written to");
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
