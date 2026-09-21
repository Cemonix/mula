//! Taking an archive apart into a directory of its own.
//!
//! Nothing that was already on the disk is written to. The entries go into a
//! staging directory beside the destination, and the last step of a run that
//! got all the way through is one `rename` giving that directory its name. A
//! run that failed or was cancelled takes the staging directory with it, so
//! the destination holds either the whole archive or nothing of it.

use std::{
    cell::Cell,
    fs::{self, File},
    io::{self, BufReader, Read},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use flate2::read::GzDecoder;
use zip::ZipArchive;

use crate::fs::{
    archive::format::{self, Format},
    ops::{Claim, Observer, claim_or_number, remove_partial, staging_path},
    preview,
};

/// How much of an archive is read to tell which one it is. A gzip stream is
/// decompressed from this prefix, and one tar header is 512 bytes, so the
/// margin is for the compressed form of those.
const SNIFFED_BYTES: usize = 8 * 1024;

/// What came of unpacking one archive.
#[derive(Debug)]
pub struct Unpacked {
    /// The directory the archive went into. Not always the name that was
    /// asked for, so it has to be said out loud.
    pub into: PathBuf,
    /// Entries the archive held that were not written: a path leading out of
    /// the destination, a name the archive uses twice, or a kind of file an
    /// archive has no business carrying.
    pub refused: Vec<String>,
}

/// Unpacks `archive` into a directory of its own under `into`.
///
/// The directory is named after the archive, less its suffix, and takes a
/// numbered name when that one is taken. An archive holding one directory and
/// nothing beside it is lifted out of its wrapper, so `notes.tar.gz` holding
/// `notes/` unpacks as `notes/` rather than as `notes/notes/`.
///
/// Every entry is reported to `watcher` with the compressed bytes it stood
/// for, and `watcher` is asked before each one whether to carry on.
pub fn unpack(archive: &Path, into: &Path, watcher: &mut dyn Observer) -> io::Result<Unpacked> {
    let format = format::sniff(&preview::read_prefix(archive, SNIFFED_BYTES)?)
        .ok_or_else(|| unreadable(archive))?;

    let name = archive
        .file_name()
        .ok_or_else(|| unreadable(archive))?
        .to_string_lossy()
        .into_owned();
    let staging = staging_path(&into.join(&name));
    fs::create_dir(&staging)?;

    let refused = match format {
        Format::Zip => from_zip(archive, &staging, watcher),
        Format::Tar => {
            let counted = Counted::new(open(archive)?);
            let read = counted.taken();
            let mut consumed = Consumed::new(watcher, &read);
            from_tar(&mut tar::Archive::new(counted), &staging, &mut consumed)
        }
        Format::TarGz => {
            let counted = Counted::new(open(archive)?);
            let read = counted.taken();
            let mut consumed = Consumed::new(watcher, &read);
            from_tar(
                &mut tar::Archive::new(GzDecoder::new(counted)),
                &staging,
                &mut consumed,
            )
        }
    };

    let refused = refused.inspect_err(|_| remove_partial(&staging))?;
    let into =
        settle(&staging, into, format::stem(&name)).inspect_err(|_| remove_partial(&staging))?;
    Ok(Unpacked { into, refused })
}

pub fn open(archive: &Path) -> io::Result<BufReader<File>> {
    Ok(BufReader::new(File::open(archive)?))
}

/// Gives the unpacked entries their final home and returns where that is.
///
/// A staging directory holding one directory and nothing else is the archive's
/// own wrapper, and what is renamed is the wrapper's contents under its own
/// name. Renaming a directory onto the empty one `claim_or_number` just
/// created is what takes the name in one step.
fn settle(staging: &Path, into: &Path, stem: &str) -> io::Result<PathBuf> {
    let (source, name) = match only_directory(staging)? {
        Some(wrapper) => {
            let name = wrapper.file_name().unwrap_or_default().to_os_string();
            (wrapper, name)
        }
        None => (staging.to_path_buf(), stem.into()),
    };

    let claimed = claim_or_number(&into.join(name), Claim::Directory)?;
    fs::rename(&source, &claimed)?;
    // Lifting the wrapper out leaves the staging directory behind it empty.
    if source != staging {
        let _ = fs::remove_dir(staging);
    }
    Ok(claimed)
}

/// The one directory `path` holds, or `None` when it holds anything else:
/// nothing, more than one entry, or a single entry that is not a directory.
fn only_directory(path: &Path) -> io::Result<Option<PathBuf>> {
    let mut entries = fs::read_dir(path)?;
    let Some(first) = entries.next().transpose()? else {
        return Ok(None);
    };
    if entries.next().is_some() || !first.file_type()?.is_dir() {
        return Ok(None);
    }
    Ok(Some(first.path()))
}

fn from_zip(archive: &Path, root: &Path, watcher: &mut dyn Observer) -> io::Result<Vec<String>> {
    let mut zip = ZipArchive::new(open(archive)?).map_err(from_zip_error)?;
    let mut refused = Vec::new();

    for index in 0..zip.len() {
        halt_if_cancelled(watcher)?;

        let mut entry = zip.by_index(index).map_err(from_zip_error)?;
        let name = entry.name().to_owned();
        let weight = entry.compressed_size();

        // `enclosed_name` is the crate's own answer to a name that is
        // absolute, holds a NUL, or climbs out with `..`.
        let Some(relative) = entry.enclosed_name() else {
            refused.push(name);
            continue;
        };
        let at = root.join(&relative);
        let Some(parent) = relative.parent() else {
            refused.push(name);
            continue;
        };
        make_way(root, parent)?;

        let written = if entry.is_dir() {
            match fs::create_dir(&at) {
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                other => other,
            }
        } else if entry.is_symlink() {
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            if !inside(root, at.parent().unwrap_or(root), Path::new(&target)) {
                refused.push(name);
                continue;
            }
            std::os::unix::fs::symlink(&target, &at)
        } else if entry.is_file() {
            let mode = entry_mode(&entry);
            write_file(&mut entry, &at, mode)
        } else {
            refused.push(name);
            continue;
        };

        match written {
            Ok(()) => watcher.entry_copied(&at, weight),
            // A name the archive uses twice. Whichever entry came second is
            // left out rather than put over the first: inside a staging
            // directory the only thing in the way is the run itself.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => refused.push(name),
            Err(e) => return Err(e),
        }
    }

    Ok(refused)
}

/// The permission bits of a zip entry, or none when it carries none.
///
/// Masking to the low nine bits is what drops setuid, setgid and the sticky
/// bit, which live above them.
fn entry_mode<R: Read>(entry: &zip::read::ZipFile<'_, R>) -> Option<u32> {
    entry.unix_mode().map(|mode| mode & 0o777)
}

/// Writes one entry's bytes to a name nothing holds yet.
fn write_file(from: &mut impl Read, at: &Path, mode: Option<u32>) -> io::Result<()> {
    let mut file = File::create_new(at)?;
    io::copy(from, &mut file)?;
    if let Some(mode) = mode {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn from_tar<R: Read>(
    archive: &mut tar::Archive<R>,
    root: &Path,
    watcher: &mut dyn Observer,
) -> io::Result<Vec<String>> {
    // What an archive says about permissions is not applied, which is what
    // keeps a setuid bit out of a tarball anyone can write. Nothing is
    // overwritten either: inside a staging directory only the run itself
    // could be in the way, and an archive naming one path twice is refused
    // rather than left with whichever entry came last.
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_overwrite(false);

    let mut refused = Vec::new();
    for entry in archive.entries()? {
        halt_if_cancelled(watcher)?;

        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().into_owned();
        let at = root.join(entry.path()?);

        // `unpack_in` canonicalises the destination's parent for every entry,
        // so a link written earlier cannot be the way out of `root`.
        match entry.unpack_in(root) {
            Ok(true) => watcher.entry_copied(&at, 0),
            Ok(false) => refused.push(name),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => refused.push(name),
            Err(e) => return Err(e),
        }
    }

    Ok(refused)
}

/// Creates the directories `relative` needs under `root`, one component at a
/// time, refusing to walk through anything already there that is not a
/// directory.
///
/// This is what lets the containment checks be lexical: with no symlink among
/// the components of a path, a name that spells out to somewhere under `root`
/// also resolves to somewhere under it.
fn make_way(root: &Path, relative: &Path) -> io::Result<()> {
    let mut at = root.to_path_buf();
    for component in relative.components() {
        at.push(component);
        match fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.is_dir() => (),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!("{} is in the way", at.display()),
                ));
            }
            Err(_) => fs::create_dir(&at)?,
        }
    }
    Ok(())
}

/// Whether following `target` from the directory `from` lands under `root`.
///
/// Worked out by spelling the path rather than by resolving it: a link may
/// point at something that is not there yet, so there is nothing to
/// canonicalise. [`make_way`] is what makes the spelling trustworthy.
fn inside(root: &Path, from: &Path, target: &Path) -> bool {
    let mut at = from.to_path_buf();
    for component in target.components() {
        match component {
            Component::Normal(part) => at.push(part),
            Component::CurDir => (),
            Component::ParentDir => {
                if !at.pop() {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    at.starts_with(root)
}

fn halt_if_cancelled(watcher: &dyn Observer) -> io::Result<()> {
    if watcher.cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "unpacking cancelled",
        ));
    }
    Ok(())
}

fn unreadable(archive: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} is not an archive Mula can open", archive.display()),
    )
}

pub fn from_zip_error(error: zip::result::ZipError) -> io::Error {
    match error {
        zip::result::ZipError::Io(e) => e,
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    }
}

/// A reader that remembers how many bytes have been taken out of it.
///
/// A compressed archive is weighed by its own size on disk, so what moves the
/// bar is the position in the file rather than the bytes that come out of the
/// decompressor. The count is shared because the reader itself disappears
/// into the decompressor and the tar reader above it.
struct Counted<R> {
    inner: R,
    taken: Rc<Cell<u64>>,
}

impl<R: Read> Counted<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            taken: Rc::new(Cell::new(0)),
        }
    }

    fn taken(&self) -> Rc<Cell<u64>> {
        Rc::clone(&self.taken)
    }
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.taken.set(self.taken.get() + read as u64);
        Ok(read)
    }
}

/// Reports what an entry cost in the archive file rather than what it weighs
/// unpacked, by handing on the growth of the count since the last entry.
///
/// A tar is read from front to back, so there is no per-entry figure to ask
/// for the way a zip's central directory holds one.
struct Consumed<'a> {
    watcher: &'a mut dyn Observer,
    read: &'a Rc<Cell<u64>>,
    reported: u64,
}

impl<'a> Consumed<'a> {
    fn new(watcher: &'a mut dyn Observer, read: &'a Rc<Cell<u64>>) -> Self {
        Self {
            watcher,
            read,
            reported: 0,
        }
    }
}

impl Observer for Consumed<'_> {
    fn entry_copied(&mut self, path: &Path, _bytes: u64) {
        let taken = self.read.get();
        self.watcher.entry_copied(path, taken - self.reported);
        self.reported = taken;
    }

    fn cancelled(&self) -> bool {
        self.watcher.cancelled()
    }
}

#[cfg(test)]
mod unpack_tests {
    use super::*;

    use std::{io::Write, os::unix::fs::symlink};

    use zip::write::SimpleFileOptions;

    use crate::fs::{archive::pack::pack, temp_tree::TempTree};

    /// Counts what an unpack reports and answers `cancelled` from a switch the
    /// test sets, so both halves of the trait can be driven on their own.
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

    /// The paths under `root`, relative to it and sorted, so a tree can be
    /// compared to what it was made from.
    fn tree_of(root: &Path) -> Vec<String> {
        let mut found = Vec::new();
        collect(root, root, &mut found);
        found.sort();
        found
    }

    fn collect(root: &Path, at: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            found.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.symlink_metadata().unwrap().is_dir() {
                collect(root, &path, found);
            }
        }
    }

    /// Packs a tree and unpacks it again, which is the only pair of calls that
    /// can show a format carries what it was given.
    fn round_trip(format: Format, name: &str) {
        let source = TempTree::new();
        source.make_file("notes/one.txt", "one");
        source.make_file("notes/deeper/two.txt", "two");
        symlink("one.txt", source.at("notes/link")).unwrap();

        let destination = TempTree::new();
        let archive = destination.at(name);
        pack(
            &[source.at("notes")],
            &archive,
            format,
            &mut Watcher::default(),
        )
        .unwrap();

        let unpacked = unpack(&archive, destination.path(), &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.into, destination.at("notes"));
        assert!(unpacked.refused.is_empty());
        assert_eq!(
            tree_of(&unpacked.into),
            ["deeper", "deeper/two.txt", "link", "one.txt"]
        );
        assert_eq!(
            fs::read_to_string(unpacked.into.join("one.txt")).unwrap(),
            "one"
        );
        assert!(
            unpacked
                .into
                .join("link")
                .symlink_metadata()
                .unwrap()
                .is_symlink(),
            "a link came back as what it pointed at"
        );
    }

    #[test]
    fn a_zip_carries_a_tree_there_and_back() {
        round_trip(Format::Zip, "notes.zip");
    }

    #[test]
    fn a_tar_carries_a_tree_there_and_back() {
        round_trip(Format::Tar, "notes.tar");
    }

    #[test]
    fn a_gzipped_tar_carries_a_tree_there_and_back() {
        round_trip(Format::TarGz, "notes.tar.gz");
    }

    /// Writes a zip of `entries`, each a name and its contents, without going
    /// through `pack` — which is the only way to build the names `pack` would
    /// never write.
    fn zip_of(at: &Path, entries: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(at).unwrap());
        for (name, contents) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn an_archive_of_loose_entries_unpacks_under_the_name_of_the_archive() {
        let tree = TempTree::new();
        zip_of(
            &tree.at("loose.zip"),
            &[("one.txt", "one"), ("two.txt", "two")],
        );

        let unpacked = unpack(&tree.at("loose.zip"), tree.path(), &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.into, tree.at("loose"));
        assert_eq!(tree_of(&unpacked.into), ["one.txt", "two.txt"]);
    }

    /// The wrapper is lifted out, so a tarball of `notes/` does not unpack as
    /// `notes/notes/`.
    #[test]
    fn an_archive_holding_one_directory_unpacks_as_that_directory() {
        let tree = TempTree::new();
        zip_of(&tree.at("wrapped.zip"), &[("inside/one.txt", "one")]);

        let unpacked = unpack(
            &tree.at("wrapped.zip"),
            tree.path(),
            &mut Watcher::default(),
        )
        .unwrap();

        assert_eq!(unpacked.into, tree.at("inside"));
        assert_eq!(tree_of(&unpacked.into), ["one.txt"]);
    }

    /// Nothing that was already there is written to, whatever it is called.
    #[test]
    fn an_archive_whose_name_is_taken_unpacks_beside_it() {
        let tree = TempTree::new();
        tree.make_file("loose/mine.txt", "mine");
        zip_of(
            &tree.at("loose.zip"),
            &[("one.txt", "one"), ("two.txt", "two")],
        );

        let unpacked = unpack(&tree.at("loose.zip"), tree.path(), &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.into, tree.at("loose(1)"));
        assert_eq!(tree_of(&tree.at("loose")), ["mine.txt"]);
    }

    /// An entry climbing out of the destination is left out, and the walk goes
    /// on: the rest of the archive is still worth having.
    #[test]
    fn an_entry_that_climbs_out_of_the_destination_is_refused() {
        let tree = TempTree::new();
        let outside = tree.make_dir("outside");
        let into = tree.make_dir("into");
        zip_of(
            &tree.at("evil.zip"),
            &[("../../outside/taken.txt", "taken"), ("fine.txt", "fine")],
        );

        let unpacked = unpack(&tree.at("evil.zip"), &into, &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.refused, ["../../outside/taken.txt"]);
        assert_eq!(tree_of(&unpacked.into), ["fine.txt"]);
        assert!(tree_of(&outside).is_empty(), "a file was written outside");
    }

    /// A link is only ever written when it points back inside, so nothing a
    /// later entry writes can be carried out through it.
    #[test]
    fn a_link_pointing_out_of_the_archive_is_refused() {
        let tree = TempTree::new();
        let into = tree.make_dir("into");

        let path = tree.at("linked.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = SimpleFileOptions::default();
        zip.add_symlink("out", "../../../etc", options).unwrap();
        zip.add_symlink("in", "sibling", options).unwrap();
        zip.finish().unwrap();

        let unpacked = unpack(&path, &into, &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.refused, ["out"]);
        assert_eq!(tree_of(&unpacked.into), ["in"]);
    }

    /// The name is used twice, and the second one is not put over the first.
    ///
    /// Written as a tar because `ZipWriter` refuses a duplicate name outright,
    /// so the archive this is about cannot be built with it. A tar names its
    /// entries one at a time and has nothing to compare them against.
    #[test]
    fn an_archive_naming_one_path_twice_writes_only_the_first() {
        let tree = TempTree::new();
        let source = tree.make_file("source/one.txt", "first");

        let path = tree.at("twice.tar");
        let mut builder = tar::Builder::new(File::create(&path).unwrap());
        builder.append_path_with_name(&source, "one.txt").unwrap();
        fs::write(&source, "second").unwrap();
        builder.append_path_with_name(&source, "one.txt").unwrap();
        builder.finish().unwrap();

        let unpacked = unpack(&path, tree.path(), &mut Watcher::default()).unwrap();

        assert_eq!(unpacked.refused, ["one.txt"]);
        assert_eq!(
            fs::read_to_string(unpacked.into.join("one.txt")).unwrap(),
            "first"
        );
    }

    /// An unpack is the whole archive or none of it, so what a cancelled one
    /// had already written goes away with it.
    #[test]
    fn a_cancelled_unpack_leaves_nothing_behind() {
        let tree = TempTree::new();
        let into = tree.make_dir("into");
        zip_of(
            &tree.at("many.zip"),
            &[
                ("one.txt", "one"),
                ("two.txt", "two"),
                ("three.txt", "three"),
            ],
        );

        let mut watcher = Watcher {
            cancel_after: Some(1),
            ..Watcher::default()
        };
        let error = unpack(&tree.at("many.zip"), &into, &mut watcher).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(
            tree_of(&into).is_empty(),
            "a cancelled unpack left something"
        );
    }

    #[test]
    fn a_file_that_is_not_an_archive_is_not_unpacked() {
        let tree = TempTree::new();
        let file = tree.make_file("notes.txt", "nothing like an archive");

        let error = unpack(&file, tree.path(), &mut Watcher::default()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    /// The bits above the low nine are where setuid lives, and an archive
    /// anyone can write is not where a program gets them.
    #[test]
    fn the_setuid_bit_of_an_entry_is_not_carried_over() {
        let tree = TempTree::new();
        let path = tree.at("setuid.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file(
            "program",
            SimpleFileOptions::default().unix_permissions(0o4755),
        )
        .unwrap();
        zip.write_all(b"#!/bin/sh\n").unwrap();
        zip.finish().unwrap();

        let unpacked = unpack(&path, tree.path(), &mut Watcher::default()).unwrap();

        let mode = fs::metadata(unpacked.into.join("program"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o755);
    }
}
