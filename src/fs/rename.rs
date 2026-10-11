//! Renaming many items at once through a text editor: the file of names that
//! is handed over, what comes back read against the items it was written
//! from, and the batch that carries the answer out.

use std::{
    collections::HashSet,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

use crate::fs::ops::{ProcessedSummary, Transfer};

/// Why the names an editor gave back cannot be carried out. Nothing of the
/// batch is renamed for any of them.
#[derive(Error, Debug, PartialEq, Eq)]
pub enum Refusal {
    #[error("{expected} names were handed out and {found} came back")]
    Count { expected: usize, found: usize },
    #[error("Line {0} is empty")]
    Empty(usize),
    #[error("\"{0}\" is not a name")]
    NotAName(String),
    #[error("Two items would be called \"{0}\"")]
    Twice(String),
}

/// Writes the names of `items`, one per line in their order, into a file of
/// its own under the system temp directory, and returns where.
///
/// Fails with `InvalidInput` for a name that is not UTF-8 or holds a line
/// break, since no line of text could carry it there and back.
pub fn write_names(items: &[PathBuf]) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut text = String::new();
    for item in items {
        let name = item
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.contains(['\n', '\r']))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} cannot be written as a line", item.display()),
                )
            })?;
        text.push_str(name);
        text.push('\n');
    }

    loop {
        let path = std::env::temp_dir().join(format!(
            "mula-rename-{}-{}.txt",
            process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::File::create_new(&path) {
            Ok(_) => {
                fs::write(&path, &text)?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Reads the file [`write_names`] made and removes it. The file is gone
/// afterwards whether or not it could be read.
pub fn take_names(path: &Path) -> io::Result<String> {
    let text = fs::read_to_string(path);
    if let Err(error) = fs::remove_file(path) {
        tracing::warn!(path = ?path, %error, "the names file could not be removed");
    }
    text
}

/// Pairs every item with the line at its position in `edited` and returns the
/// pairs whose name changed, each target in the directory of its source.
///
/// Blank lines at the end are passed over. Anything else that would leave an
/// item without a name, or two items under one, refuses the whole batch:
/// a different count of lines, an empty line, a line holding `/` or naming
/// `.` or `..`, and a target that is already the path of another item of the
/// batch once it is done.
pub fn pair(items: &[PathBuf], edited: &str) -> Result<Vec<Transfer>, Refusal> {
    let mut lines: Vec<&str> = edited.lines().collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    if lines.len() != items.len() {
        return Err(Refusal::Count {
            expected: items.len(),
            found: lines.len(),
        });
    }

    let mut pairs = Vec::new();
    let mut taken = HashSet::new();
    for (number, (item, &name)) in items.iter().zip(&lines).enumerate() {
        if name.is_empty() {
            return Err(Refusal::Empty(number + 1));
        }
        if name.contains('/') || name == "." || name == ".." {
            return Err(Refusal::NotAName(name.to_owned()));
        }

        let to = item.with_file_name(name);
        if !taken.insert(to.clone()) {
            return Err(Refusal::Twice(name.to_owned()));
        }
        if to != *item {
            pairs.push(Transfer {
                from: item.clone(),
                to,
            });
        }
    }

    Ok(pairs)
}

/// What came of a batch of renames.
#[derive(Debug)]
pub struct Renamed {
    pub summary: ProcessedSummary,
    /// Where each item that could not be renamed is now: under its old name,
    /// or under the name it was set aside under when even that is taken.
    pub failed: Vec<PathBuf>,
    pub reason: Option<String>,
}

/// Renames every pair, in two steps: each source is first set aside under a
/// hidden name in its own directory, and only then given its new one. A chain
/// (`a → b`, `b → c`) and a cycle (`a ↔ b`) need no ordering that way, and a
/// change of case alone goes through on a filesystem that ignores case.
///
/// Before anything moves, a target that exists and is not the source of some
/// pair refuses the whole batch, as does a source that cannot be set aside;
/// what was set aside by then goes back. A source that is gone is skipped.
/// Not cancellable: every step is one `rename`.
pub fn rename_all(pairs: Vec<Transfer>) -> Renamed {
    let mut summary = ProcessedSummary::new(pairs.len());

    let mut present = Vec::new();
    let mut moving = HashSet::new();
    let mut queue = pairs.into_iter();
    while let Some(pair) = queue.next() {
        match pair.from.symlink_metadata() {
            Ok(meta) => {
                moving.insert((meta.dev(), meta.ino()));
                present.push(pair);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => summary.skip(),
            Err(e) => {
                let at = present
                    .into_iter()
                    .chain([pair])
                    .chain(queue)
                    .map(|pair| pair.from);
                return refused(summary, at.collect(), e);
            }
        }
    }

    if let Some(blocked) = present.iter().find(|pair| {
        pair.to
            .symlink_metadata()
            .is_ok_and(|meta| !moving.contains(&(meta.dev(), meta.ino())))
    }) {
        let error = io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", blocked.to.display()),
        );
        let at = present.into_iter().map(|pair| pair.from);
        return refused(summary, at.collect(), error);
    }

    let mut aside: Vec<(Transfer, PathBuf)> = Vec::new();
    let mut queue = present.into_iter().enumerate();
    while let Some((index, pair)) = queue.next() {
        let hidden = pair
            .from
            .with_file_name(format!(".mula-rename-{}-{index}", process::id()));
        if let Err(e) = vacant(&hidden).and_then(|()| fs::rename(&pair.from, &hidden)) {
            let mut at: Vec<PathBuf> = [pair]
                .into_iter()
                .chain(queue.map(|(_, pair)| pair))
                .map(|pair| pair.from)
                .collect();
            for (pair, hidden) in aside {
                match fs::rename(&hidden, &pair.from) {
                    Ok(()) => at.push(pair.from),
                    Err(error) => {
                        tracing::error!(path = ?hidden, %error, "could not be put back");
                        at.push(hidden);
                    }
                }
            }
            return refused(summary, at, e);
        }
        aside.push((pair, hidden));
    }

    let mut failed = Vec::new();
    let mut reason = None;
    for (pair, hidden) in aside {
        match vacant(&pair.to).and_then(|()| fs::rename(&hidden, &pair.to)) {
            Ok(()) => summary.process(),
            Err(e) => {
                tracing::error!(path = ?pair.to, error = %e, "rename failed");
                reason = Some(e.to_string());
                summary.fail();
                let back = vacant(&pair.from).and_then(|()| fs::rename(&hidden, &pair.from));
                failed.push(if back.is_ok() { pair.from } else { hidden });
            }
        }
    }

    Renamed {
        summary,
        failed,
        reason,
    }
}

/// The batch given up on `error`, with every item that had not been renamed
/// counted as failed where it is now.
fn refused(mut summary: ProcessedSummary, at: Vec<PathBuf>, error: io::Error) -> Renamed {
    tracing::error!(%error, "rename batch refused");
    for _ in &at {
        summary.fail();
    }
    Renamed {
        summary,
        failed: at,
        reason: Some(error.to_string()),
    }
}

/// Fails with `AlreadyExists` when anything, a dangling link included, is at
/// `path`.
fn vacant(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", path.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::fs::temp_tree::TempTree;

    fn items(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| Path::new("/d").join(name))
            .collect()
    }

    fn pairs(tree: &TempTree, names: &[(&str, &str)]) -> Vec<Transfer> {
        names
            .iter()
            .map(|(from, to)| Transfer {
                from: tree.at(from),
                to: tree.at(to),
            })
            .collect()
    }

    fn contents(path: PathBuf) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn only_the_lines_that_changed_become_pairs() {
        let paired = pair(&items(&["a", "b", "c"]), "a\nB\nc\n").unwrap();

        assert_eq!(
            paired,
            vec![Transfer {
                from: PathBuf::from("/d/b"),
                to: PathBuf::from("/d/B"),
            }]
        );
    }

    #[test]
    fn blank_lines_at_the_end_are_passed_over() {
        assert!(pair(&items(&["a"]), "x\n\n\n").is_ok());
    }

    #[test]
    fn a_missing_line_refuses_the_batch() {
        assert_eq!(
            pair(&items(&["a", "b"]), "a\n"),
            Err(Refusal::Count {
                expected: 2,
                found: 1
            })
        );
    }

    #[test]
    fn an_empty_line_in_the_middle_refuses_the_batch() {
        assert_eq!(
            pair(&items(&["a", "b", "c"]), "a\n\nc\n"),
            Err(Refusal::Empty(2))
        );
    }

    #[test]
    fn a_name_cannot_reach_into_another_directory() {
        for name in ["x/y", ".", ".."] {
            assert_eq!(
                pair(&items(&["a"]), name),
                Err(Refusal::NotAName(name.to_owned()))
            );
        }
    }

    #[test]
    fn two_items_cannot_end_under_one_name() {
        assert_eq!(
            pair(&items(&["a", "b"]), "x\nx\n"),
            Err(Refusal::Twice("x".to_owned()))
        );
    }

    #[test]
    fn a_new_name_cannot_be_one_that_stays() {
        assert_eq!(
            pair(&items(&["a", "b"]), "a\na\n"),
            Err(Refusal::Twice("a".to_owned()))
        );
    }

    #[test]
    fn the_same_name_in_two_directories_is_two_names() {
        let items = vec![PathBuf::from("/d/a"), PathBuf::from("/e/b")];

        assert!(pair(&items, "x\nx\n").is_ok());
    }

    #[test]
    fn names_go_out_one_per_line_and_come_back_once() {
        let file = write_names(&items(&["a b", "c"])).unwrap();

        assert_eq!(take_names(&file).unwrap(), "a b\nc\n");
        assert!(!file.exists());
    }

    #[test]
    fn a_name_with_a_line_break_is_not_written() {
        let error = write_names(&items(&["a\nb"])).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn two_names_trade_places() {
        let tree = TempTree::new();
        tree.make_file("a", "A");
        tree.make_file("b", "B");

        let renamed = rename_all(pairs(&tree, &[("a", "b"), ("b", "a")]));

        assert_eq!(renamed.summary.processed(), 2);
        assert_eq!(contents(tree.at("a")), "B");
        assert_eq!(contents(tree.at("b")), "A");
    }

    #[test]
    fn a_chain_moves_along() {
        let tree = TempTree::new();
        tree.make_file("1", "one");
        tree.make_file("2", "two");

        rename_all(pairs(&tree, &[("1", "2"), ("2", "3")]));

        assert_eq!(contents(tree.at("2")), "one");
        assert_eq!(contents(tree.at("3")), "two");
        assert!(!tree.at("1").exists());
    }

    #[test]
    fn a_target_outside_the_batch_refuses_all_of_it() {
        let tree = TempTree::new();
        tree.make_file("a", "A");
        tree.make_file("b", "B");
        tree.make_file("taken", "kept");

        let renamed = rename_all(pairs(&tree, &[("a", "x"), ("b", "taken")]));

        assert_eq!(renamed.summary.processed(), 0);
        assert_eq!(renamed.failed, vec![tree.at("a"), tree.at("b")]);
        assert_eq!(contents(tree.at("taken")), "kept");
        assert!(!tree.at("x").exists());
    }

    #[test]
    fn a_source_that_is_gone_is_skipped() {
        let tree = TempTree::new();
        tree.make_file("a", "A");

        let renamed = rename_all(pairs(&tree, &[("gone", "x"), ("a", "y")]));

        assert_eq!(renamed.summary.skipped(), 1);
        assert_eq!(renamed.summary.processed(), 1);
        assert_eq!(contents(tree.at("y")), "A");
    }

    #[test]
    fn nothing_is_left_set_aside() {
        let tree = TempTree::new();
        tree.make_file("a", "");
        tree.make_file("b", "");

        rename_all(pairs(&tree, &[("a", "b"), ("b", "a")]));

        let mut left: Vec<_> = fs::read_dir(tree.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["a", "b"]);
    }
}
