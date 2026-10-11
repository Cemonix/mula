//! Whether two files hold the same bytes, read side by side.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

use crate::fs::ops::{Observer, halt_if_cancelled};

/// What two files came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Identical,
    /// Told from the metadata alone, before a byte is read.
    SizesDiffer {
        first: u64,
        second: u64,
    },
    /// The offset of the first byte that is not the same in both, counted
    /// from zero.
    DifferAt(u64),
}

/// How much of each file is read at a time.
const CHUNK: usize = 256 * 1024;

/// Compares the bytes of `first` and `second`, reporting every chunk of
/// `first` to `watcher` and asking it between chunks whether to stop, which
/// ends the comparison with `Interrupted`.
///
/// Both have to be regular files once links are followed. Anything else is
/// refused before it is opened: a fifo would hold the open for as long as
/// nobody writes to it.
pub fn compare(first: &Path, second: &Path, watcher: &mut dyn Observer) -> io::Result<Comparison> {
    let first_len = file_len(first)?;
    let second_len = file_len(second)?;
    if first_len != second_len {
        return Ok(Comparison::SizesDiffer {
            first: first_len,
            second: second_len,
        });
    }

    let mut first_file = File::open(first)?;
    let mut second_file = File::open(second)?;
    let mut first_buf = vec![0; CHUNK];
    let mut second_buf = vec![0; CHUNK];
    let mut offset = 0;
    loop {
        halt_if_cancelled(watcher)?;
        let first_read = fill(&mut first_file, &mut first_buf)?;
        let second_read = fill(&mut second_file, &mut second_buf)?;

        let common = first_read.min(second_read);
        let (a, b) = (&first_buf[..common], &second_buf[..common]);
        if a != b {
            let at = a
                .iter()
                .zip(b)
                .position(|(x, y)| x != y)
                .expect("two slices of one length that differ differ somewhere");
            return Ok(Comparison::DifferAt(offset + at as u64));
        }
        // A file that ends before the other has changed size since its
        // metadata was read, and differs where it ends.
        if first_read != second_read {
            return Ok(Comparison::DifferAt(offset + common as u64));
        }
        if first_read == 0 {
            return Ok(Comparison::Identical);
        }

        watcher.copied(first, first_read as u64);
        offset += first_read as u64;
    }
}

/// The size of the regular file at `path`, or `InvalidInput` naming it when
/// it is something else.
fn file_len(path: &Path) -> io::Result<u64> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a file", path.display()),
        ));
    }
    Ok(metadata.len())
}

/// Reads into `buf` until it is full or the file ends, and gives back how
/// much was read.
fn fill(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    while read < buf.len() {
        match file.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
    Ok(read)
}

#[cfg(test)]
mod compare_tests {
    use super::*;
    use crate::fs::temp_tree::TempTree;

    /// Counts what is reported and answers `cancelled` from a switch the test
    /// sets.
    #[derive(Default)]
    struct Watcher {
        bytes: u64,
        cancelled: bool,
    }

    impl Observer for Watcher {
        fn copied(&mut self, _path: &Path, bytes: u64) {
            self.bytes += bytes;
        }

        fn cancelled(&self) -> bool {
            self.cancelled
        }
    }

    fn compared(first: &[u8], second: &[u8]) -> Comparison {
        let t = TempTree::new();
        let (a, b) = (t.at("a"), t.at("b"));
        fs::write(&a, first).unwrap();
        fs::write(&b, second).unwrap();

        compare(&a, &b, &mut Watcher::default()).unwrap()
    }

    #[test]
    fn the_same_bytes_are_identical() {
        assert_eq!(compared(b"hello", b"hello"), Comparison::Identical);
        assert_eq!(compared(b"", b""), Comparison::Identical);
    }

    #[test]
    fn sizes_that_differ_are_told_before_reading() {
        assert_eq!(
            compared(b"hello", b"hello!"),
            Comparison::SizesDiffer {
                first: 5,
                second: 6
            }
        );
    }

    #[test]
    fn the_first_difference_is_found_past_the_first_chunk() {
        let first = vec![7; CHUNK + 10];
        let mut second = first.clone();
        second[CHUNK + 3] = 8;
        second[CHUNK + 9] = 8;

        assert_eq!(
            compared(&first, &second),
            Comparison::DifferAt(CHUNK as u64 + 3)
        );
    }

    #[test]
    fn every_byte_compared_is_reported_once() {
        let t = TempTree::new();
        let content = vec![1; CHUNK + 10];
        let (a, b) = (t.at("a"), t.at("b"));
        fs::write(&a, &content).unwrap();
        fs::write(&b, &content).unwrap();

        let mut watcher = Watcher::default();
        compare(&a, &b, &mut watcher).unwrap();

        assert_eq!(watcher.bytes, content.len() as u64);
    }

    #[test]
    fn a_directory_is_refused() {
        let t = TempTree::new();
        let file = t.make_file("a", "a");
        let dir = t.make_dir("sub");

        let error = compare(&file, &dir, &mut Watcher::default()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_cancelled_comparison_stops_with_interrupted() {
        let t = TempTree::new();
        let a = t.make_file("a", "same");
        let b = t.make_file("b", "same");
        let mut watcher = Watcher {
            cancelled: true,
            ..Watcher::default()
        };

        let error = compare(&a, &b, &mut watcher).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    }
}
