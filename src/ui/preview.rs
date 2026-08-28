//! The Quick View panel: whatever is under the cursor, drawn where the
//! opposite listing would be.
//!
//! Dumb like every other widget. Everything it draws was settled by the
//! reading thread; nothing here opens a path. What is left is laying it out
//! for an area whose size only this side knows: how many bytes fit on a dump
//! row, and how a bitmap fits a grid of half-pixels.

use std::path::Path;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, StatefulWidget, Widget},
};

use crate::{
    fs::{
        directory::DirEntryKind,
        preview::{Bitmap, Content, LinkTarget, Refused},
    },
    ui::{
        graphics::capabilities::{CellSize, Graphics},
        icon::Icon,
        name_of,
        pane::Pane,
    },
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

/// Paints the upper of the two pixels a cell holds; the cell's background is
/// the lower one. Two pixels to a cell is what makes the grid roughly square
/// in a terminal whose cells are about twice as tall as they are wide.
const UPPER_HALF: &str = "▀";
/// The same block the other way up, for a cell whose upper pixel is not part
/// of the picture and has to keep showing the panel behind it.
const LOWER_HALF: &str = "▄";
/// Alpha at or above which a pixel is painted at all. Below it the panel shows
/// through, which is what makes a logo on transparency look like one.
const OPAQUE_ENOUGH: u8 = 0x20;

/// One look at one path. Holds nothing of its own: both halves are borrowed
/// from the state the main loop keeps, and either may be missing while an
/// answer is on its way.
#[derive(Debug)]
pub struct PreviewPane<'a> {
    path: Option<&'a Path>,
    content: Option<&'a Content>,
    /// How the terminal draws pictures of its own, or `None` where half blocks
    /// are all there is.
    graphics: Option<Graphics>,
}

/// What the panel has to put in its area, in the two shapes it comes in. Kept
/// apart so neither drawing path needs an arm for the other.
enum Body<'a> {
    Lines(Vec<Line<'a>>),
    Image(&'a Bitmap),
}

impl<'a> PreviewPane<'a> {
    /// Colour of a line that says why there is nothing rather than showing
    /// something.
    const MUTED: Color = Color::DarkGray;
    /// Colour of the bytes in a hex dump, which are read as a block and want
    /// to sit behind the offsets and the text beside them.
    const HEX: Color = Color::Gray;

    /// `path` names what is being looked at and `content` is the answer, which
    /// is `None` for as long as the read is still running. `graphics` decides
    /// which of the two backends a picture goes to.
    pub fn new(
        path: Option<&'a Path>,
        content: Option<&'a Content>,
        graphics: Option<Graphics>,
    ) -> Self {
        Self {
            path,
            content,
            graphics,
        }
    }

    /// The title: the name of what is under the cursor, or nothing at all
    /// while the cursor is on nothing.
    fn title(&self) -> Line<'_> {
        match self.path.map(name_of) {
            Some(name) => Line::from(name.bold()),
            None => Line::default(),
        }
    }

    /// The body, laid out for an area this size. Width decides how many bytes
    /// a hex row holds and height decides how much is worth building at all.
    fn body(&self, area: Rect) -> Body<'_> {
        let rows = area.height as usize;

        let Some(content) = self.content else {
            // Nothing has arrived. Which of the two it is depends on whether
            // there is anything to arrive for.
            return Body::Lines(match self.path {
                Some(_) => vec![muted(words::LOADING)],
                None => vec![muted(words::NOTHING_SELECTED)],
            });
        };

        Body::Lines(match content {
            Content::Image(bitmap) => return Body::Image(bitmap),

            Content::Directory(listing) => {
                // The parent leads every listing and says nothing about the
                // directory being looked at.
                let entries: Vec<_> = listing
                    .entries()
                    .iter()
                    .filter(|entry| entry.kind != DirEntryKind::Parent)
                    .collect();
                if entries.is_empty() {
                    return Body::Lines(vec![muted(words::EMPTY_DIRECTORY)]);
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
                    return Body::Lines(vec![muted(words::EMPTY_FILE)]);
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
                    return Body::Lines(vec![muted(words::EMPTY_FILE)]);
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
        })
    }
}

/// Draws the panel and says where a picture the terminal holds itself has to
/// go.
///
/// The area comes back rather than the sequence that puts it there: a widget
/// stays a function from a `Rect` to cells, and the writing belongs to the one
/// place that owns what is on the screen. `None` is a panel with no picture of
/// that kind in it, which a half-block panel always is.
impl StatefulWidget for &PreviewPane<'_> {
    type State = Option<Rect>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        *state = None;

        let block = Block::bordered()
            .title_top(self.title().centered())
            .border_style(Pane::UNFOCUSED);

        let inner = block.inner(area);
        block.render(area, buf);

        match self.body(inner) {
            Body::Lines(lines) => Paragraph::new(lines).render(inner, buf),
            Body::Image(bitmap) => match self.graphics {
                None => draw_image(bitmap, inner, buf),
                // The cells under a picture the terminal draws have to be
                // blank, and have to be *known* to be blank: what ratatui
                // takes for unchanged it never writes out, and whatever stood
                // here would show through the moment the picture went away.
                Some(graphics) => {
                    Clear.render(inner, buf);
                    *state = Some(placement_area(bitmap, inner, graphics.cell));
                }
            },
        }
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

/// Where the picture sits in the grid of half-pixels a panel offers, and how
/// to read one point of it.
struct Placement {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

impl Placement {
    /// Centres `bitmap` in a grid `columns` across and `rows` of half-pixels
    /// tall, keeping the shape of the picture.
    fn fit(bitmap: &Bitmap, columns: u32, rows: u32) -> Self {
        let (width, height) = scaled(bitmap.width, bitmap.height, columns, rows);
        Self {
            left: (columns - width) / 2,
            // Rounded down to a whole cell. Starting on the lower half of one
            // would put every row of the picture in the wrong half of its own.
            top: ((rows - height) / 2) & !1,
            width,
            height,
        }
    }

    /// The colour of the grid point at `(x, y)`: `None` where the picture is
    /// not, and where what is there is too transparent to paint.
    fn sample(&self, bitmap: &Bitmap, x: u32, y: u32) -> Option<Color> {
        let x = x.checked_sub(self.left).filter(|x| *x < self.width)?;
        let y = y.checked_sub(self.top).filter(|y| *y < self.height)?;

        // The box of source pixels this one grid point stands for. Averaging
        // them is what keeps this second scaling, after the one the reader
        // already did, from turning every edge into stairs.
        let x0 = x * bitmap.width / self.width;
        let x1 = ((x + 1) * bitmap.width / self.width).clamp(x0 + 1, bitmap.width);
        let y0 = y * bitmap.height / self.height;
        let y1 = ((y + 1) * bitmap.height / self.height).clamp(y0 + 1, bitmap.height);

        let mut total = [0u32; 4];
        let mut count = 0;
        for source_y in y0..y1 {
            for source_x in x0..x1 {
                for (sum, channel) in total.iter_mut().zip(bitmap.pixel(source_x, source_y)) {
                    *sum += u32::from(channel);
                }
                count += 1;
            }
        }

        let alpha = total[3] / count;
        if alpha < u32::from(OPAQUE_ENOUGH) {
            return None;
        }

        // What is only partly transparent is composited onto black: the colour
        // behind the panel is the terminal's own and cannot be read from here.
        let mix = |sum: u32| ((sum / count) * alpha / 255) as u8;
        Some(Color::Rgb(mix(total[0]), mix(total[1]), mix(total[2])))
    }
}

/// The largest size with the shape of `width` by `height` that fits inside
/// `max_width` by `max_height`. Compared as cross products, so nothing rounds
/// through a float, and never smaller than one, so a very wide picture comes
/// out as a line rather than as nothing.
fn scaled(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width * max_height <= max_width * height {
        ((width * max_height / height).max(1), max_height)
    } else {
        (max_width, (height * max_width / width).max(1))
    }
}

/// The cells a picture the terminal draws itself is placed into: centred in
/// `area` and as near its own shape as whole cells allow.
///
/// The fit is worked out in pixels, which is where the shape is, and rounded
/// back to cells after. The protocol scales the picture into exactly the cells
/// it is handed, so that rounding is the whole of the distortion: under half a
/// cell on each side.
fn placement_area(bitmap: &Bitmap, area: Rect, cell: CellSize) -> Rect {
    let (width, height) = scaled(
        bitmap.width,
        bitmap.height,
        u32::from(area.width) * u32::from(cell.width.get()),
        u32::from(area.height) * u32::from(cell.height.get()),
    );

    let columns = cells(width, cell.width.get()).clamp(1, area.width.max(1));
    let rows = cells(height, cell.height.get()).clamp(1, area.height.max(1));

    Rect {
        x: area.x + (area.width.saturating_sub(columns)) / 2,
        y: area.y + (area.height.saturating_sub(rows)) / 2,
        width: columns,
        height: rows,
    }
}

/// A length in pixels as one in cells, to the nearest whole cell.
fn cells(pixels: u32, per_cell: u16) -> u16 {
    let per_cell = u32::from(per_cell);
    ((pixels + per_cell / 2) / per_cell) as u16
}

/// Draws `bitmap` into `area`, two pixels to a cell.
///
/// A cell that no part of the picture reaches is left alone rather than
/// painted over, so the panel shows through around the edges and behind
/// anything transparent.
fn draw_image(bitmap: &Bitmap, area: Rect, buf: &mut Buffer) {
    let columns = u32::from(area.width);
    let rows = u32::from(area.height) * 2;
    if columns == 0 || rows == 0 || bitmap.width == 0 || bitmap.height == 0 {
        return;
    }

    let placement = Placement::fit(bitmap, columns, rows);

    for row in 0..area.height {
        for column in 0..area.width {
            let x = u32::from(column);
            let upper = placement.sample(bitmap, x, u32::from(row) * 2);
            let lower = placement.sample(bitmap, x, u32::from(row) * 2 + 1);

            let cell = &mut buf[(area.x + column, area.y + row)];
            match (upper, lower) {
                (None, None) => (),
                (Some(colour), None) => {
                    cell.set_symbol(UPPER_HALF).set_fg(colour);
                }
                (None, Some(colour)) => {
                    cell.set_symbol(LOWER_HALF).set_fg(colour);
                }
                (Some(upper), Some(lower)) => {
                    cell.set_symbol(UPPER_HALF).set_fg(upper).set_bg(lower);
                }
            }
        }
    }
}

#[cfg(test)]
mod preview_pane_tests {
    use super::*;

    use std::{num::NonZeroU16, path::PathBuf, sync::Arc};

    use crate::ui::graphics::capabilities::Protocol;

    use crate::fs::directory::{Detail, DirEntry, Directory};

    /// Renders into a buffer and gives the rows back as strings, without the
    /// border the block draws around them and without trailing blanks.
    fn rows(pane: &PreviewPane, width: u16, height: u16) -> Vec<String> {
        let buf = rendered(pane, width, height);

        (0..height)
            .map(|y| {
                let row: String = (0..width).map(|x| buf[(x, y)].symbol()).collect();
                row.trim_matches(|c| c == '│' || c == ' ').to_string()
            })
            .collect()
    }

    /// Renders and hands the whole buffer back, so a test can look at colours
    /// as well as at symbols.
    fn rendered(pane: &PreviewPane, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        StatefulWidget::render(pane, area, &mut buf, &mut None);
        buf
    }

    /// A bitmap from one colour per pixel, given row by row.
    fn bitmap(width: u32, height: u32, pixels: &[[u8; 4]]) -> Bitmap {
        Bitmap::of(
            width,
            height,
            pixels.iter().flatten().copied().collect::<Vec<u8>>(),
        )
    }

    #[test]
    fn the_title_is_the_name_of_what_is_under_the_cursor() {
        let path = PathBuf::from("/home/user/notes.txt");
        let pane = PreviewPane::new(Some(&path), None, None);

        assert!(rows(&pane, 30, 4)[0].contains("notes.txt"));
    }

    #[test]
    fn text_is_drawn_line_by_line() {
        let content = Content::Text {
            lines: vec!["first".into(), "second".into()],
            clipped: false,
        };
        let path = PathBuf::from("notes.txt");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

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
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

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
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

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
                    meta: None,
                },
                DirEntry {
                    path: Arc::from(Path::new("/home/inside.txt")),
                    kind: DirEntryKind::File,
                    meta: None,
                },
            ],
            Detail::NamesOnly,
        );
        let content = Content::Directory(listing);
        let path = PathBuf::from("/home");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

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
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

        let rows = rows(&pane, 30, 5);
        assert!(rows[1].contains("/etc/hosts"));
        assert!(rows[2].contains(words::LINK_TO_FILE));
    }

    #[test]
    fn nothing_under_the_cursor_draws_an_empty_panel() {
        let pane = PreviewPane::new(None, None, None);

        let rows = rows(&pane, 30, 4);
        assert!(rows[1].contains(words::NOTHING_SELECTED));
    }

    #[test]
    fn a_cell_carries_the_pixel_above_it_as_ink_and_the_one_below_as_ground() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        const BLUE: [u8; 4] = [0, 0, 255, 255];
        // One column, two rows: exactly the two halves of one cell.
        let content = Content::Image(bitmap(1, 2, &[RED, BLUE]));
        let path = PathBuf::from("flag.png");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

        // One cell of body inside a border on every side.
        let buf = rendered(&pane, 3, 3);
        let cell = &buf[(1, 1)];

        assert_eq!(cell.symbol(), UPPER_HALF);
        assert_eq!(cell.fg, Color::Rgb(255, 0, 0));
        assert_eq!(cell.bg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn a_transparent_pixel_leaves_the_panel_showing_through() {
        const CLEAR: [u8; 4] = [0, 0, 0, 0];
        const GREEN: [u8; 4] = [0, 255, 0, 255];
        let content = Content::Image(bitmap(1, 2, &[CLEAR, GREEN]));
        let path = PathBuf::from("logo.png");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

        let buf = rendered(&pane, 3, 3);
        let cell = &buf[(1, 1)];

        // The lower block draws the pixel that is there and leaves the ground
        // to the panel, rather than painting it black.
        assert_eq!(cell.symbol(), LOWER_HALF);
        assert_eq!(cell.fg, Color::Rgb(0, 255, 0));
        assert_eq!(cell.bg, Color::Reset);
    }

    #[test]
    fn a_picture_keeps_its_shape_inside_the_grid() {
        // Twice as wide as it is tall, in a grid that is square.
        assert_eq!(scaled(20, 10, 10, 10), (10, 5));
        // Taller than it is wide, so the height is what binds.
        assert_eq!(scaled(10, 20, 10, 10), (5, 10));
        // Already smaller than the grid: filled out, shape kept.
        assert_eq!(scaled(2, 1, 10, 10), (10, 5));
        // Far wider than the grid is tall, and still a row rather than nothing.
        assert_eq!(scaled(1000, 1, 10, 10), (10, 1));
    }

    #[test]
    fn a_cell_no_part_of_the_picture_reaches_is_left_alone() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        // Four wide and one tall, in a body six wide and two rows tall: the
        // picture is one half-pixel high, so the second row is outside it.
        let content = Content::Image(bitmap(4, 1, &[RED, RED, RED, RED]));
        let path = PathBuf::from("stripe.png");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

        let buf = rendered(&pane, 8, 4);

        assert_eq!(buf[(1, 2)].symbol(), " ");
        assert_eq!(buf[(1, 2)].fg, Color::Reset);
        assert_eq!(buf[(1, 2)].bg, Color::Reset);
    }

    /// A cell ten pixels across and twenty tall, which is roughly the shape a
    /// terminal cell has.
    fn graphics() -> Graphics {
        Graphics {
            protocol: Protocol::Kitty,
            cell: CellSize {
                width: NonZeroU16::new(10).expect("ten is not zero"),
                height: NonZeroU16::new(20).expect("twenty is not zero"),
            },
        }
    }

    /// The whole point of measuring in pixels: forty columns of a cell twice
    /// as tall as it is wide hold a square picture in twenty rows, not forty.
    #[test]
    fn a_square_picture_is_placed_in_cells_that_make_it_square() {
        let area = Rect::new(0, 0, 40, 40);
        let placed = placement_area(&bitmap(1, 1, &[[0, 0, 0, 255]]), area, graphics().cell);

        assert_eq!(placed.width, 40);
        assert_eq!(placed.height, 20);
        assert_eq!(
            placed.width * 10,
            placed.height * 20,
            "the placement is square in pixels"
        );
    }

    #[test]
    fn a_placement_is_centred_in_what_it_does_not_fill() {
        let area = Rect::new(4, 6, 40, 40);
        let placed = placement_area(&bitmap(1, 1, &[[0, 0, 0, 255]]), area, graphics().cell);

        assert_eq!(placed.x, 4);
        assert_eq!(placed.y, 6 + (40 - 20) / 2);
    }

    /// A picture far wider than it is tall comes out a row rather than
    /// nothing, and never reaches past the panel.
    #[test]
    fn a_placement_stays_inside_the_panel() {
        let area = Rect::new(0, 0, 6, 4);
        let wide = Content::Image(bitmap(4, 1, &[[0, 0, 0, 255]; 4]));
        let Content::Image(wide) = &wide else {
            unreachable!("it was built as an image")
        };

        let placed = placement_area(wide, area, graphics().cell);

        assert!(placed.width >= 1 && placed.width <= area.width);
        assert!(placed.height >= 1 && placed.height <= area.height);
    }

    /// Where the terminal draws the picture itself, the panel must leave the
    /// cells under it blank rather than half blocks: ratatui writes out only
    /// what changed, and anything left here would outlive the picture.
    #[test]
    fn a_picture_the_terminal_draws_leaves_the_panel_blank() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        let content = Content::Image(bitmap(2, 2, &[RED, RED, RED, RED]));
        let path = PathBuf::from("photo.png");
        let pane = PreviewPane::new(Some(&path), Some(&content), Some(graphics()));

        let area = Rect::new(0, 0, 12, 8);
        let mut buf = Buffer::empty(area);
        let mut placed = None;
        StatefulWidget::render(&pane, area, &mut buf, &mut placed);

        let placed = placed.expect("a picture the terminal draws asks for an area");
        assert!(area.contains(ratatui::layout::Position::new(placed.x, placed.y)));

        // Everything inside the border, not only the cells the picture covers.
        for y in 1..area.height - 1 {
            for x in 1..area.width - 1 {
                assert_eq!(buf[(x, y)].symbol(), " ", "at {x},{y}");
            }
        }
    }

    /// The same panel without a protocol still paints half blocks, so the two
    /// backends are told apart by nothing but this.
    #[test]
    fn the_same_panel_without_a_protocol_paints_half_blocks() {
        const RED: [u8; 4] = [255, 0, 0, 255];
        let content = Content::Image(bitmap(2, 2, &[RED, RED, RED, RED]));
        let path = PathBuf::from("photo.png");
        let pane = PreviewPane::new(Some(&path), Some(&content), None);

        let area = Rect::new(0, 0, 12, 8);
        let mut buf = Buffer::empty(area);
        let mut placed = None;
        StatefulWidget::render(&pane, area, &mut buf, &mut placed);

        assert_eq!(placed, None);
        assert!(
            (1..area.height - 1)
                .flat_map(|y| (1..area.width - 1).map(move |x| (x, y)))
                .any(|(x, y)| buf[(x, y)].symbol() == UPPER_HALF)
        );
    }
}
