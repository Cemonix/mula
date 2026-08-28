use std::{cmp::Ordering, io, os::unix::fs::MetadataExt, path::Path, sync::Arc};

/// How much of each entry a listing reads.
///
/// Ordered by how much that is, so a pane can ask whether the listing it has
/// already answers the question it is now being asked: reading more also
/// answers everything a thinner read would have.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub enum Detail {
    /// The name and the kind, both of which `read_dir` gives away.
    NamesOnly,
    /// Size and modification time as well, one `stat` per entry.
    WithMetadata,
}

/// What one `stat` of an entry said.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct EntryMeta {
    pub size: u64,
    /// `st_mtime`, in seconds since the epoch.
    pub modified: i64,
}

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
    /// `None` throughout a [`Detail::NamesOnly`] listing, and on the one entry
    /// that went away between the `read_dir` and the `stat` that followed it.
    pub meta: Option<EntryMeta>,
}

/// One directory listing, sorted on construction. A `Directory` is a snapshot:
/// nothing mutates it, and a newer listing means reading another one.
#[derive(Debug)]
pub struct Directory {
    path: Arc<Path>,
    entries: Vec<DirEntry>,
    detail: Detail,
}

impl Directory {
    /// Reads `path`, prefixing the listing with a [`DirEntryKind::Parent`]
    /// entry whenever `path` has a parent. A directory that cannot be opened,
    /// or a single entry whose type cannot be read, fails the whole listing.
    ///
    /// `detail` is what the listing costs: [`Detail::NamesOnly`] is the one
    /// `read_dir`, and [`Detail::WithMetadata`] adds a `stat` per entry. On a
    /// directory of tens of thousands of files that is the difference between
    /// one pass over the directory and one syscall per name in it, so a caller
    /// that draws no columns asks for no metadata.
    ///
    /// Neither `kind` nor the metadata follows symlinks: every symlink is
    /// reported as [`DirEntryKind::Symlink`] and sized as the link itself,
    /// whatever it points at.
    pub fn read(path: Arc<Path>, detail: Detail) -> Result<Self, io::Error> {
        let mut entries = Vec::new();

        if let Some(parent) = path.parent() {
            entries.push(DirEntry {
                path: Arc::from(parent),
                kind: DirEntryKind::Parent,
                meta: meta_of(parent, detail),
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
            let path = entry.path();
            entries.push(DirEntry {
                meta: meta_of(&path, detail),
                path: Arc::from(path),
                kind,
            });
        }

        Ok(Self::new(path, entries, detail))
    }

    /// Sorts `entries` with [`compare_entries`] and stores them under `path`.
    /// The entries are taken as given: nothing checks that they live in `path`,
    /// that their metadata matches `detail`, and no `Parent` entry is added.
    pub fn new(path: Arc<Path>, mut entries: Vec<DirEntry>, detail: Detail) -> Self {
        entries.sort_by(compare_entries);
        Self {
            path,
            entries,
            detail,
        }
    }

    pub fn path(&self) -> &Arc<Path> {
        &self.path
    }

    /// What this listing was read with, and so which columns it can fill.
    pub fn detail(&self) -> Detail {
        self.detail
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

/// The metadata a [`Detail::WithMetadata`] listing carries for `path`, and
/// nothing at all for a [`Detail::NamesOnly`] one.
///
/// A `stat` that fails leaves the entry without metadata rather than failing
/// the listing: the name came from a `read_dir` that has already finished, so
/// a file deleted in between is a row with empty columns and not a directory
/// that cannot be read.
fn meta_of(path: &Path, detail: Detail) -> Option<EntryMeta> {
    match detail {
        Detail::NamesOnly => None,
        Detail::WithMetadata => std::fs::symlink_metadata(path).ok().map(|meta| EntryMeta {
            size: meta.size(),
            modified: meta.mtime(),
        }),
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
                meta: None,
            })
            .collect()
    }

    /// A listing of `entries` under `path`, read with names alone.
    fn listing(path: &str, entries: Vec<DirEntry>) -> Directory {
        Directory::new(Arc::from(Path::new(path)), entries, Detail::NamesOnly)
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
        let directory = listing(
            "/",
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
        let directory = listing(
            "/home",
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
        let directory = listing(
            "/home",
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
        let directory = listing(
            "/home",
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

        let directory = Directory::read(Arc::from(nested.as_path()), Detail::NamesOnly).unwrap();
        let parent = directory.get(0).unwrap();

        assert_eq!(parent.kind, DirEntryKind::Parent);
        assert_eq!(parent.path.as_ref(), tree.at("bla"));
        assert!(parent.path.is_dir());
    }

    /// The `stat` per entry is what the columns cost, so a listing that was
    /// not asked for it does not pay it.
    #[test]
    fn a_listing_of_names_alone_carries_no_metadata() {
        let tree = TempTree::new();
        tree.make_file("one.txt", "hello\n");

        let directory = Directory::read(Arc::from(tree.path()), Detail::NamesOnly).unwrap();

        assert_eq!(directory.detail(), Detail::NamesOnly);
        assert!(directory.entries().iter().all(|entry| entry.meta.is_none()));
    }

    #[test]
    fn a_listing_read_with_metadata_carries_the_size_of_every_entry() {
        let tree = TempTree::new();
        tree.make_file("one.txt", "hello\n");
        tree.make_dir("sub");

        let directory = Directory::read(Arc::from(tree.path()), Detail::WithMetadata).unwrap();

        assert_eq!(directory.detail(), Detail::WithMetadata);
        // The parent is read too, so no row is left with an empty column.
        assert!(directory.entries().iter().all(|entry| entry.meta.is_some()));
        let file = directory
            .entries()
            .iter()
            .find(|entry| entry.path.ends_with("one.txt"))
            .unwrap();
        assert_eq!(file.meta.unwrap().size, 6);
    }

    /// A link is sized as itself. Following it would report the size of
    /// something the row does not name, and would fail on a broken link.
    #[test]
    fn a_symlink_is_sized_as_the_link_and_not_as_its_target() {
        let tree = TempTree::new();
        let file = tree.make_file("one.txt", "hello\n");
        std::os::unix::fs::symlink(&file, tree.at("link")).unwrap();

        let directory = Directory::read(Arc::from(tree.path()), Detail::WithMetadata).unwrap();
        let link = directory
            .entries()
            .iter()
            .find(|entry| entry.path.ends_with("link"))
            .unwrap();

        assert_eq!(link.kind, DirEntryKind::Symlink);
        assert_eq!(link.meta.unwrap().size, file.as_os_str().len() as u64);
    }

    #[test]
    fn symlinks_sort_among_themselves_by_name() {
        let directory = listing(
            "/home",
            entries(&[
                ("Link", DirEntryKind::Symlink),
                ("anchor", DirEntryKind::Symlink),
                ("bin", DirEntryKind::Symlink),
            ]),
        );

        assert_eq!(names(&directory), ["/anchor", "/bin", "/Link"]);
    }
}
