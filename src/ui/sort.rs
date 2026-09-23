use std::cmp::Ordering;

use crate::fs::directory::{Detail, DirEntry, DirEntryKind, compare_file_names};

/// What a listing is ordered by, within each kind of entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Extension,
}

/// Whether a key runs its own way or the other.
///
/// A key's own way is the one it is usually asked for in: names and
/// extensions from A, sizes from the largest, times from the newest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Natural,
    Reversed,
}

/// How a pane orders its listing.
///
/// The parent leads and directories come before symlinks and files whatever
/// the key; the key orders the entries within each of those groups, and two
/// entries it cannot tell apart fall back to their names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    key: SortKey,
    order: Order,
}

impl Sort {
    /// The next key along, run its own way.
    pub fn next_key(self) -> Self {
        let key = match self.key {
            SortKey::Name => SortKey::Modified,
            SortKey::Modified => SortKey::Size,
            SortKey::Size => SortKey::Extension,
            SortKey::Extension => SortKey::Name,
        };
        Self {
            key,
            order: Order::Natural,
        }
    }

    /// The same key, run the other way.
    pub fn reversed(self) -> Self {
        let order = match self.order {
            Order::Natural => Order::Reversed,
            Order::Reversed => Order::Natural,
        };
        Self { order, ..self }
    }

    /// How much of each entry a listing has to be read with for this key to
    /// have anything to compare.
    pub fn detail(self) -> Detail {
        match self.key {
            SortKey::Name | SortKey::Extension => Detail::NamesOnly,
            SortKey::Modified | SortKey::Size => Detail::WithMetadata,
        }
    }

    /// Describes the order for the border of the pane, or `None` for the one
    /// every pane starts in.
    pub fn label(self) -> Option<String> {
        if self == Self::default() {
            return None;
        }

        let key = match self.key {
            SortKey::Name => "name",
            SortKey::Modified => "time",
            SortKey::Size => "size",
            SortKey::Extension => "extension",
        };
        Some(match self.order {
            Order::Natural => format!("by {key}"),
            Order::Reversed => format!("by {key}, reversed"),
        })
    }

    pub fn compare(self, a: &DirEntry, b: &DirEntry) -> Ordering {
        let by_key = match self.key {
            SortKey::Name => compare_file_names(&a.path, &b.path),
            SortKey::Extension => extension(a).cmp(&extension(b)),
            SortKey::Size => size(b).cmp(&size(a)),
            SortKey::Modified => modified(b).cmp(&modified(a)),
        };
        let by_key = match self.order {
            Order::Natural => by_key,
            Order::Reversed => by_key.reverse(),
        };

        a.kind
            .rank()
            .cmp(&b.kind.rank())
            .then(by_key)
            .then_with(|| compare_file_names(&a.path, &b.path))
    }
}

/// The extension folded to lower case, `None` for a name without one.
fn extension(entry: &DirEntry) -> Option<String> {
    let extension = entry.path.extension()?.to_string_lossy();
    Some(extension.to_lowercase())
}

/// The size the size column shows: `None` for a directory, and for an entry
/// read without metadata.
fn size(entry: &DirEntry) -> Option<u64> {
    match entry.kind {
        DirEntryKind::Parent | DirEntryKind::Directory => None,
        DirEntryKind::Symlink | DirEntryKind::File => entry.meta.map(|meta| meta.size),
    }
}

fn modified(entry: &DirEntry) -> Option<i64> {
    entry.meta.map(|meta| meta.modified)
}

#[cfg(test)]
mod sort_tests {
    use std::{path::Path, sync::Arc};

    use super::*;
    use crate::fs::directory::EntryMeta;

    fn entry(name: &str, kind: DirEntryKind, meta: Option<(u64, i64)>) -> DirEntry {
        DirEntry {
            path: Arc::from(Path::new("/a").join(name).as_path()),
            kind,
            meta: meta.map(|(size, modified)| EntryMeta { size, modified }),
        }
    }

    fn file(name: &str, size: u64, modified: i64) -> DirEntry {
        entry(name, DirEntryKind::File, Some((size, modified)))
    }

    fn sorted(sort: Sort, mut entries: Vec<DirEntry>) -> Vec<String> {
        entries.sort_by(|a, b| sort.compare(a, b));
        entries
            .iter()
            .map(|entry| crate::ui::name_of(&entry.path))
            .collect()
    }

    fn by(key: SortKey) -> Sort {
        Sort {
            key,
            order: Order::Natural,
        }
    }

    #[test]
    fn sizes_run_from_the_largest() {
        let entries = vec![file("small", 1, 0), file("big", 100, 0), file("mid", 10, 0)];

        assert_eq!(sorted(by(SortKey::Size), entries), ["big", "mid", "small"]);
    }

    #[test]
    fn times_run_from_the_newest() {
        let entries = vec![file("old", 0, 1), file("new", 0, 3), file("mid", 0, 2)];

        assert_eq!(
            sorted(by(SortKey::Modified), entries),
            ["new", "mid", "old"]
        );
    }

    #[test]
    fn reversing_turns_the_key_around() {
        let entries = vec![file("small", 1, 0), file("big", 100, 0), file("mid", 10, 0)];

        assert_eq!(
            sorted(by(SortKey::Size).reversed(), entries),
            ["small", "mid", "big"]
        );
    }

    /// Reversing turns the key around and nothing else: the parent is still
    /// the way out, and still first.
    #[test]
    fn the_parent_and_the_directories_lead_whatever_the_order() {
        let entries = vec![
            file("zeta", 1, 0),
            entry("dir", DirEntryKind::Directory, Some((4096, 0))),
            entry("up", DirEntryKind::Parent, None),
            file("alpha", 2, 0),
        ];

        assert_eq!(
            sorted(Sort::default().reversed(), entries),
            ["up", "dir", "zeta", "alpha"]
        );
    }

    /// A directory's own `st_size` is not what is in it, so the size key does
    /// not order directories by it; their names do.
    #[test]
    fn directories_sorted_by_size_keep_their_names_order() {
        let entries = vec![
            entry("b", DirEntryKind::Directory, Some((64, 0))),
            entry("a", DirEntryKind::Directory, Some((4096, 0))),
        ];

        assert_eq!(sorted(by(SortKey::Size), entries), ["a", "b"]);
    }

    #[test]
    fn entries_the_key_cannot_tell_apart_fall_back_to_their_names() {
        let entries = vec![file("b.txt", 5, 0), file("a.txt", 5, 0), file("c.rs", 5, 0)];

        assert_eq!(
            sorted(by(SortKey::Extension), entries),
            ["c.rs", "a.txt", "b.txt"]
        );
    }

    #[test]
    fn an_extension_is_compared_without_its_case() {
        let entries = vec![file("b.TXT", 0, 0), file("a.rs", 0, 0), file("c.txt", 0, 0)];

        assert_eq!(
            sorted(by(SortKey::Extension), entries),
            ["a.rs", "b.TXT", "c.txt"]
        );
    }

    /// The listing a key needs metadata for may arrive without it; until the
    /// thicker one lands the names are all there is to go on.
    #[test]
    fn entries_read_without_metadata_fall_back_to_their_names() {
        let entries = vec![
            entry("b", DirEntryKind::File, None),
            entry("a", DirEntryKind::File, None),
        ];

        assert_eq!(sorted(by(SortKey::Size), entries), ["a", "b"]);
    }

    #[test]
    fn cycling_the_key_comes_back_to_the_name_and_runs_its_own_way() {
        let mut sort = Sort::default().reversed();
        for _ in 0..4 {
            sort = sort.next_key();
            assert_eq!(sort.order, Order::Natural);
        }

        assert_eq!(sort, Sort::default());
    }

    #[test]
    fn a_key_that_compares_metadata_asks_for_it() {
        assert_eq!(by(SortKey::Name).detail(), Detail::NamesOnly);
        assert_eq!(by(SortKey::Extension).detail(), Detail::NamesOnly);
        assert_eq!(by(SortKey::Size).detail(), Detail::WithMetadata);
        assert_eq!(by(SortKey::Modified).detail(), Detail::WithMetadata);
    }

    #[test]
    fn the_order_every_pane_starts_in_is_not_labelled() {
        assert_eq!(Sort::default().label(), None);
        assert_eq!(
            by(SortKey::Size).reversed().label().as_deref(),
            Some("by size, reversed")
        );
    }
}
