//! Which container an archive is: read from its first bytes when one is
//! opened, and from its name when one is written.

use std::io::Read;

use flate2::read::GzDecoder;

/// A container Mula can both read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
}

/// The suffixes a name can be given to ask for a format, longest first so
/// `.tar.gz` is matched whole rather than as a `.gz` nobody offers.
const SUFFIXES: &[(&str, Format)] = &[
    (".tar.gz", Format::TarGz),
    (".tgz", Format::TarGz),
    (".zip", Format::Zip),
    (".tar", Format::Tar),
];

/// The suffixes above as one phrase, for the message a name none of them fits
/// is turned away with.
pub const WRITABLE: &str = ".zip, .tar, .tar.gz or .tgz";

impl Format {
    /// The format `name` asks for by how it ends, ignoring case, or `None`
    /// when it ends in none of [`SUFFIXES`].
    pub fn of_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        SUFFIXES
            .iter()
            .find(|(suffix, _)| lowered.ends_with(suffix))
            .map(|(_, format)| *format)
    }
}

/// `name` without the archive suffix it ends in, which is the name an archive
/// unpacks under. A name ending in no suffix anyone knows is its own stem, and
/// so is a name that is nothing but a suffix — `.zip` unpacks as `.zip`.
pub fn stem(name: &str) -> &str {
    let lowered = name.to_ascii_lowercase();
    SUFFIXES
        .iter()
        .find_map(|(suffix, _)| {
            lowered
                .ends_with(suffix)
                .then(|| &name[..name.len() - suffix.len()])
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or(name)
}

/// Where a tar header keeps its magic, and what stands there. Both the POSIX
/// spelling and GNU's older one begin this way, and a tar old enough to have
/// no magic at all is not recognised.
const USTAR_AT: usize = 257;
const USTAR: &[u8] = b"ustar";

const ZIP: &[u8] = b"PK\x03\x04";
/// An archive holding nothing at all is its end-of-central-directory record
/// and no local header, so it begins with a different pair.
const ZIP_EMPTY: &[u8] = b"PK\x05\x06";
const GZIP: &[u8] = &[0x1f, 0x8b];

/// How much of a gzip stream is decompressed to see whether a tar is inside.
/// One tar header is 512 bytes and the magic sits near its end.
const GZIP_PEEK: usize = 512;

/// The format the first bytes of a file announce, or `None` for anything that
/// is not an archive this can open.
///
/// A gzip stream is decompressed far enough to read the header inside it,
/// because gzip says only that something is compressed and Mula unpacks a tar
/// rather than a stream. `bytes` is a prefix of the file, so a tar whose
/// header lies past it is not recognised either.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(ZIP) || bytes.starts_with(ZIP_EMPTY) {
        return Some(Format::Zip);
    }
    if is_tar(bytes) {
        return Some(Format::Tar);
    }
    if bytes.starts_with(GZIP) {
        let mut head = Vec::new();
        GzDecoder::new(bytes)
            .take(GZIP_PEEK as u64)
            .read_to_end(&mut head)
            .ok()?;
        return is_tar(&head).then_some(Format::TarGz);
    }
    None
}

fn is_tar(bytes: &[u8]) -> bool {
    bytes
        .get(USTAR_AT..USTAR_AT + USTAR.len())
        .is_some_and(|magic| magic == USTAR)
}

#[cfg(test)]
mod format_tests {
    use super::*;

    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    /// The first bytes of a tar: a header whose magic sits at 257, padded out
    /// to the 512 a header occupies.
    fn tar_header() -> Vec<u8> {
        let mut bytes = vec![0u8; 512];
        bytes[USTAR_AT..USTAR_AT + USTAR.len()].copy_from_slice(USTAR);
        bytes
    }

    fn gzipped(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn a_zip_is_known_by_its_local_header() {
        assert_eq!(sniff(b"PK\x03\x04rest"), Some(Format::Zip));
    }

    #[test]
    fn an_empty_zip_is_still_a_zip() {
        assert_eq!(sniff(b"PK\x05\x06rest"), Some(Format::Zip));
    }

    #[test]
    fn a_tar_is_known_by_the_magic_in_its_first_header() {
        assert_eq!(sniff(&tar_header()), Some(Format::Tar));
    }

    #[test]
    fn a_gzipped_tar_is_known_by_what_is_inside_it() {
        assert_eq!(sniff(&gzipped(&tar_header())), Some(Format::TarGz));
    }

    /// gzip says that something is compressed and no more. What Mula unpacks
    /// is the tar inside, so a gzip holding anything else is not an archive.
    #[test]
    fn a_gzipped_file_that_is_not_a_tar_is_not_an_archive() {
        assert_eq!(sniff(&gzipped(b"hello, and nothing like a tar")), None);
    }

    /// The bytes are a prefix of the file, and a truncated gzip stream stops
    /// short rather than failing the sniff.
    #[test]
    fn a_gzip_cut_off_before_the_header_is_not_an_archive() {
        let whole = gzipped(&tar_header());
        assert_eq!(sniff(&whole[..whole.len() / 2]), None);
    }

    #[test]
    fn a_file_that_announces_nothing_is_not_an_archive() {
        assert_eq!(sniff(b"#!/bin/sh\necho hello\n"), None);
    }

    #[test]
    fn an_empty_file_is_not_an_archive() {
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn a_name_asks_for_a_format_by_how_it_ends() {
        assert_eq!(Format::of_name("backup.zip"), Some(Format::Zip));
        assert_eq!(Format::of_name("backup.tar"), Some(Format::Tar));
        assert_eq!(Format::of_name("backup.tar.gz"), Some(Format::TarGz));
        assert_eq!(Format::of_name("backup.tgz"), Some(Format::TarGz));
        assert_eq!(Format::of_name("backup.rar"), None);
        assert_eq!(Format::of_name("backup"), None);
    }

    #[test]
    fn the_suffix_is_read_whatever_case_it_is_written_in() {
        assert_eq!(Format::of_name("BACKUP.ZIP"), Some(Format::Zip));
        assert_eq!(Format::of_name("Backup.Tar.Gz"), Some(Format::TarGz));
    }

    /// The double suffix comes off whole. Taking the last dot alone would
    /// unpack `notes.tar.gz` into a directory called `notes.tar`.
    #[test]
    fn the_stem_is_the_name_without_the_suffix() {
        assert_eq!(stem("notes.tar.gz"), "notes");
        assert_eq!(stem("notes.tgz"), "notes");
        assert_eq!(stem("notes.zip"), "notes");
        assert_eq!(stem("notes.tar"), "notes");
    }

    #[test]
    fn a_name_with_no_suffix_is_its_own_stem() {
        assert_eq!(stem("notes"), "notes");
        assert_eq!(stem("notes.rar"), "notes.rar");
    }

    /// Stripping would leave nothing to name a directory with.
    #[test]
    fn a_name_that_is_nothing_but_a_suffix_is_its_own_stem() {
        assert_eq!(stem(".zip"), ".zip");
    }
}
