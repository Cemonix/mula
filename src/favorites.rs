use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use toml::Value;

/// The key the list is written under.
const KEY: &str = "favorites";

/// Written above the list, so the file says where it came from when it is
/// found by someone who did not put it there.
const HEADER: &str = "# The directories Mula jumps to. Written by Mula itself.\n";

/// Where the list lives: `$XDG_STATE_HOME/mula`, else the platform's own place
/// for what a program keeps about itself.
///
/// Not `~/.config/mula`, which holds the keys. That file is written by the
/// user and read by Mula; this one is the other way round, and rewriting a
/// file someone edits by hand would cost them their comments.
///
/// `None` when there is no home to write under.
fn favorites_file() -> Option<PathBuf> {
    let dir = match env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
        Some(state) => PathBuf::from(state),
        None => {
            let home = PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?);
            match cfg!(target_os = "macos") {
                true => home.join("Library/Application Support"),
                false => home.join(".local/state"),
            }
        }
    };
    Some(dir.join("mula").join("favorites.toml"))
}

#[derive(Error, Debug)]
pub enum FavoritesError {
    #[error("there is no home directory to keep the favorites in")]
    NoHome,
    #[error("{} could not be read: {source}", path.display())]
    Unreadable { path: PathBuf, source: io::Error },
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{} holds something other than a list of paths under {KEY}", path.display())]
    NotPaths { path: PathBuf },
    #[error("{} is not text, and only text can be written down", path.display())]
    NotText { path: PathBuf },
    #[error("{} could not be written: {source}", path.display())]
    Unwritable { path: PathBuf, source: io::Error },
    #[error("the favorites file could not be read, so it is left as it is until Mula restarts")]
    ReadOnly,
}

/// What adding a directory came to. A directory already on the list is not an
/// error and not a change either, and the two read differently to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    Added,
    AlreadyListed,
}

/// The directories the user comes back to, and the file they are kept in.
///
/// The list and the file agree: every change is written out at once, and a
/// write that fails leaves the list as it was.
#[derive(Debug)]
pub struct Favorites {
    paths: Vec<PathBuf>,
    /// Where the list is written back, or `None` when it is not written at
    /// all: there is nowhere to write it, or what is on disk could not be
    /// read and rewriting it would take a list the user still has.
    file: Option<PathBuf>,
}

impl Favorites {
    /// The list as it stands on disk, and what went wrong reading it.
    ///
    /// A file that is not there yet is an empty list and no complaint — it is
    /// what every first run looks like.
    pub fn load() -> (Self, Option<FavoritesError>) {
        let Some(file) = favorites_file() else {
            return (Self::closed(), Some(FavoritesError::NoHome));
        };

        match read(&file) {
            Ok(paths) => (
                Self {
                    paths,
                    file: Some(file),
                },
                None,
            ),
            Err(e) => (Self::closed(), Some(e)),
        }
    }

    /// A list that is written nowhere, which is what a file that could not be
    /// read leaves behind.
    fn closed() -> Self {
        Self {
            paths: Vec::new(),
            file: None,
        }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Puts `path` at the end of the list and writes it out.
    ///
    /// The end rather than the front: a list that reorders itself is one the
    /// user has to read again every time they open it.
    pub fn add(&mut self, path: &Path) -> Result<Added, FavoritesError> {
        if path.to_str().is_none() {
            return Err(FavoritesError::NotText {
                path: path.to_path_buf(),
            });
        }
        if self.paths.iter().any(|listed| listed == path) {
            return Ok(Added::AlreadyListed);
        }

        self.paths.push(path.to_path_buf());
        if let Err(e) = self.save() {
            self.paths.pop();
            return Err(e);
        }
        Ok(Added::Added)
    }

    /// Takes `path` off the list and writes it out. A path the list does not
    /// hold leaves both alone.
    ///
    /// The path rather than a row number: the caller reads the row out of
    /// [`Favorites::paths`], and a row that is no longer there hands back
    /// nothing rather than an index to be checked.
    pub fn remove(&mut self, path: &Path) -> Result<(), FavoritesError> {
        let Some(index) = self.paths.iter().position(|listed| listed == path) else {
            return Ok(());
        };

        let removed = self.paths.remove(index);
        if let Err(e) = self.save() {
            self.paths.insert(index, removed);
            return Err(e);
        }
        Ok(())
    }

    /// Writes the whole list out, creating the directory above it.
    ///
    /// The whole list rather than the one line that changed: it is a handful
    /// of paths, and rewriting it is what keeps a removal removed.
    fn save(&self) -> Result<(), FavoritesError> {
        let Some(file) = &self.file else {
            return Err(FavoritesError::ReadOnly);
        };

        let listed = self
            .paths
            .iter()
            .map(|path| {
                path.to_str()
                    .map(|path| Value::String(path.to_string()))
                    .ok_or_else(|| FavoritesError::NotText {
                        path: path.to_path_buf(),
                    })
            })
            .collect::<Result<Vec<Value>, FavoritesError>>()?;

        let mut document = toml::Table::new();
        document.insert(KEY.to_string(), Value::Array(listed));

        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|source| FavoritesError::Unwritable {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(file, format!("{HEADER}{document}")).map_err(|source| {
            FavoritesError::Unwritable {
                path: file.clone(),
                source,
            }
        })
    }
}

/// Reads the list out of `file`, treating a file that is not there as a list
/// with nothing on it.
fn read(file: &Path) -> Result<Vec<PathBuf>, FavoritesError> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(FavoritesError::Unreadable {
                path: file.to_path_buf(),
                source,
            });
        }
    };
    parse(file, &text)
}

/// Takes the array of strings under `favorites` out of the text of the file.
/// A file holding no such key is a list with nothing on it; anything else
/// under it is a file this cannot be reading.
fn parse(file: &Path, text: &str) -> Result<Vec<PathBuf>, FavoritesError> {
    let document: toml::Table = text.parse().map_err(|source| FavoritesError::Malformed {
        path: file.to_path_buf(),
        source,
    })?;

    let Some(listed) = document.get(KEY) else {
        return Ok(Vec::new());
    };
    let Value::Array(listed) = listed else {
        return Err(FavoritesError::NotPaths {
            path: file.to_path_buf(),
        });
    };

    listed
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(PathBuf::from)
                .ok_or_else(|| FavoritesError::NotPaths {
                    path: file.to_path_buf(),
                })
        })
        .collect()
}

#[cfg(test)]
mod favorites_tests {
    use super::*;

    use crate::fs::temp_tree::TempTree;

    /// A list kept in a file of its own inside `tree`.
    fn favorites(tree: &TempTree) -> Favorites {
        Favorites {
            paths: Vec::new(),
            file: Some(tree.at("state/favorites.toml")),
        }
    }

    #[test]
    fn a_file_that_is_not_there_yet_is_an_empty_list() {
        let tree = TempTree::new();

        assert!(read(&tree.at("nothing.toml")).unwrap().is_empty());
    }

    #[test]
    fn what_was_added_is_read_back() {
        let tree = TempTree::new();
        let mut list = favorites(&tree);

        list.add(Path::new("/one")).unwrap();
        list.add(Path::new("/two")).unwrap();

        let paths = read(&tree.at("state/favorites.toml")).unwrap();
        assert_eq!(paths, [PathBuf::from("/one"), PathBuf::from("/two")]);
    }

    #[test]
    fn adding_a_directory_twice_adds_it_once() {
        let tree = TempTree::new();
        let mut list = favorites(&tree);

        assert_eq!(list.add(Path::new("/one")).unwrap(), Added::Added);
        assert_eq!(list.add(Path::new("/one")).unwrap(), Added::AlreadyListed);
        assert_eq!(list.paths(), [PathBuf::from("/one")]);
    }

    #[test]
    fn a_removal_is_written_out() {
        let tree = TempTree::new();
        let mut list = favorites(&tree);
        list.add(Path::new("/one")).unwrap();
        list.add(Path::new("/two")).unwrap();

        list.remove(Path::new("/one")).unwrap();

        let paths = read(&tree.at("state/favorites.toml")).unwrap();
        assert_eq!(paths, [PathBuf::from("/two")]);
    }

    /// The file could not be read, so it is not written either: the user's own
    /// list is still in it, and one add would replace it with one entry.
    #[test]
    fn a_list_that_is_written_nowhere_takes_nothing() {
        let mut list = Favorites::closed();

        let error = list.add(Path::new("/one")).unwrap_err();

        assert!(matches!(error, FavoritesError::ReadOnly), "{error}");
        assert!(list.paths().is_empty());
    }

    #[test]
    fn a_file_holding_something_else_is_not_read_as_a_list() {
        let file = Path::new("/favorites.toml");

        assert!(matches!(
            parse(file, "favorites = 3").unwrap_err(),
            FavoritesError::NotPaths { .. }
        ));
        assert!(matches!(
            parse(file, "favorites = [3]").unwrap_err(),
            FavoritesError::NotPaths { .. }
        ));
        assert!(matches!(
            parse(file, "favorites = [").unwrap_err(),
            FavoritesError::Malformed { .. }
        ));
    }

    /// A path with a quote or a backslash in it comes back as it went in,
    /// which is what writing TOML rather than a line per path is for.
    #[test]
    fn a_path_with_quotes_in_it_survives_the_file() {
        let tree = TempTree::new();
        let mut list = favorites(&tree);
        let awkward = Path::new("/tmp/a \"quoted\"\\name");

        list.add(awkward).unwrap();

        let paths = read(&tree.at("state/favorites.toml")).unwrap();
        assert_eq!(paths, [awkward.to_path_buf()]);
    }
}
