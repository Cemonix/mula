//! Colouring the text a preview shows, by the grammar its name or its first
//! line points to.
//!
//! The theme is written in the terminal's palette rather than in RGB. A syntect
//! colour with an alpha of zero carries a palette index in its red channel, the
//! encoding `bat` uses for its `ansi` theme; any other alpha is the terminal's
//! own foreground.

use std::{ffi::OsStr, path::Path, str::FromStr};

use syntect::{
    easy::HighlightLines,
    highlighting::{Color, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings},
    parsing::SyntaxSet,
};

use crate::fs::preview::{Ink, Run, TextLine, printable};

const RED: u8 = 1;
const GREEN: u8 = 2;
const YELLOW: u8 = 3;
const BLUE: u8 = 4;
const MAGENTA: u8 = 5;
const CYAN: u8 = 6;
const BRIGHT_BLACK: u8 = 8;

/// The ink each scope selector is drawn in. Where several match, the most
/// specific selector wins, whatever its place in the list.
const SCOPES: &[(&str, Ink)] = &[
    (
        "comment, punctuation.definition.comment",
        Ink::Palette(BRIGHT_BLACK),
    ),
    ("string, constant.character", Ink::Palette(GREEN)),
    ("constant.character.escape", Ink::Palette(CYAN)),
    (
        "constant.numeric, constant.language, constant.other, support.constant",
        Ink::Palette(YELLOW),
    ),
    ("keyword, storage", Ink::Palette(MAGENTA)),
    ("keyword.operator", Ink::Plain),
    ("entity.name.function, support.function", Ink::Palette(BLUE)),
    (
        "entity.name.type, entity.name.class, entity.other.inherited-class, support.type, support.class",
        Ink::Palette(YELLOW),
    ),
    ("variable.language", Ink::Palette(RED)),
    ("entity.name.tag", Ink::Palette(CYAN)),
    ("entity.other.attribute-name", Ink::Palette(YELLOW)),
    ("entity.name.section, markup.heading", Ink::Palette(BLUE)),
    ("markup.inserted", Ink::Palette(GREEN)),
    ("markup.deleted, invalid", Ink::Palette(RED)),
    ("markup.changed", Ink::Palette(YELLOW)),
    ("markup.underline.link, meta.diff.range", Ink::Palette(CYAN)),
    ("markup.raw", Ink::Palette(GREEN)),
    ("meta.diff.header", Ink::Palette(BLUE)),
];

/// `ink` in the encoding the theme is written in.
fn encode(ink: Ink) -> Color {
    match ink {
        Ink::Plain => Color {
            r: 0,
            g: 0,
            b: 0,
            a: 1,
        },
        Ink::Palette(index) => Color {
            r: index,
            g: 0,
            b: 0,
            a: 0,
        },
    }
}

/// The ink a colour out of the theme stands for.
fn decode(color: Color) -> Ink {
    match color.a {
        0 => Ink::Palette(color.r),
        _ => Ink::Plain,
    }
}

/// The bundled grammars and the theme over them. Loaded once per reader, on
/// the first text file it is asked for.
pub struct Highlighting {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighting {
    /// Decompresses the bundled grammars and builds the theme from [`SCOPES`].
    pub fn load() -> Self {
        let scopes = SCOPES
            .iter()
            .map(|(selector, ink)| ThemeItem {
                scope: ScopeSelectors::from_str(selector).expect("the selectors are well formed"),
                style: StyleModifier {
                    foreground: Some(encode(*ink)),
                    background: None,
                    font_style: None,
                },
            })
            .collect();

        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            theme: Theme {
                name: None,
                author: None,
                settings: ThemeSettings {
                    foreground: Some(encode(Ink::Plain)),
                    ..ThemeSettings::default()
                },
                scopes,
            },
        }
    }

    /// A painter for the file at `path` that begins with `first_line`, or
    /// `None` when neither names a grammar.
    ///
    /// The whole file name is tried before its extension, so `Makefile` and
    /// `.bashrc` are found; the first line is what finds a shebang.
    pub fn painter(&self, path: &Path, first_line: &str) -> Option<Painter<'_>> {
        let name = path.file_name().and_then(OsStr::to_str);
        let extension = path.extension().and_then(OsStr::to_str);

        let syntax = name
            .and_then(|name| self.syntaxes.find_syntax_by_extension(name))
            .or_else(|| extension.and_then(|ext| self.syntaxes.find_syntax_by_extension(ext)))
            .or_else(|| self.syntaxes.find_syntax_by_first_line(first_line))?;

        Some(Painter {
            lines: HighlightLines::new(syntax, &self.theme),
            syntaxes: &self.syntaxes,
        })
    }
}

/// Colours one file line after line. A line begins in the state the one before
/// it left, so the lines have to be handed over in order.
pub struct Painter<'a> {
    lines: HighlightLines<'a>,
    syntaxes: &'a SyntaxSet,
}

impl Painter<'_> {
    /// Colours the next line of the file, cut to `max_chars` the way an
    /// uncoloured line is: each run goes through `printable`, and what is
    /// left of the line's budget bounds the next one. Neighbouring runs of one
    /// ink are joined.
    ///
    /// Only the first `max_chars` characters are handed to the grammar. `None`
    /// when the grammar fails on the line, after which the painter is not to
    /// be used again.
    pub fn paint(&mut self, line: &str, max_chars: usize) -> Option<TextLine> {
        let kept = match line.char_indices().nth(max_chars) {
            Some((end, _)) => &line[..end],
            None => line,
        };
        let with_end = format!("{kept}\n");
        let regions = self.lines.highlight_line(&with_end, self.syntaxes).ok()?;

        let mut runs: Vec<Run> = Vec::new();
        let mut left = max_chars;
        for (style, piece) in regions {
            let text = printable(piece, left);
            left = left.saturating_sub(text.chars().count());
            if text.is_empty() {
                continue;
            }

            let ink = decode(style.foreground);
            match runs.last_mut() {
                Some(last) if last.ink == ink => last.text.push_str(&text),
                _ => runs.push(Run { text, ink }),
            }
        }

        Some(TextLine { runs })
    }
}

#[cfg(test)]
mod highlight_tests {
    use super::*;

    use std::sync::LazyLock;

    static HIGHLIGHTING: LazyLock<Highlighting> = LazyLock::new(Highlighting::load);

    /// Paints `source` as the file `name`, line by line.
    fn paint(name: &str, source: &str, max_chars: usize) -> Option<Vec<TextLine>> {
        let mut painter =
            HIGHLIGHTING.painter(Path::new(name), source.lines().next().unwrap_or_default())?;
        Some(
            source
                .lines()
                .map(|line| painter.paint(line, max_chars).expect("the grammar copes"))
                .collect(),
        )
    }

    fn inks(lines: &[TextLine]) -> Vec<Ink> {
        let mut inks: Vec<Ink> = lines
            .iter()
            .flat_map(|line| line.runs.iter().map(|run| run.ink))
            .collect();
        inks.dedup();
        inks
    }

    const RUST: &str = "// a comment\nfn main() {\n\tlet s = \"text\";\n}\n";

    #[test]
    fn the_runs_of_a_line_join_back_to_its_text() {
        let lines = paint("main.rs", RUST, 512).expect("rust has a grammar");

        let expected: Vec<String> = RUST.lines().map(|line| printable(line, 512)).collect();
        let joined: Vec<String> = lines.iter().map(TextLine::text).collect();
        assert_eq!(joined, expected);
    }

    #[test]
    fn a_known_language_is_drawn_in_more_than_one_ink() {
        let lines = paint("main.rs", RUST, 512).expect("rust has a grammar");

        assert!(inks(&lines).len() > 1, "inks were {:?}", inks(&lines));
        assert!(
            lines
                .iter()
                .flat_map(|line| &line.runs)
                .all(|run| !run.text.is_empty()),
            "an empty run was kept"
        );
    }

    #[test]
    fn a_painted_line_is_cut_where_an_uncoloured_one_would_be() {
        let source = "let long = \"abcdefghijklmnopqrstuvwxyz\";\n";

        let lines = paint("long.rs", source, 10).expect("rust has a grammar");
        assert_eq!(lines[0].text(), printable(source.trim_end(), 10));
    }

    #[test]
    fn a_shebang_names_the_grammar_of_a_file_without_an_extension() {
        assert!(paint("build", "#!/bin/sh\necho hi\n", 512).is_some());
    }

    #[test]
    fn a_whole_file_name_names_a_grammar() {
        assert!(paint("Makefile", "all:\n\techo hi\n", 512).is_some());
    }

    #[test]
    fn a_name_nothing_knows_has_no_painter() {
        assert!(paint("notes.unknown-format", "just words\n", 512).is_none());
    }

    #[test]
    fn a_palette_ink_survives_the_theme_encoding() {
        for ink in [Ink::Plain, Ink::Palette(0), Ink::Palette(15)] {
            assert_eq!(decode(encode(ink)), ink);
        }
    }
}
