//! The Quick View panel: whatever is under the cursor, drawn where the
//! opposite listing would be.
//!
//! Dumb like every other widget. Everything it draws was settled by the
//! reading thread; nothing here opens a path, and the only decision left is
//! how many of the bytes it was handed fit on a row.

use std::path::Path;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::{
    fs::{
        directory::DirEntryKind,
        preview::{Content, LinkTarget, Refused},
    },
    ui::{icon::Icon, pane::Pane},
};

/// Wording the panel puts on screen when there is nothing to draw. Gathered
/// here so it reads as one voice instead of being scattered through the match
/// below.
mod words {
    pub const LOADING: &str = "…";
    pub const NOTHING_SELECTED: &str = "Nothing to preview";
    pub const NOT_A_FILE: &str = "Not a regular file";
    pub const MORE: &str = "…";
    pub const EMPTY_DIRECTORY: &str = "Empty directory";
    pub const EMPTY_FILE: &str = "Empty file";
    pub const BROKEN_LINK: &str = "broken link";
    pub const LINK_TO_DIRECTORY: &str = "directory";
    pub const LINK_TO_FILE: &str = "file";
    pub const LINK_TO_OTHER: &str = "special file";
}

/// One look at one path. Holds nothing of its own: both halves are borrowed
/// from the state the main loop keeps, and either may be missing while an
/// answer is on its way.
#[derive(Debug)]
pub struct PreviewPane<'a> {
    path: Option<&'a Path>,
    content: Option<&'a Content>,
}

impl<'a> PreviewPane<'a> {
    /// Colour of a line that says why there is nothing rather than showing
    /// something.
    const MUTED: Color = Color::DarkGray;
    /// Colour of the bytes in a hex dump, which are read as a block and want
    /// to sit behind the offsets and the text beside them.
    const HEX: Color = Color::Gray;

    /// `path` names what is being looked at and `content` is the answer, which
    /// is `None` for as long as the read is still running.
    pub fn new(path: Option<&'a Path>, content: Option<&'a Content>) -> Self {
        Self { path, content }
    }

    /// The title: the name of what is under the cursor, or nothing at all
    /// while the cursor is on nothing.
    fn title(&self) -> Line<'_> {
        let name = self.path.map(|path| match path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => path.to_string_lossy().to_string(),
        });

        match name {
            Some(name) => Line::from(name.bold()),
            None => Line::default(),
        }
    }

    /// The body, laid out for an area this size. Width decides how many bytes
    /// a hex row holds and height decides how much is worth building at all.
    fn body(&self, area: Rect) -> Vec<Line<'_>> {
        let rows = area.height as usize;

        let Some(content) = self.content else {
            // Nothing has arrived. Which of the two it is depends on whether
            // there is anything to arrive for.
            return match self.path {
                Some(_) => vec![muted(words::LOADING)],
                None => vec![muted(words::NOTHING_SELECTED)],
            };
        };

        match content {
            Content::Directory(listing) => {
                // The parent leads every listing and says nothing about the
                // directory being looked at.
                let entries: Vec<_> = listing
                    .entries()
                    .iter()
                    .filter(|entry| entry.kind != DirEntryKind::Parent)
                    .collect();
                if entries.is_empty() {
                    return vec![muted(words::EMPTY_DIRECTORY)];
                }

                let mut lines: Vec<Line> = entries
                    .iter()
                    .take(rows)
                    .map(|entry| {
                        let icon = Icon::icon_for(entry);
                        Line::from(vec![
                            Span::styled(format!("{} ", icon.glyph), Style::new().fg(icon.color)),
                            Span::raw(name_of(&entry.path)),
                        ])
                    })
                    .collect();
                if entries.len() > lines.len() {
                    lines.pop();
                    lines.push(muted(words::MORE));
                }
                lines
            }

            Content::Symlink { target, points_to } => vec![
                Line::from(Span::raw(target.to_string_lossy().to_string())),
                muted(match points_to {
                    LinkTarget::Directory => words::LINK_TO_DIRECTORY,
                    LinkTarget::File => words::LINK_TO_FILE,
                    LinkTarget::Other => words::LINK_TO_OTHER,
                    LinkTarget::Broken => words::BROKEN_LINK,
                }),
            ],

            Content::Text { lines, clipped } => {
                if lines.is_empty() && !clipped {
                    return vec![muted(words::EMPTY_FILE)];
                }

                let mut drawn: Vec<Line> = lines
                    .iter()
                    .take(rows)
                    .map(|line| Line::from(Span::raw(line.as_str())))
                    .collect();
                if *clipped || lines.len() > drawn.len() {
                    drawn.truncate(rows.saturating_sub(1));
                    drawn.push(muted(words::MORE));
                }
                drawn
            }

            Content::Binary { bytes, clipped } => {
                if bytes.is_empty() {
                    return vec![muted(words::EMPTY_FILE)];
                }

                let per_row = bytes_per_row(area.width);
                let mut drawn: Vec<Line> = bytes
                    .chunks(per_row)
                    .take(rows)
                    .enumerate()
                    .map(|(row, chunk)| hex_line(row * per_row, chunk, per_row))
                    .collect();
                if *clipped || bytes.len() > drawn.len() * per_row {
                    drawn.truncate(rows.saturating_sub(1));
                    drawn.push(muted(words::MORE));
                }
                drawn
            }

            Content::Refused(Refused::NotAFile) => vec![muted(words::NOT_A_FILE)],

            // Permissions, or a file that went away between the listing and
            // the read. The panel says so and browsing carries on.
            Content::Unreadable(reason) => vec![muted(reason)],
        }
    }
}

impl Widget for &PreviewPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::bordered()
            .title_top(self.title().centered())
            .border_style(Pane::UNFOCUSED);

        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(self.body(inner)).render(inner, buf);
    }
}

/// The last component of `path`, or the whole path when it has none, which is
/// what the filesystem root is.
fn name_of(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().to_string(),
        None => path.to_string_lossy().to_string(),
    }
}

/// A line that explains rather than shows. The style sits on the span: a
/// styled `Line` repaints its whole area.
fn muted(text: &str) -> Line<'_> {
    Line::from(Span::styled(
        text,
        Style::new().fg(PreviewPane::MUTED).italic(),
    ))
}

/// How many bytes fit on one dump row of this width. A row is a six-digit
/// offset, two spaces, `3n - 1` columns of hex, two spaces and `n` characters
/// of text: `9 + 4n` in all.
///
/// Never less than four, so a panel too narrow to be useful draws a short row
/// rather than nothing.
fn bytes_per_row(width: u16) -> usize {
    let fits = usize::from(width).saturating_sub(9) / 4;
    // Rounded down to four, so rows break at the same places whatever the
    // width, and the eye can follow a column down the dump.
    (fits / 4 * 4).max(4)
}

/// One row of the dump: the offset, the bytes as hex, and the same bytes as
/// text with everything unprintable as a dot. `per_row` holds the text column
/// in line under a short last row.
fn hex_line(offset: usize, chunk: &[u8], per_row: usize) -> Line<'static> {
    let mut hex = String::with_capacity(per_row * 3);
    let mut text = String::with_capacity(per_row);

    for (index, byte) in chunk.iter().enumerate() {
        if index > 0 {
            hex.push(' ');
        }
        hex.push_str(&format!("{byte:02x}"));
        text.push(match byte {
            0x20..=0x7e => *byte as char,
            _ => '.',
        });
    }
    // Pads a short last row out to the full width so the text column does not
    // slide left under it.
    for _ in chunk.len()..per_row {
        hex.push_str("   ");
    }

    Line::from(vec![
        Span::styled(
            format!("{offset:06x}  "),
            Style::new().fg(PreviewPane::MUTED),
        ),
        Span::styled(hex, Style::new().fg(PreviewPane::HEX)),
        Span::raw("  "),
        Span::raw(text),
    ])
}

#[cfg(test)]
mod preview_pane_tests {
    use super::*;

    use std::{path::PathBuf, sync::Arc};

    use crate::fs::directory::{DirEntry, Directory};

    /// Renders into a buffer and gives the rows back as strings, without the
    /// border the block draws around them and without trailing blanks.
    fn rows(pane: &PreviewPane, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        pane.render(area, &mut buf);

        (0..height)
            .map(|y| {
                let row: String = (0..width).map(|x| buf[(x, y)].symbol()).collect();
                row.trim_matches(|c| c == '│' || c == ' ').to_string()
            })
            .collect()
    }

    #[test]
    fn the_title_is_the_name_of_what_is_under_the_cursor() {
        let path = PathBuf::from("/home/user/notes.txt");
        let pane = PreviewPane::new(Some(&path), None);

        assert!(rows(&pane, 30, 4)[0].contains("notes.txt"));
    }

    #[test]
    fn text_is_drawn_line_by_line() {
        let content = Content::Text {
            lines: vec!["first".into(), "second".into()],
            clipped: false,
        };
        let path = PathBuf::from("notes.txt");
        let pane = PreviewPane::new(Some(&path), Some(&content));

        let rows = rows(&pane, 30, 5);
        assert_eq!(rows[1], "first");
        assert_eq!(rows[2], "second");
    }

    #[test]
    fn a_clipped_file_says_so_on_the_last_row_it_has() {
        let content = Content::Text {
            lines: (0..20).map(|i| format!("line {i}")).collect(),
            clipped: true,
        };
        let path = PathBuf::from("big.txt");
        let pane = PreviewPane::new(Some(&path), Some(&content));

        // Four rows of border and body, so only two lines of the file fit.
        let rows = rows(&pane, 30, 4);
        assert_eq!(rows[1], "line 0");
        assert_eq!(rows[2], words::MORE);
    }

    #[test]
    fn binary_is_drawn_as_hex_beside_its_text() {
        let content = Content::Binary {
            bytes: b"Hi\x00\x01".to_vec(),
            clipped: false,
        };
        let path = PathBuf::from("data.bin");
        let pane = PreviewPane::new(Some(&path), Some(&content));

        let rows = rows(&pane, 40, 4);
        assert!(rows[1].starts_with("000000"), "the row was {:?}", rows[1]);
        assert!(rows[1].contains("48 69 00 01"), "the row was {:?}", rows[1]);
        // Only the two printable bytes are shown as themselves.
        assert!(rows[1].ends_with("Hi.."), "the row was {:?}", rows[1]);
    }

    #[test]
    fn a_hex_row_holds_more_bytes_when_the_panel_is_wider() {
        // 9 fixed columns and 4 per byte, rounded down to a multiple of four.
        assert_eq!(bytes_per_row(38), 4);
        assert_eq!(bytes_per_row(78), 16);
        // Too narrow to fit even one, and still a row rather than nothing.
        assert_eq!(bytes_per_row(4), 4);
    }

    #[test]
    fn a_directory_lists_what_is_inside_without_the_parent() {
        let listing = Directory::new(
            Arc::from(Path::new("/home")),
            vec![
                DirEntry {
                    path: Arc::from(Path::new("/")),
                    kind: DirEntryKind::Parent,
                },
                DirEntry {
                    path: Arc::from(Path::new("/home/inside.txt")),
                    kind: DirEntryKind::File,
                },
            ],
        );
        let content = Content::Directory(listing);
        let path = PathBuf::from("/home");
        let pane = PreviewPane::new(Some(&path), Some(&content));

        let rows = rows(&pane, 30, 5);
        assert!(rows[1].contains("inside.txt"), "the row was {:?}", rows[1]);
        assert!(
            !rows.iter().any(|row| row.contains("..")),
            "the parent reached the panel: {rows:?}"
        );
    }

    #[test]
    fn a_symlink_shows_its_target_and_what_it_is() {
        let content = Content::Symlink {
            target: PathBuf::from("/etc/hosts"),
            points_to: LinkTarget::File,
        };
        let path = PathBuf::from("hosts");
        let pane = PreviewPane::new(Some(&path), Some(&content));

        let rows = rows(&pane, 30, 5);
        assert!(rows[1].contains("/etc/hosts"));
        assert!(rows[2].contains(words::LINK_TO_FILE));
    }

    #[test]
    fn nothing_under_the_cursor_draws_an_empty_panel() {
        let pane = PreviewPane::new(None, None);

        let rows = rows(&pane, 30, 4);
        assert!(rows[1].contains(words::NOTHING_SELECTED));
    }
}
