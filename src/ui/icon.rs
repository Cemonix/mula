use std::path::Path;

use ratatui::style::Color;

use crate::fs::{
    archive::contents::{self, EntryKind},
    directory::{DirEntry, DirEntryKind},
};

// The glyph codepoints and most of the colours below were taken from the
// `devicons` crate (https://github.com/alexpasmantier/rust-devicons), version
// 0.6.13, which is licensed under the Apache License 2.0. The tables here are a
// curated subset with several colours replaced.

/// A glyph and the colour it is drawn in. `Color` is stored ready to use, so
/// the tables below need no parsing at runtime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icon {
    pub glyph: char,
    pub color: Color,
}

impl Icon {
    const fn new(glyph: char, r: u8, g: u8, b: u8) -> Self {
        Self {
            glyph,
            color: Color::Rgb(r, g, b),
        }
    }

    /// The icon for an entry. Consults, in order: the entry kind, the exact file
    /// name, then the extension, and falls back to `DEFAULT_FILE` when none match.
    pub fn icon_for(entry: &DirEntry) -> Icon {
        match entry.kind {
            DirEntryKind::Parent => PARENT,
            DirEntryKind::Directory => DIRECTORY,
            DirEntryKind::Symlink => SYMLINK,
            DirEntryKind::File => entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(Self::by_name)
                .or_else(|| Self::by_extension(&entry.path))
                .unwrap_or(DEFAULT_FILE),
        }
    }

    /// The icon for a name inside an archive, where there is no `DirEntry` to
    /// ask. The kind decides first and the name after it, the same order
    /// [`Icon::icon_for`] takes.
    pub fn for_archive_entry(entry: &contents::Entry) -> Icon {
        match entry.kind {
            EntryKind::Directory => DIRECTORY,
            EntryKind::Symlink => SYMLINK,
            EntryKind::File => Self::by_name(&entry.name)
                .or_else(|| Self::by_extension(Path::new(&entry.name)))
                .unwrap_or(DEFAULT_FILE),
        }
    }

    /// Looks `name` up in `BY_NAME`, comparing without regard to ASCII case.
    /// Returns `None` when nothing matches.
    fn by_name(name: &str) -> Option<Icon> {
        BY_NAME
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, icon)| *icon)
    }

    /// Looks the extension of `path` up in `BY_EXTENSION`, comparing without regard
    /// to ASCII case. Returns `None` when the path has no extension or nothing
    /// matches.
    fn by_extension(path: &Path) -> Option<Icon> {
        let extension = path.extension()?.to_str()?;
        BY_EXTENSION
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(extension))
            .map(|(_, icon)| *icon)
    }
}

/// Exact file names, read before `BY_EXTENSION`. Names are written as they
/// usually appear; the lookup ignores ASCII case.
static BY_NAME: &[(&str, Icon)] = &[
    ("Cargo.toml", Icon::new('\u{e68b}', 0xde, 0xa5, 0x84)),
    ("Cargo.lock", Icon::new('\u{e68b}', 0xb0, 0x89, 0x68)),
    ("Makefile", Icon::new('\u{e779}', 0x6d, 0x80, 0x86)),
    ("Dockerfile", Icon::new('\u{f0868}', 0x45, 0x8e, 0xe6)),
    (
        "docker-compose.yml",
        Icon::new('\u{f0868}', 0x45, 0x8e, 0xe6),
    ),
    ("CMakeLists.txt", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("LICENSE", Icon::new('\u{e60a}', 0xcb, 0xcb, 0x41)),
    ("README.md", Icon::new('\u{f48a}', 0xd8, 0xc8, 0x8a)),
    ("package.json", Icon::new('\u{e71e}', 0xe8, 0x27, 0x4b)),
    ("package-lock.json", Icon::new('\u{e71e}', 0x7a, 0x0d, 0x21)),
    ("tsconfig.json", Icon::new('\u{e69d}', 0x51, 0x9a, 0xba)),
    ("go.mod", Icon::new('\u{e627}', 0x51, 0x9a, 0xba)),
    ("go.sum", Icon::new('\u{e627}', 0x51, 0x9a, 0xba)),
    ("pyproject.toml", Icon::new('\u{e606}', 0xff, 0xbc, 0x03)),
    ("requirements.txt", Icon::new('\u{e606}', 0xff, 0xbc, 0x03)),
    ("Gemfile", Icon::new('\u{e791}', 0x70, 0x15, 0x16)),
    ("Rakefile", Icon::new('\u{e791}', 0x70, 0x15, 0x16)),
    ("node_modules", Icon::new('\u{e718}', 0xe8, 0x27, 0x4b)),
    (".gitignore", Icon::new('\u{e702}', 0xf5, 0x4d, 0x27)),
    (".gitattributes", Icon::new('\u{e702}', 0xf5, 0x4d, 0x27)),
    (".gitmodules", Icon::new('\u{e702}', 0xf5, 0x4d, 0x27)),
    (".env", Icon::new('\u{f462}', 0xfa, 0xf7, 0x43)),
    (".editorconfig", Icon::new('\u{e652}', 0xd8, 0xd8, 0xd8)),
    (".dockerignore", Icon::new('\u{f0868}', 0x45, 0x8e, 0xe6)),
    (".bashrc", Icon::new('\u{e615}', 0x89, 0xe0, 0x51)),
    (".bash_profile", Icon::new('\u{e615}', 0x89, 0xe0, 0x51)),
    (".zshrc", Icon::new('\u{e615}', 0x89, 0xe0, 0x51)),
    (".vimrc", Icon::new('\u{e62b}', 0x01, 0x98, 0x33)),
    (".npmrc", Icon::new('\u{e71e}', 0xe8, 0x27, 0x4b)),
    (".prettierrc", Icon::new('\u{e6b4}', 0x42, 0x85, 0xf4)),
    (".babelrc", Icon::new('\u{e639}', 0xcb, 0xcb, 0x41)),
    (".DS_Store", Icon::new('\u{e615}', 0x6f, 0x8b, 0x95)),
];

/// File extensions without the leading dot. The lookup ignores ASCII case.
static BY_EXTENSION: &[(&str, Icon)] = &[
    ("rs", Icon::new('\u{e68b}', 0xde, 0xa5, 0x84)),
    ("py", Icon::new('\u{e606}', 0xff, 0xbc, 0x03)),
    ("js", Icon::new('\u{e60c}', 0xcb, 0xcb, 0x41)),
    ("jsx", Icon::new('\u{e625}', 0x20, 0xc2, 0xe3)),
    ("ts", Icon::new('\u{e628}', 0x51, 0x9a, 0xba)),
    ("tsx", Icon::new('\u{e7ba}', 0x13, 0x54, 0xbf)),
    ("vue", Icon::new('\u{e6a0}', 0x8d, 0xc1, 0x49)),
    ("svelte", Icon::new('\u{e697}', 0xff, 0x3e, 0x00)),
    ("go", Icon::new('\u{e627}', 0x51, 0x9a, 0xba)),
    ("c", Icon::new('\u{e61e}', 0x59, 0x9e, 0xff)),
    ("h", Icon::new('\u{f0fd}', 0xa0, 0x74, 0xc4)),
    ("cpp", Icon::new('\u{e61d}', 0x51, 0x9a, 0xba)),
    ("cc", Icon::new('\u{e61d}', 0xf3, 0x4b, 0x7d)),
    ("hpp", Icon::new('\u{f0fd}', 0xa0, 0x74, 0xc4)),
    ("cs", Icon::new('\u{f031b}', 0x9b, 0x4f, 0x96)),
    ("java", Icon::new('\u{e738}', 0xcc, 0x3e, 0x44)),
    ("kt", Icon::new('\u{e634}', 0x7f, 0x52, 0xff)),
    ("rb", Icon::new('\u{e791}', 0x70, 0x15, 0x16)),
    ("php", Icon::new('\u{e608}', 0xa0, 0x74, 0xc4)),
    ("swift", Icon::new('\u{e755}', 0xe3, 0x79, 0x33)),
    ("lua", Icon::new('\u{e620}', 0x51, 0xa0, 0xcf)),
    ("sh", Icon::new('\u{e795}', 0xa3, 0xb8, 0xbf)),
    ("bash", Icon::new('\u{e795}', 0x89, 0xe0, 0x51)),
    ("zsh", Icon::new('\u{e795}', 0x89, 0xe0, 0x51)),
    ("fish", Icon::new('\u{e795}', 0xa3, 0xb8, 0xbf)),
    ("vim", Icon::new('\u{e62b}', 0x01, 0x98, 0x33)),
    ("sql", Icon::new('\u{e706}', 0xda, 0xd8, 0xd8)),
    ("html", Icon::new('\u{e736}', 0xe4, 0x4d, 0x26)),
    ("css", Icon::new('\u{e749}', 0x42, 0xa5, 0xf5)),
    ("scss", Icon::new('\u{e603}', 0xf5, 0x53, 0x85)),
    ("sass", Icon::new('\u{e603}', 0xf5, 0x53, 0x85)),
    ("less", Icon::new('\u{e614}', 0x56, 0x3d, 0x7c)),
    ("json", Icon::new('\u{e60b}', 0xcb, 0xcb, 0x41)),
    ("yaml", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("yml", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("toml", Icon::new('\u{e6b2}', 0x9c, 0x42, 0x21)),
    ("xml", Icon::new('\u{f05c0}', 0xe3, 0x79, 0x33)),
    ("ini", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("cfg", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("conf", Icon::new('\u{e615}', 0x6d, 0x80, 0x86)),
    ("md", Icon::new('\u{f48a}', 0x83, 0xa5, 0x98)),
    ("txt", Icon::new('\u{f0219}', 0x89, 0xe0, 0x51)),
    ("pdf", Icon::new('\u{eaeb}', 0xb3, 0x0b, 0x00)),
    ("doc", Icon::new('\u{f022c}', 0x18, 0x5a, 0xbd)),
    ("docx", Icon::new('\u{f022c}', 0x18, 0x5a, 0xbd)),
    ("xls", Icon::new('\u{f021b}', 0x20, 0x72, 0x45)),
    ("xlsx", Icon::new('\u{f021b}', 0x20, 0x72, 0x45)),
    ("csv", Icon::new('\u{e64a}', 0x89, 0xe0, 0x51)),
    ("png", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("jpg", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("jpeg", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("gif", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("svg", Icon::new('\u{f0721}', 0xff, 0xb1, 0x3b)),
    ("webp", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("bmp", Icon::new('\u{e60d}', 0xa0, 0x74, 0xc4)),
    ("ico", Icon::new('\u{e60d}', 0xcb, 0xcb, 0x41)),
    ("mp3", Icon::new('\u{f001}', 0x00, 0xaf, 0xff)),
    ("wav", Icon::new('\u{f001}', 0x00, 0xaf, 0xff)),
    ("flac", Icon::new('\u{f001}', 0x00, 0x75, 0xaa)),
    ("ogg", Icon::new('\u{f001}', 0x00, 0x75, 0xaa)),
    ("mp4", Icon::new('\u{e69f}', 0xfd, 0x97, 0x1f)),
    ("mkv", Icon::new('\u{e69f}', 0xfd, 0x97, 0x1f)),
    ("avi", Icon::new('\u{e69f}', 0xfd, 0x97, 0x1f)),
    ("mov", Icon::new('\u{e69f}', 0xfd, 0x97, 0x1f)),
    ("webm", Icon::new('\u{e69f}', 0xfd, 0x97, 0x1f)),
    ("zip", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("tar", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("gz", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("bz2", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("xz", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("7z", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("rar", Icon::new('\u{f410}', 0xec, 0xa5, 0x17)),
    ("exe", Icon::new('\u{eae8}', 0x9f, 0x05, 0x00)),
    ("dll", Icon::new('\u{eb9c}', 0xb0, 0x8b, 0x5a)),
    ("so", Icon::new('\u{eb9c}', 0xdc, 0xdd, 0xd6)),
    ("dylib", Icon::new('\u{eb9c}', 0xdc, 0xdd, 0xd6)),
    ("deb", Icon::new('\u{f410}', 0xd8, 0xa0, 0xa0)),
    ("rpm", Icon::new('\u{f410}', 0xd8, 0xa0, 0xa0)),
    ("iso", Icon::new('\u{e271}', 0xd0, 0xbe, 0xc8)),
    ("log", Icon::new('\u{f0331}', 0x9a, 0xa5, 0xb1)),
    ("lock", Icon::new('\u{e672}', 0xb0, 0xb4, 0xbb)),
    ("ttf", Icon::new('\u{f031}', 0xec, 0xec, 0xec)),
    ("otf", Icon::new('\u{f031}', 0xec, 0xec, 0xec)),
    ("woff", Icon::new('\u{f031}', 0xec, 0xec, 0xec)),
    ("woff2", Icon::new('\u{f031}', 0xec, 0xec, 0xec)),
    ("nix", Icon::new('\u{f313}', 0x7e, 0xba, 0xe4)),
    ("hs", Icon::new('\u{e61f}', 0xa0, 0x74, 0xc4)),
    ("ml", Icon::new('\u{e67a}', 0xe3, 0x79, 0x33)),
    ("ex", Icon::new('\u{e62d}', 0xa0, 0x74, 0xc4)),
    ("exs", Icon::new('\u{e62d}', 0xa0, 0x74, 0xc4)),
    ("erl", Icon::new('\u{e7b1}', 0xb8, 0x39, 0x98)),
    ("clj", Icon::new('\u{e768}', 0x8d, 0xc1, 0x49)),
    ("scala", Icon::new('\u{e737}', 0xcc, 0x3e, 0x44)),
    ("dart", Icon::new('\u{e798}', 0x03, 0x58, 0x9c)),
    ("r", Icon::new('\u{f07d4}', 0x22, 0x66, 0xba)),
    ("jl", Icon::new('\u{e624}', 0xa2, 0x70, 0xba)),
    ("pl", Icon::new('\u{e769}', 0x51, 0x9a, 0xba)),
    ("tex", Icon::new('\u{e69b}', 0x3d, 0x61, 0x17)),
    ("diff", Icon::new('\u{e728}', 0x6f, 0x8b, 0x95)),
    ("patch", Icon::new('\u{e728}', 0x6f, 0x8b, 0x95)),
    ("rss", Icon::new('\u{e619}', 0xfb, 0x9d, 0x3b)),
];

/// Drawn for `DirEntryKind::Directory`.
const DIRECTORY: Icon = Icon::new('\u{f07b}', 0x82, 0xaa, 0xff);
/// Drawn for `DirEntryKind::Parent`.
const PARENT: Icon = Icon::new('\u{f062}', 0x80, 0x8a, 0x94);
/// Drawn for `DirEntryKind::Symlink`, whatever the link points at.
const SYMLINK: Icon = Icon::new('\u{f0c1}', 0x56, 0xb6, 0xc2);
/// Drawn for a file that matched neither table.
const DEFAULT_FILE: Icon = Icon::new('\u{f15b}', 0x9a, 0xa5, 0xb1);

#[cfg(test)]
mod icon_tests {
    use std::sync::Arc;

    use super::*;

    fn entry(path: &str, kind: DirEntryKind) -> DirEntry {
        DirEntry {
            path: Arc::from(Path::new(path)),
            kind,
            meta: None,
        }
    }

    fn file(path: &str) -> DirEntry {
        entry(path, DirEntryKind::File)
    }

    #[test]
    fn an_exact_name_wins_over_the_extension() {
        // Cargo.toml is in both tables: as a name, and as the toml extension.
        assert_eq!(Icon::icon_for(&file("/p/Cargo.toml")).glyph, '\u{e68b}');
        assert_eq!(Icon::icon_for(&file("/p/other.toml")).glyph, '\u{e6b2}');
    }

    #[test]
    fn a_dotfile_is_found_by_its_whole_name() {
        // Path::extension is None for .gitignore, so only BY_NAME can match it.
        assert_eq!(Icon::icon_for(&file("/p/.gitignore")).glyph, '\u{e702}');
    }

    #[test]
    fn the_lookup_ignores_ascii_case_in_both_tables() {
        assert_eq!(
            Icon::icon_for(&file("/p/CARGO.TOML")),
            Icon::icon_for(&file("/p/Cargo.toml"))
        );
        assert_eq!(
            Icon::icon_for(&file("/p/main.RS")),
            Icon::icon_for(&file("/p/main.rs"))
        );
    }

    #[test]
    fn an_unknown_extension_and_a_bare_name_fall_back_to_the_default() {
        assert_eq!(Icon::icon_for(&file("/p/thing.qqq")), DEFAULT_FILE);
        assert_eq!(Icon::icon_for(&file("/p/thing")), DEFAULT_FILE);
    }

    #[test]
    fn the_kind_decides_before_either_table_is_read() {
        // Named and suffixed like a Rust source file, but not a file.
        assert_eq!(
            Icon::icon_for(&entry("/p/Cargo.toml", DirEntryKind::Directory)),
            DIRECTORY
        );
        assert_eq!(
            Icon::icon_for(&entry("/p/main.rs", DirEntryKind::Symlink)),
            SYMLINK
        );
        assert_eq!(Icon::icon_for(&entry("/p", DirEntryKind::Parent)), PARENT);
    }

    #[test]
    fn only_the_last_extension_is_read() {
        // Path::extension yields gz for archive.tar.gz, never tar.gz.
        assert_eq!(
            Icon::icon_for(&file("/p/archive.tar.gz")),
            Icon::icon_for(&file("/p/archive.gz"))
        );
    }

    #[test]
    fn no_key_is_listed_twice_in_either_table() {
        for table in [BY_NAME, BY_EXTENSION] {
            for (i, (key, _)) in table.iter().enumerate() {
                let duplicate = table
                    .iter()
                    .skip(i + 1)
                    .any(|(other, _)| other.eq_ignore_ascii_case(key));
                assert!(!duplicate, "{key} is listed twice");
            }
        }
    }
}
