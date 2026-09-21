//! Putting a batch of items into one archive.
//!
//! The batch flattens the way a transfer does: every item goes in under its
//! own name, whatever directory it came from, and a directory takes its
//! contents with it. The archive is written beside the name it was asked for
//! and renamed onto it at the end, so a run that failed or was cancelled
//! leaves no half-written archive behind.

use std::{
    fs::{self, File, Metadata},
    io::{self, BufWriter, Seek, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

use flate2::{Compression, write::GzEncoder};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::fs::{
    archive::format::Format,
    ops::{Claim, Observer, remove_partial, staging_path},
};

/// What came of writing one archive.
#[derive(Debug)]
pub struct Packed {
    /// Items that were left out: a fifo, a socket or a device, none of which
    /// an archive has any way to carry.
    pub refused: Vec<String>,
}

/// Writes `items` into `archive`, in the format `archive` is named for.
///
/// The name is taken before anything is written and fails the whole run if it
/// is not free, so a typed name never replaces a file that is already there.
///
/// Every entry is reported to `watcher` with the bytes it contributed, and
/// `watcher` is asked before each one whether to carry on.
pub fn pack(
    items: &[impl AsRef<Path>],
    archive: &Path,
    format: Format,
    watcher: &mut dyn Observer,
) -> io::Result<Packed> {
    Claim::File.take(archive)?;

    let staging = staging_path(archive);
    let refused = write(items, &staging, format, watcher).inspect_err(|_| {
        remove_partial(&staging);
        remove_partial(archive);
    })?;

    fs::rename(&staging, archive).inspect_err(|_| remove_partial(&staging))?;
    Ok(Packed { refused })
}

fn write(
    items: &[impl AsRef<Path>],
    staging: &Path,
    format: Format,
    watcher: &mut dyn Observer,
) -> io::Result<Vec<String>> {
    let file = BufWriter::new(File::create_new(staging)?);

    match format {
        Format::Zip => {
            let mut sink = ToZip {
                zip: ZipWriter::new(file),
            };
            let refused = walk_all(items, &mut sink, watcher)?;
            sink.zip.finish().map_err(from_zip_error)?;
            Ok(refused)
        }
        Format::Tar => {
            let mut sink = ToTar::new(tar::Builder::new(file));
            let refused = walk_all(items, &mut sink, watcher)?;
            sink.builder.finish()?;
            Ok(refused)
        }
        Format::TarGz => {
            let mut sink = ToTar::new(tar::Builder::new(GzEncoder::new(
                file,
                Compression::default(),
            )));
            let refused = walk_all(items, &mut sink, watcher)?;
            // The tar has to be closed before the stream around it, or the
            // trailer never reaches the file.
            sink.builder.finish()?;
            sink.builder.into_inner()?.finish()?;
            Ok(refused)
        }
    }
}

/// Where a walk puts what it finds. The name is the path inside the archive,
/// always with `/` between its parts, whatever the filesystem uses.
trait Sink {
    fn directory(&mut self, name: &str, from: &Path) -> io::Result<()>;
    fn file(&mut self, name: &str, from: &Path, metadata: &Metadata) -> io::Result<()>;
    fn symlink(&mut self, name: &str, from: &Path, target: &Path) -> io::Result<()>;
}

fn walk_all(
    items: &[impl AsRef<Path>],
    sink: &mut dyn Sink,
    watcher: &mut dyn Observer,
) -> io::Result<Vec<String>> {
    let mut refused = Vec::new();

    for item in items {
        let item = item.as_ref();
        // Every item lands under its own name, which is the whole of what a
        // flattened batch keeps of where it came from.
        let Some(name) = item.file_name() else {
            refused.push(item.display().to_string());
            continue;
        };
        walk(item, &name.to_string_lossy(), sink, watcher, &mut refused)?;
    }

    Ok(refused)
}

fn walk(
    from: &Path,
    name: &str,
    sink: &mut dyn Sink,
    watcher: &mut dyn Observer,
    refused: &mut Vec<String>,
) -> io::Result<()> {
    halt_if_cancelled(watcher)?;

    let metadata = from.symlink_metadata()?;
    let kind = metadata.file_type();

    if kind.is_symlink() {
        sink.symlink(name, from, &fs::read_link(from)?)?;
    } else if kind.is_dir() {
        sink.directory(name, from)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let child = format!("{name}/{}", entry.file_name().to_string_lossy());
            walk(&entry.path(), &child, sink, watcher, refused)?;
        }
        return Ok(());
    } else if kind.is_file() {
        sink.file(name, from, &metadata)?;
    } else {
        refused.push(name.to_string());
        return Ok(());
    }

    watcher.entry_copied(from, metadata.len());
    Ok(())
}

struct ToZip<W: Write + Seek> {
    zip: ZipWriter<W>,
}

impl<W: Write + Seek> ToZip<W> {
    /// The header one entry goes in under. The permission bits travel; the
    /// mode a zip carries is advisory and read back masked.
    fn options(metadata: &Metadata) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(metadata.permissions().mode())
            .large_file(metadata.len() >= u32::MAX as u64)
    }
}

impl<W: Write + Seek> Sink for ToZip<W> {
    fn directory(&mut self, name: &str, from: &Path) -> io::Result<()> {
        let options = Self::options(&from.symlink_metadata()?);
        self.zip
            .add_directory(name, options)
            .map_err(from_zip_error)
    }

    fn file(&mut self, name: &str, from: &Path, metadata: &Metadata) -> io::Result<()> {
        self.zip
            .start_file(name, Self::options(metadata))
            .map_err(from_zip_error)?;
        io::copy(&mut File::open(from)?, &mut self.zip)?;
        Ok(())
    }

    fn symlink(&mut self, name: &str, from: &Path, target: &Path) -> io::Result<()> {
        let options = Self::options(&from.symlink_metadata()?);
        self.zip
            .add_symlink(name, target.to_string_lossy(), options)
            .map_err(from_zip_error)
    }
}

struct ToTar<W: Write> {
    builder: tar::Builder<W>,
}

impl<W: Write> ToTar<W> {
    /// A link goes in as a link rather than as what it points at, which is
    /// also what GNU tar does without `--dereference`.
    fn new(mut builder: tar::Builder<W>) -> Self {
        builder.follow_symlinks(false);
        Self { builder }
    }
}

/// One call covers all three: with symlinks left unfollowed,
/// `append_path_with_name` writes a directory header, a file with its
/// contents, or a link, according to what it finds.
impl<W: Write> Sink for ToTar<W> {
    fn directory(&mut self, name: &str, from: &Path) -> io::Result<()> {
        self.builder.append_path_with_name(from, name)
    }

    fn file(&mut self, name: &str, from: &Path, _metadata: &Metadata) -> io::Result<()> {
        self.builder.append_path_with_name(from, name)
    }

    fn symlink(&mut self, name: &str, from: &Path, _target: &Path) -> io::Result<()> {
        self.builder.append_path_with_name(from, name)
    }
}

fn halt_if_cancelled(watcher: &dyn Observer) -> io::Result<()> {
    if watcher.cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "packing cancelled",
        ));
    }
    Ok(())
}

fn from_zip_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(e) => e,
        other => io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::PathBuf};

    use zip::ZipArchive;

    use crate::fs::temp_tree::TempTree;

    #[derive(Default)]
    struct Watcher {
        entries: Vec<PathBuf>,
        cancel_after: Option<usize>,
    }

    impl Observer for Watcher {
        fn entry_copied(&mut self, path: &Path, _bytes: u64) {
            self.entries.push(path.to_path_buf());
        }

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|after| self.entries.len() >= after)
        }
    }

    /// The names a zip holds, sorted, so what went in can be compared to what
    /// was asked for.
    fn names_in(archive: &Path) -> Vec<String> {
        let mut zip = ZipArchive::new(File::open(archive).unwrap()).unwrap();
        let mut names: Vec<String> = (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().to_owned())
            .collect();
        names.sort();
        names
    }

    /// The batch flattens: two items from different directories both land at
    /// the top of the archive, under their own names.
    #[test]
    fn every_item_goes_in_under_its_own_name() {
        let tree = TempTree::new();
        let one = tree.make_file("here/one.txt", "one");
        let two = tree.make_file("elsewhere/deeper/two.txt", "two");
        let archive = tree.at("both.zip");

        pack(&[one, two], &archive, Format::Zip, &mut Watcher::default()).unwrap();

        assert_eq!(names_in(&archive), ["one.txt", "two.txt"]);
    }

    /// A directory takes its contents with it, and only the item itself is
    /// flattened.
    #[test]
    fn a_directory_keeps_what_is_under_it() {
        let tree = TempTree::new();
        tree.make_file("notes/deeper/two.txt", "two");
        tree.make_file("notes/one.txt", "one");
        let archive = tree.at("notes.zip");

        pack(
            &[tree.at("notes")],
            &archive,
            Format::Zip,
            &mut Watcher::default(),
        )
        .unwrap();

        assert_eq!(
            names_in(&archive),
            [
                "notes/",
                "notes/deeper/",
                "notes/deeper/two.txt",
                "notes/one.txt"
            ]
        );
    }

    /// The name was typed, so a file already under it is news rather than
    /// something to write over.
    #[test]
    fn a_name_that_is_taken_is_not_written_over() {
        let tree = TempTree::new();
        let one = tree.make_file("one.txt", "one");
        let archive = tree.make_file("taken.zip", "not an archive");

        let error = pack(&[one], &archive, Format::Zip, &mut Watcher::default()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&archive).unwrap(), "not an archive");
    }

    #[test]
    fn a_cancelled_pack_leaves_no_archive_behind() {
        let tree = TempTree::new();
        let one = tree.make_file("one.txt", "one");
        let two = tree.make_file("two.txt", "two");
        let three = tree.make_file("three.txt", "three");
        let archive = tree.at("some.zip");

        let mut watcher = Watcher {
            cancel_after: Some(1),
            ..Watcher::default()
        };
        let error = pack(&[one, two, three], &archive, Format::Zip, &mut watcher).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!archive.exists(), "a cancelled pack left an archive");
    }

    /// A fifo has no contents to store and would block whatever opened it.
    /// The rest of the batch is still worth archiving.
    #[test]
    fn a_fifo_is_left_out_rather_than_failing_the_batch() {
        let tree = TempTree::new();
        let one = tree.make_file("one.txt", "one");
        let pipe = tree.at("pipe");
        let name = CString::new(pipe.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is a NUL-terminated string that outlives the call,
        // and the mode is a plain permission bitmask.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
        let archive = tree.at("some.zip");

        let packed = pack(&[one, pipe], &archive, Format::Zip, &mut Watcher::default()).unwrap();

        assert_eq!(packed.refused, ["pipe"]);
        assert_eq!(names_in(&archive), ["one.txt"]);
    }
}
