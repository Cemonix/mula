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

use std::{io, path::Path, sync::Arc};

use crate::fs::{
    directory::Directory,
    reader::{Live, Outbox, ReadJob},
};

/// One directory to read, taken when the request was sent. Like every job, it
/// never reads back.
#[derive(Debug)]
pub struct Listing {
    pub path: Arc<Path>,
}

impl ReadJob for Listing {
    type Config = ();
    /// The failure travels with the answer. A pane has no other way home, and
    /// a directory that cannot be read is news it has to report.
    type Msg = io::Result<Directory>;

    /// `live` goes unread: the reader already skips a request that was
    /// replaced before it started, and a listing reports nothing on its way,
    /// so there is no point between those two to check.
    fn run(self, _config: &Self::Config, _live: &Live<'_>, out: &Outbox<'_, Self::Msg>) {
        out.send(Directory::read(self.path));
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    use std::{
        thread,
        time::{Duration, Instant},
    };

    use crate::fs::{reader::Reader, temp_tree::TempTree};

    /// Drains the way the main loop does until the answer arrives, and gives
    /// up rather than hanging if it never does.
    fn settle(reader: &mut Reader<Listing>) -> io::Result<Directory> {
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
        let mut reader = Reader::<Listing>::start(());
        reader
            .send(Listing {
                path: Arc::from(tree.path()),
            })
            .unwrap();

        let directory = settle(&mut reader).unwrap();

        // The parent leads the listing, so the two files sit behind it.
        assert_eq!(directory.len(), 3);
    }

    #[test]
    fn a_file_comes_back_as_not_a_directory() {
        let tree = TempTree::new();
        let file = tree.make_file("one.txt", "");
        let mut reader = Reader::<Listing>::start(());
        reader
            .send(Listing {
                path: Arc::from(file.as_path()),
            })
            .unwrap();

        let error = settle(&mut reader).unwrap_err();

        // The pane tells "that was a file" from a real failure by this kind
        // alone, so what the platform actually returns is pinned here rather
        // than assumed.
        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
    }

    #[test]
    fn a_directory_that_cannot_be_read_comes_back_as_the_error() {
        let tree = TempTree::new();
        let mut reader = Reader::<Listing>::start(());
        reader
            .send(Listing {
                path: Arc::from(tree.at("missing").as_path()),
            })
            .unwrap();

        assert!(settle(&mut reader).is_err());
    }
}
