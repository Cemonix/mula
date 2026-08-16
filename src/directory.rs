use std::{cmp::Ordering, io, path::Path, rc::Rc};

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum DirEntryKind {
    Parent,
    Directory,
    File,
}

#[derive(Debug)]
pub struct DirEntry {
    pub path: Rc<Path>,
    pub kind: DirEntryKind,
}

/// One directory listing, sorted on construction. A `Directory` is a snapshot:
/// nothing mutates it, and a newer listing means reading another one.
#[derive(Debug)]
pub struct Directory {
    path: Rc<Path>,
    entries: Vec<DirEntry>,
}

impl Directory {
    /// Reads `path`, prefixing the listing with a [`DirEntryKind::Parent`]
    /// entry whenever `path` has a parent. A directory that cannot be opened,
    /// or a single entry that cannot be read, fails the whole listing.
    ///
    /// `kind` follows symlinks: a symlink pointing at a directory is reported
    /// as [`DirEntryKind::Directory`].
    pub fn read(path: Rc<Path>) -> Result<Self, io::Error> {
        let mut entries = Vec::new();

        if let Some(parent) = path.parent() {
            entries.push(DirEntry {
                path: Rc::from(parent),
                kind: DirEntryKind::Parent,
            });
        }

        for entry in std::fs::read_dir(&path)? {
            let entry_path: Rc<Path> = Rc::from(entry?.path());
            let kind = if entry_path.is_dir() {
                DirEntryKind::Directory
            } else {
                DirEntryKind::File
            };
            entries.push(DirEntry {
                path: entry_path,
                kind,
            });
        }

        Ok(Self::new(path, entries))
    }

    /// Sorts `entries` with [`compare_entries`] and stores them under `path`.
    /// The entries are taken as given: nothing checks that they live in `path`,
    /// and no `Parent` entry is added.
    pub fn new(path: Rc<Path>, mut entries: Vec<DirEntry>) -> Self {
        entries.sort_by(compare_entries);
        Self { path, entries }
    }

    pub fn path(&self) -> &Rc<Path> {
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

/// Orders the parent entry before everything, directories before files, and
/// two entries of the same kind by [`compare_file_names`].
fn compare_entries(a: &DirEntry, b: &DirEntry) -> Ordering {
    match (&a.kind, &b.kind) {
        (DirEntryKind::Parent, _) => Ordering::Less,
        (_, DirEntryKind::Parent) => Ordering::Greater,
        (DirEntryKind::Directory, DirEntryKind::File) => Ordering::Less,
        (DirEntryKind::File, DirEntryKind::Directory) => Ordering::Greater,
        _ => compare_file_names(&a.path, &b.path),
    }
}

/// Orders two paths by their file name, folding each character to lower case as
/// it is read, and falls back to the unfolded names when the folded ones come
/// out equal. A name that is not valid UTF-8 is compared as `to_string_lossy`
/// renders it, every invalid byte standing in as U+FFFD. A path with no file
/// name counts as empty.
fn compare_file_names(a: &Path, b: &Path) -> Ordering {
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

    /// Builds one entry per `(name, kind)` pair, all directly under `/`.
    fn entries(names: &[(&str, DirEntryKind)]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|(name, kind)| DirEntry {
                path: Rc::from(Path::new("/").join(name).as_path()),
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
            Rc::from(Path::new("/")),
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
            Rc::from(Path::new("/home")),
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
}
