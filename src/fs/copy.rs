//! Copies one regular file, reporting its bytes to an [`Observer`] as they
//! land and stopping between chunks when it is cancelled.
//!
//! On macOS a clone is tried first and chunks are what follows when it is
//! declined; on Linux the chunks go through `copy_file_range` while the
//! filesystems take it, and through a buffer of ours where they do not.

use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[cfg(target_os = "linux")]
use std::{os::fd::AsRawFd, ptr};

use crate::fs::ops::{Observer, halt_if_cancelled};

/// How much is copied between two reports, and between two checks for a
/// cancel.
const CHUNK: usize = 1 << 20;

/// Copies the regular file `from` to `to`, creating `to` or truncating what is
/// there, and gives it the permission bits of `from` whatever the umask says.
/// On macOS the rest of what `copyfile` carries comes along too: ACLs,
/// extended attributes, flags and times.
///
/// Reports every chunk to `watcher`, or the whole file at once when it was
/// cloned. Fails with `Interrupted` when `watcher` is cancelled between two
/// chunks, leaving `to` part written for the caller to remove.
pub fn copy_file(from: &Path, to: &Path, watcher: &mut dyn Observer) -> io::Result<()> {
    let reader = File::open(from)?;
    let metadata = reader.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", from.display()),
        ));
    }

    #[cfg(target_os = "macos")]
    if let macos::Cloned::Whole = macos::clone(&reader, to)? {
        watcher.copied(from, metadata.len());
        return Ok(());
    }

    let writer = create(to, &metadata)?;
    copy_chunks(&reader, &writer, from, watcher)?;

    #[cfg(target_os = "macos")]
    macos::copy_metadata(&reader, &writer)?;

    Ok(())
}

/// Opens `to` for writing with the permission bits of `metadata`. They are
/// set again once it is open, since the mode given to `open` passes through
/// the umask and is ignored for a file that was already there.
fn create(to: &Path, metadata: &Metadata) -> io::Result<File> {
    let permissions = metadata.permissions();
    let writer = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(permissions.mode())
        .open(to)?;
    writer.set_permissions(permissions)?;
    Ok(writer)
}

/// Moves everything from the reader's offset to its end into the writer, one
/// chunk at a time, reporting each chunk under `from` and checking `watcher`
/// for a cancel before each one.
fn copy_chunks(
    reader: &File,
    writer: &File,
    from: &Path,
    watcher: &mut dyn Observer,
) -> io::Result<()> {
    let mut mover = Mover::new();
    loop {
        halt_if_cancelled(watcher)?;
        match mover.chunk(reader, writer)? {
            0 => return Ok(()),
            moved => watcher.copied(from, moved as u64),
        }
    }
}

/// How the bytes of one chunk get from the reader to the writer. Both work
/// from each file's own offset, so one can take over from the other part way.
enum Mover {
    /// `copy_file_range`, which leaves the data in the kernel and lets a
    /// filesystem that can share blocks share them.
    #[cfg(target_os = "linux")]
    Kernel,
    /// `read` into a buffer and `write` out of it.
    Buffer(Vec<u8>),
}

impl Mover {
    #[cfg(target_os = "linux")]
    fn new() -> Self {
        Mover::Kernel
    }

    #[cfg(not(target_os = "linux"))]
    fn new() -> Self {
        Mover::buffer()
    }

    fn buffer() -> Self {
        Mover::Buffer(vec![0; CHUNK])
    }

    /// Moves at most one chunk and gives back how much it moved, which is
    /// zero at the end of the reader. A `copy_file_range` the filesystems
    /// decline turns this into a buffer for the rest of the file.
    fn chunk(&mut self, mut reader: &File, mut writer: &File) -> io::Result<usize> {
        match self {
            #[cfg(target_os = "linux")]
            Mover::Kernel => match copy_range(reader, writer) {
                Err(e) if declined(&e) => {
                    *self = Mover::buffer();
                    self.chunk(reader, writer)
                }
                other => other,
            },
            Mover::Buffer(buffer) => {
                let read = loop {
                    match reader.read(buffer) {
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => (),
                        other => break other?,
                    }
                };
                writer.write_all(&buffer[..read])?;
                Ok(read)
            }
        }
    }
}

/// One `copy_file_range` of at most a chunk, from the reader's offset to the
/// writer's, advancing both.
#[cfg(target_os = "linux")]
fn copy_range(reader: &File, writer: &File) -> io::Result<usize> {
    loop {
        // SAFETY: both descriptors belong to files borrowed for the length of
        // the call, and null offsets make the kernel use and advance each
        // file's own.
        let moved = unsafe {
            libc::copy_file_range(
                reader.as_raw_fd(),
                ptr::null_mut(),
                writer.as_raw_fd(),
                ptr::null_mut(),
                CHUNK,
                0,
            )
        };
        if moved >= 0 {
            return Ok(moved as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Whether `copy_file_range` failed for want of support rather than for
/// anything a buffer would also run into: a kernel without it, two
/// filesystems it cannot cross, or a filesystem that does not implement it.
/// The same set `std::fs::copy` falls back on.
#[cfg(target_os = "linux")]
fn declined(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS | libc::EXDEV | libc::EINVAL | libc::EOPNOTSUPP | libc::EPERM)
    )
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        ffi::CString,
        fs::File,
        io,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
        path::Path,
        ptr,
    };

    /// What came of asking for a clone.
    pub enum Cloned {
        /// `to` holds the whole file, metadata and all.
        Whole,
        /// Nothing was written, and the file has to be copied.
        Declined,
    }

    /// Clones the file open in `reader` to a new `to`. Declined, with nothing
    /// written, when the volume cannot clone, `to` is on another volume, or
    /// `to` already exists — the cases `std::fs::copy` falls back on.
    pub fn clone(reader: &File, to: &Path) -> io::Result<Cloned> {
        let to = CString::new(to.as_os_str().as_bytes())?;

        // SAFETY: the descriptor belongs to a file borrowed for the length of
        // the call, and `to` is a NUL-terminated string that outlives it.
        let result =
            unsafe { libc::fclonefileat(reader.as_raw_fd(), libc::AT_FDCWD, to.as_ptr(), 0) };
        if result == 0 {
            return Ok(Cloned::Whole);
        }

        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOTSUP | libc::EXDEV | libc::EEXIST | libc::ENOSYS) => Ok(Cloned::Declined),
            _ => Err(error),
        }
    }

    /// Gives `writer` what `copyfile` calls the metadata of `reader`: the
    /// mode, owner and ACL, the extended attributes, the flags and the times.
    /// Run once the data is in, so the data does not move the times again.
    pub fn copy_metadata(reader: &File, writer: &File) -> io::Result<()> {
        // SAFETY: both descriptors belong to files borrowed for the length of
        // the call, and a null state is one `fcopyfile` accepts.
        let result = unsafe {
            libc::fcopyfile(
                reader.as_raw_fd(),
                writer.as_raw_fd(),
                ptr::null_mut(),
                libc::COPYFILE_METADATA,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;

    use std::{fs, path::PathBuf};

    use crate::fs::temp_tree::TempTree;

    /// Records every report and answers `cancelled` once it has heard
    /// `cancel_after` of them.
    #[derive(Default)]
    struct Watcher {
        reports: Vec<(PathBuf, u64)>,
        cancel_after: Option<usize>,
    }

    impl Observer for Watcher {
        fn copied(&mut self, path: &Path, bytes: u64) {
            self.reports.push((path.to_path_buf(), bytes));
        }

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|after| self.reports.len() >= after)
        }
    }

    /// Two and a half chunks of bytes that differ from one chunk to the next,
    /// so a chunk written twice or left out would show.
    fn contents() -> Vec<u8> {
        (0..CHUNK * 5 / 2).map(|at| (at % 251) as u8).collect()
    }

    /// Runs the chunked half of [`copy_file`] alone. On a volume that clones,
    /// `copy_file` never reaches it.
    fn copy_in_chunks(from: &Path, to: &Path, watcher: &mut Watcher) -> io::Result<()> {
        let reader = File::open(from)?;
        let writer = create(to, &reader.metadata()?)?;
        copy_chunks(&reader, &writer, from, watcher)
    }

    #[test]
    fn a_file_bigger_than_a_chunk_is_reported_chunk_by_chunk() {
        let t = TempTree::new();
        let from = t.at("big");
        fs::write(&from, contents()).unwrap();

        let mut watcher = Watcher::default();
        copy_in_chunks(&from, &t.at("copy"), &mut watcher).unwrap();

        assert_eq!(watcher.reports.len(), 3, "{:?}", watcher.reports);
        let reported: u64 = watcher.reports.iter().map(|(_, bytes)| bytes).sum();
        assert_eq!(reported, contents().len() as u64);
        assert_eq!(fs::read(t.at("copy")).unwrap(), contents());
    }

    #[test]
    fn a_cancel_stops_the_copy_between_two_chunks() {
        let t = TempTree::new();
        let from = t.at("big");
        fs::write(&from, contents()).unwrap();

        let mut watcher = Watcher {
            cancel_after: Some(1),
            ..Watcher::default()
        };
        let err = copy_in_chunks(&from, &t.at("copy"), &mut watcher).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        assert_eq!(watcher.reports.len(), 1);
        assert_eq!(
            fs::metadata(t.at("copy")).unwrap().len(),
            watcher.reports[0].1,
            "nothing past the first chunk may have been written"
        );
    }

    #[test]
    fn a_copy_keeps_the_permission_bits_whatever_the_umask() {
        let t = TempTree::new();
        let from = t.at("script");
        fs::write(&from, "#!/bin/sh\n").unwrap();
        // Group and other write are what a usual umask of 022 takes away.
        fs::set_permissions(&from, fs::Permissions::from_mode(0o777)).unwrap();

        copy_in_chunks(&from, &t.at("chunked"), &mut Watcher::default()).unwrap();
        copy_file(&from, &t.at("whole"), &mut Watcher::default()).unwrap();

        for copy in ["chunked", "whole"] {
            let mode = fs::metadata(t.at(copy)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o777, "{copy}");
        }
    }

    #[test]
    fn a_copy_reports_what_the_file_weighs() {
        let t = TempTree::new();
        let from = t.at("big");
        fs::write(&from, contents()).unwrap();

        let mut watcher = Watcher::default();
        copy_file(&from, &t.at("copy"), &mut watcher).unwrap();

        let reported: u64 = watcher.reports.iter().map(|(_, bytes)| bytes).sum();
        assert_eq!(reported, contents().len() as u64);
        assert_eq!(fs::read(t.at("copy")).unwrap(), contents());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_chunked_copy_on_macos_keeps_the_times_and_the_extended_attributes() {
        let t = TempTree::new();
        let from = t.at("dated");
        fs::write(&from, "old").unwrap();
        let then =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        File::options()
            .write(true)
            .open(&from)
            .unwrap()
            .set_modified(then)
            .unwrap();
        let name = std::ffi::CString::new("user.mula").unwrap();
        let path = std::ffi::CString::new(from.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let set =
            unsafe { libc::setxattr(path.as_ptr(), name.as_ptr(), b"v".as_ptr().cast(), 1, 0, 0) };
        assert_eq!(set, 0);

        let to = t.at("copy");
        let reader = File::open(&from).unwrap();
        let writer = create(&to, &reader.metadata().unwrap()).unwrap();
        copy_chunks(&reader, &writer, &from, &mut Watcher::default()).unwrap();
        macos::copy_metadata(&reader, &writer).unwrap();

        assert_eq!(fs::metadata(&to).unwrap().modified().unwrap(), then);
        let path = std::ffi::CString::new(to.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: as above, and a null buffer asks only for the size.
        let size =
            unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
        assert_eq!(size, 1);
    }
}
