use std::{cmp::Ordering, io, path::Path, sync::Arc};

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum DirEntryKind {
    Parent,
    Directory,
    Symlink,
    File,
}

impl DirEntryKind {
    /// Position in the listing: the parent leads, then directories, symlinks,
    /// and files last.
    fn rank(self) -> u8 {
        match self {
            DirEntryKind::Parent => 0,
            DirEntryKind::Directory => 1,
            DirEntryKind::Symlink => 2,
            DirEntryKind::File => 3,
        }
    }
}

#[derive(Debug)]
pub struct DirEntry {
    pub path: Arc<Path>,
    pub kind: DirEntryKind,
}

/// One directory listing, sorted on construction. A `Directory` is a snapshot:
/// nothing mutates it, and a newer listing means reading another one.
#[derive(Debug)]
pub struct Directory {
    path: Arc<Path>,
    entries: Vec<DirEntry>,
}

impl Directory {
    /// Reads `path`, prefixing the listing with a [`DirEntryKind::Parent`]
    /// entry whenever `path` has a parent. A directory that cannot be opened,
    /// or a single entry that cannot be read, fails the whole listing.
    ///
    /// `kind` does not follow symlinks: every symlink is reported as
    /// [`DirEntryKind::Symlink`], whatever it points at.
    pub fn read(path: Arc<Path>) -> Result<Self, io::Error> {
        let mut entries = Vec::new();

        if let Some(parent) = path.parent() {
            entries.push(DirEntry {
                path: Arc::from(parent),
                kind: DirEntryKind::Parent,
            });
        }

        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let kind = if file_type.is_symlink() {
                DirEntryKind::Symlink
            } else if file_type.is_dir() {
                DirEntryKind::Directory
            } else {
                DirEntryKind::File
            };
            entries.push(DirEntry {
                path: Arc::from(entry.path()),
                kind,
            });
        }

        Ok(Self::new(path, entries))
    }

    /// Sorts `entries` with [`compare_entries`] and stores them under `path`.
    /// The entries are taken as given: nothing checks that they live in `path`,
    /// and no `Parent` entry is added.
    pub fn new(path: Arc<Path>, mut entries: Vec<DirEntry>) -> Self {
        entries.sort_by(compare_entries);
        Self { path, entries }
    }

    pub fn path(&self) -> &Arc<Path> {
        &self.path
    }

    pub fn entries(&self) -> &[DirEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, index: usize) -> Option<&DirEntry> {
        self.entries.get(index)
    }
}

/// Orders entries by [`DirEntryKind::rank`], and two entries of the same rank
/// by [`compare_file_names`].
fn compare_entries(a: &DirEntry, b: &DirEntry) -> Ordering {
    a.kind
        .rank()
        .cmp(&b.kind.rank())
        .then_with(|| compare_file_names(&a.path, &b.path))
}

/// Orders two paths by their file name, folding each character to lower case as
/// it is read, and falls back to the unfolded names when the folded ones come
/// out equal. A name that is not valid UTF-8 is compared as `to_string_lossy`
/// renders it, every invalid byte standing in as U+FFFD. A path with no file
/// name counts as empty.
pub(crate) fn compare_file_names(a: &Path, b: &Path) -> Ordering {
    let a = a.file_name().unwrap_or_default().to_string_lossy();
    let b = b.file_name().unwrap_or_default().to_string_lossy();

    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        .then_with(|| a.cmp(&b))
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    use crate::fs::temp_tree::TempTree;

    /// Builds one entry per `(name, kind)` pair, all directly under `/`.
    fn entries(names: &[(&str, DirEntryKind)]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|(name, kind)| DirEntry {
                path: Arc::from(Path::new("/").join(name).as_path()),
                kind: *kind,
            })
            .collect()
    }

    fn names(directory: &Directory) -> Vec<String> {
        directory
            .entries()
            .iter()
            .map(|entry| entry.path.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn names_sort_case_insensitively_and_tie_break_on_the_raw_name() {
        let directory = Directory::new(
            Arc::from(Path::new("/")),
            entries(&[
                ("banana", DirEntryKind::File),
                ("apple", DirEntryKind::File),
                ("Cherry", DirEntryKind::File),
                ("Apple", DirEntryKind::File),
            ]),
        );

        assert_eq!(
            names(&directory),
            ["/Apple", "/apple", "/banana", "/Cherry"]
        );
    }

    #[test]
    fn the_parent_leads_and_directories_come_before_files() {
        let directory = Directory::new(
            Arc::from(Path::new("/home")),
            entries(&[
                ("zip", DirEntryKind::File),
                ("src", DirEntryKind::Directory),
                ("..", DirEntryKind::Parent),
                ("Makefile", DirEntryKind::File),
                ("assets", DirEntryKind::Directory),
            ]),
        );

        assert_eq!(
            names(&directory),
            ["/..", "/assets", "/src", "/Makefile", "/zip"]
        );
    }

    #[test]
    fn every_kind_sorts_by_rank_before_it_sorts_by_name() {
        // The names run in the opposite order to the ranks, so any listing that
        // came out alphabetically would show here.
        let directory = Directory::new(
            Arc::from(Path::new("/home")),
            entries(&[
                ("a-file", DirEntryKind::File),
                ("b-link", DirEntryKind::Symlink),
                ("c-dir", DirEntryKind::Directory),
                ("..", DirEntryKind::Parent),
            ]),
        );

        assert_eq!(names(&directory), ["/..", "/c-dir", "/b-link", "/a-file"]);
    }

    #[test]
    fn a_directory_leads_a_symlink_that_sorts_earlier_by_name() {
        // Compared by name alone these three close a cycle: the directory z
        // precedes the file a, a precedes the symlink b, and b precedes z.
        let directory = Directory::new(
            Arc::from(Path::new("/home")),
            entries(&[
                ("z", DirEntryKind::Directory),
                ("a", DirEntryKind::File),
                ("b", DirEntryKind::Symlink),
            ]),
        );

        assert_eq!(names(&directory), ["/z", "/b", "/a"]);
    }

    #[test]
    fn the_parent_entry_holds_the_resolved_parent_path() {
        let tree = TempTree::new();
        let nested = tree.make_dir("bla/blac");

        let directory = Directory::read(Arc::from(nested.as_path())).unwrap();
        let parent = directory.get(0).unwrap();

        assert_eq!(parent.kind, DirEntryKind::Parent);
        assert_eq!(parent.path.as_ref(), tree.at("bla"));
        assert!(parent.path.is_dir());
    }

    #[test]
    fn symlinks_sort_among_themselves_by_name() {
        let directory = Directory::new(
            Arc::from(Path::new("/home")),
            entries(&[
                ("Link", DirEntryKind::Symlink),
                ("anchor", DirEntryKind::Symlink),
                ("bin", DirEntryKind::Symlink),
            ]),
        );

        assert_eq!(names(&directory), ["/anchor", "/bin", "/Link"]);
    }
}
