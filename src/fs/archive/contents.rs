//! What an archive holds, read without taking it apart.
//!
//! This is the whole of what Mula can say about the inside of an archive
//! without unpacking it. The names come back as the archive spells them —
//! nothing is joined onto a path — so there is nothing here that could be
//! handed to a program as a file.

use std::{io, path::Path};

use flate2::read::GzDecoder;
use zip::ZipArchive;

use crate::fs::{
    archive::{format::Format, unpack::open},
    reader::Live,
};

/// One name an archive holds, and what it is. There is no path: an entry
/// inside an archive is not anywhere until it is unpacked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The size unpacked, which is what a reader wants to know.
    pub size: u64,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    Symlink,
    File,
}

/// The names an archive holds, as far as they were read.
#[derive(Debug)]
pub struct Contents {
    pub entries: Vec<Entry>,
    /// Set when the archive goes on past what was read.
    pub clipped: bool,
}

/// Reads the names in `archive`, stopping at `limit` of them.
///
/// A zip keeps an index at its end, so this is one seek and a walk of it.
/// A tar keeps none, so the whole stream is read to see what is in it — and
/// for a compressed one that means decompressing it, which is why `live` is
/// asked between entries and why the limit is there at all.
pub fn read(
    archive: &Path,
    format: Format,
    limit: usize,
    live: &Live<'_>,
) -> io::Result<Option<Contents>> {
    match format {
        Format::Zip => from_zip(archive, limit),
        Format::Tar => from_tar(&mut tar::Archive::new(open(archive)?), limit, live),
        Format::TarGz => from_tar(
            &mut tar::Archive::new(GzDecoder::new(open(archive)?)),
            limit,
            live,
        ),
    }
}

fn from_zip(archive: &Path, limit: usize) -> io::Result<Option<Contents>> {
    let mut zip = ZipArchive::new(open(archive)?).map_err(super::unpack::from_zip_error)?;
    let mut entries = Vec::new();

    for index in 0..zip.len().min(limit) {
        let entry = zip
            .by_index_raw(index)
            .map_err(super::unpack::from_zip_error)?;
        entries.push(Entry {
            name: entry.name().to_owned(),
            size: entry.size(),
            kind: if entry.is_dir() {
                EntryKind::Directory
            } else if entry.is_symlink() {
                EntryKind::Symlink
            } else {
                EntryKind::File
            },
        });
    }

    let clipped = zip.len() > entries.len();
    Ok(Some(Contents { entries, clipped }))
}

fn from_tar<R: io::Read>(
    archive: &mut tar::Archive<R>,
    limit: usize,
    live: &Live<'_>,
) -> io::Result<Option<Contents>> {
    let mut entries = Vec::new();
    let mut clipped = false;

    for entry in archive.entries()? {
        // Reading a compressed tar is the one preview that goes on costing
        // after the cursor has moved.
        if live.cancelled() {
            return Ok(None);
        }
        if entries.len() == limit {
            clipped = true;
            break;
        }

        let entry = entry?;
        let header = entry.header();
        entries.push(Entry {
            name: entry.path()?.to_string_lossy().into_owned(),
            size: header.size()?,
            kind: match header.entry_type() {
                tar::EntryType::Directory => EntryKind::Directory,
                tar::EntryType::Symlink | tar::EntryType::Link => EntryKind::Symlink,
                _ => EntryKind::File,
            },
        });
    }

    Ok(Some(Contents { entries, clipped }))
}
