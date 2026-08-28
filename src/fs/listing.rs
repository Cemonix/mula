//! Reading one directory on the reading thread.
//!
//! Unlike a preview, a listing is not cleared while the next one is read: the
//! pane goes on drawing what it already has, so a request that is replaced
//! costs nothing to throw away. What arrives is therefore always the answer to
//! the last question asked, and the pane never has to tell two apart.
//!
//! One reader instance per panel, not one for the role. `Reader` is last wins,
//! and `refresh_panes` asks both panels at once — a shared counter would drop
//! the first answer and leave that panel waiting for a generation that never
//! comes.

use std::{fs, io, path::Path, sync::Arc};

use crate::fs::{
    directory::{Detail, Directory},
    preview,
    reader::{Live, Outbox, ReadJob},
};

/// How much of a file is read to tell text from anything else. `file(1)` and
/// git settle it on a prefix of about this size, and a NUL that appears only
/// past it belongs to a file no editor would have made.
const SNIFFED_BYTES: usize = 8 * 1024;

/// One directory to read, taken when the request was sent. Like every job, it
/// never reads back.
///
/// `detail` travels with the path rather than being settled on the reading
/// thread, because how much of each entry is worth reading is a question about
/// what the pane draws, and only the pane knows the answer.
#[derive(Debug)]
pub struct Listing {
    pub path: Arc<Path>,
    pub detail: Detail,
}

/// What a path is, when it is not a directory. Enough to tell what should open
/// it, and no more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A regular file whose first bytes hold no NUL.
    Text,
    /// A regular file that is not text: a picture, an archive, a program.
    Opaque,
    /// A fifo, a socket or a device. Opening one blocks whatever opened it for
    /// as long as nobody is at the other end, so it is settled on the metadata
    /// and never opened.
    NotAFile,
}

/// What came of reading a path as a directory.
#[derive(Debug)]
pub enum Listed {
    Directory(Directory),
    /// It is not one. What it is instead travels with the answer, so entering
    /// an entry costs one read rather than two.
    NotADirectory(Kind),
}

impl ReadJob for Listing {
    type Config = ();
    /// The failure travels with the answer. A pane has no other way home, and
    /// a directory that cannot be read is news it has to report.
    type Msg = io::Result<Listed>;

    /// `live` goes unread: the reader already skips a request that was
    /// replaced before it started, and a listing reports nothing on its way,
    /// so there is no point between those two to check.
    fn run(self, _config: &Self::Config, _live: &Live<'_>, out: &Outbox<'_, Self::Msg>) {
        out.send(read(self.path, self.detail));
    }
}

/// Reads `path` as a directory, and works out what it is instead when it is
/// not one.
fn read(path: Arc<Path>, detail: Detail) -> io::Result<Listed> {
    match Directory::read(Arc::clone(&path), detail) {
        Ok(directory) => Ok(Listed::Directory(directory)),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => {
            Ok(Listed::NotADirectory(kind_of(&path)?))
        }
        Err(e) => Err(e),
    }
}

/// What `path` is, for a path that is known not to be a directory.
///
/// Symlinks are followed, here and in `Directory::read` before it: what a link
/// points at is what entering it enters and what opening it opens.
///
/// The metadata settles it before anything is opened, so the fifo that would
/// block `File::open` forever is turned away rather than read.
fn kind_of(path: &Path) -> io::Result<Kind> {
    if !fs::metadata(path)?.is_file() {
        return Ok(Kind::NotAFile);
    }

    let bytes = preview::read_prefix(path, SNIFFED_BYTES)?;
    Ok(if preview::is_text(&bytes) {
        Kind::Text
    } else {
        Kind::Opaque
    })
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt,
        thread,
        time::{Duration, Instant},
    };

    use crate::fs::{reader::Reader, temp_tree::TempTree};

    /// Drains the way the main loop does until the answer arrives, and gives
    /// up rather than hanging if it never does.
    fn settle(reader: &mut Reader<Listing>) -> io::Result<Listed> {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            assert!(Instant::now() < deadline, "the listing never came back");
            if let Some(listing) = reader.drain().msgs.pop() {
                return listing;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_listing_comes_back_with_the_entries_of_the_directory() {
        let tree = TempTree::of(["one.txt", "two.txt"]);

        let Listed::Directory(directory) = listed(tree.path()).unwrap() else {
            panic!("the directory did not come back as a listing");
        };

        // The parent leads the listing, so the two files sit behind it.
        assert_eq!(directory.len(), 3);
    }

    /// Sends `path` and waits for what came back, the way the main loop would.
    fn listed(path: &Path) -> io::Result<Listed> {
        let mut reader = Reader::<Listing>::start(());
        reader
            .send(Listing {
                path: Arc::from(path),
                detail: Detail::NamesOnly,
            })
            .unwrap();
        settle(&mut reader)
    }

    #[test]
    fn a_text_file_comes_back_as_text() {
        let tree = TempTree::new();
        let file = tree.make_file("one.txt", "hello\n");

        assert!(matches!(
            listed(&file).unwrap(),
            Listed::NotADirectory(Kind::Text)
        ));
    }

    /// An empty file has no NUL in it, and an editor is what opens one.
    #[test]
    fn an_empty_file_comes_back_as_text() {
        let tree = TempTree::new();
        let file = tree.make_file("empty", "");

        assert!(matches!(
            listed(&file).unwrap(),
            Listed::NotADirectory(Kind::Text)
        ));
    }

    #[test]
    fn a_file_holding_a_nul_comes_back_as_opaque() {
        let tree = TempTree::new();
        let file = tree.make_file("picture", "PNG\0\0\0");

        assert!(matches!(
            listed(&file).unwrap(),
            Listed::NotADirectory(Kind::Opaque)
        ));
    }

    /// The answer comes from the metadata alone. Opening a fifo nobody writes
    /// to never returns, so a read here would hang this test rather than fail
    /// it.
    #[test]
    fn a_fifo_comes_back_without_being_opened() {
        let tree = TempTree::new();
        let path = tree.at("pipe");
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is a NUL-terminated string that outlives the call, and
        // the mode is a plain permission bitmask.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

        assert!(matches!(
            listed(&path).unwrap(),
            Listed::NotADirectory(Kind::NotAFile)
        ));
    }

    /// A link is what it points at: entering it enters the file, and opening it
    /// opens the file.
    #[test]
    fn a_symlink_comes_back_as_what_it_points_at() {
        let tree = TempTree::new();
        let file = tree.make_file("one.txt", "hello\n");
        let link = tree.at("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();

        assert!(matches!(
            listed(&link).unwrap(),
            Listed::NotADirectory(Kind::Text)
        ));
    }

    #[test]
    fn a_directory_that_cannot_be_read_comes_back_as_the_error() {
        let tree = TempTree::new();

        assert!(listed(&tree.at("missing")).is_err());
    }
}
