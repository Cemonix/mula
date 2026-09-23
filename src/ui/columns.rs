//! What a pane row shows to the right of the name.
//!
//! A cycle rather than a setting, because the columns are worth their cost
//! only while they are being read: [`Columns::Name`] is one `read_dir` per
//! directory, and anything wider is a `stat` per entry on top of it. What the
//! panes ask the reader for follows from the value here, so turning the columns
//! off turns the syscalls off with them.
//!
//! Every cell is padded to a fixed width, so the columns line up down the pane
//! whatever the names above and below are.

use chrono::{DateTime, Local};
use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::fs::directory::{Detail, DirEntry, DirEntryKind};

/// Stands in the size column of the parent, which is never measured.
const NO_SIZE: &str = "\u{2014}";

/// Stands in the size column for a directory whose total is still being
/// measured.
const MEASURING: &str = "\u{2026}";

/// What each row of a listing shows besides its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    Name,
    Size,
    SizeAndTime,
}

impl Columns {
    /// Colour of a size.
    const SIZE: Color = Color::Gray;
    /// Colour of the time, and of a size cell with no size in it. Both are
    /// things the eye should pass over on its way to a name.
    const MUTED: Color = Color::DarkGray;
    /// Columns of the size cell. `1023B` is the widest thing it holds;
    /// [`bytes`] keeps everything else inside it.
    const SIZE_WIDTH: usize = 5;
    /// Columns of the time cell, which is what [`format_time`] draws:
    /// `2026-08-28 18:45`.
    const TIME_WIDTH: usize = 16;
    /// Blank columns between the name and a cell, and between two cells.
    const GAP: usize = 1;

    /// The next setting in the cycle: one column fewer each time, and back to
    /// all of them from the name alone.
    ///
    /// Narrowing rather than widening, because a listing starts with every
    /// column drawn, and what the key is reached for is room for the names.
    pub fn next(self) -> Self {
        match self {
            Columns::SizeAndTime => Columns::Size,
            Columns::Size => Columns::Name,
            Columns::Name => Columns::SizeAndTime,
        }
    }

    /// How much of each entry a listing has to be read with to fill these
    /// columns.
    pub fn detail(self) -> Detail {
        match self {
            Columns::Name => Detail::NamesOnly,
            Columns::Size | Columns::SizeAndTime => Detail::WithMetadata,
        }
    }

    /// Columns taken to the right of the name, the gaps in front of each cell
    /// included. What is left of a row's width after this is the name's.
    pub fn width(self) -> usize {
        match self {
            Columns::Name => 0,
            Columns::Size => Self::GAP + Self::SIZE_WIDTH,
            Columns::SizeAndTime => Self::GAP + Self::SIZE_WIDTH + Self::GAP + Self::TIME_WIDTH,
        }
    }

    /// The cells of `entry`, right-aligned in their columns and together
    /// exactly [`Columns::width`] wide.
    ///
    /// An entry with no metadata leaves its cells blank rather than dropping
    /// them: the row still has to line up with the rows around it. `total` is
    /// what a directory measured to, `None` while it is being measured.
    pub fn cells(self, entry: &DirEntry, total: Option<u64>) -> Vec<Span<'static>> {
        match self {
            Columns::Name => Vec::new(),
            Columns::Size => vec![size_cell(entry, total)],
            Columns::SizeAndTime => vec![size_cell(entry, total), time_cell(entry)],
        }
    }

    /// Whether the size column is drawn, and so whether directories are worth
    /// measuring.
    pub fn show_size(self) -> bool {
        match self {
            Columns::Name => false,
            Columns::Size | Columns::SizeAndTime => true,
        }
    }
}

/// The size column of `entry`, padded to its width.
///
/// A directory shows `total`, what its files add up to; its own `st_size` is
/// the size of the record the filesystem keeps. Until it is measured it draws
/// an ellipsis, and the parent a dash, both in the colour the time is drawn in
/// so they stay quiet under the sizes that mean something.
fn size_cell(entry: &DirEntry, total: Option<u64>) -> Span<'static> {
    let (text, colour) = match (entry.kind, total) {
        (DirEntryKind::Parent, _) => (String::from(NO_SIZE), Columns::MUTED),
        (DirEntryKind::Directory, Some(total)) => (bytes(total), Columns::SIZE),
        (DirEntryKind::Directory, None) => (String::from(MEASURING), Columns::MUTED),
        (DirEntryKind::Symlink | DirEntryKind::File, _) => (
            entry.meta.map(|meta| bytes(meta.size)).unwrap_or_default(),
            Columns::SIZE,
        ),
    };

    cell(&text, Columns::SIZE_WIDTH, colour)
}

/// The time column of `entry`, padded to its width. An entry read without
/// metadata leaves it blank.
fn time_cell(entry: &DirEntry) -> Span<'static> {
    let text = entry
        .meta
        .map(|meta| format_time(meta.modified))
        .unwrap_or_default();

    cell(&text, Columns::TIME_WIDTH, Columns::MUTED)
}

/// One cell: `text` right-aligned in `width` columns, behind the gap that holds
/// it off whatever is in front of it.
fn cell(text: &str, width: usize, colour: Color) -> Span<'static> {
    Span::styled(
        format!("{text:>width$}", width = Columns::GAP + width),
        Style::new().fg(colour),
    )
}

/// A byte count in at most [`Columns::SIZE_WIDTH`] columns: exact below a
/// kibibyte, then scaled to the largest unit it reaches, with one decimal
/// while it stays under ten of them.
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 6] = ["K", "M", "G", "T", "P", "E"];

    if count < 1024 {
        return format!("{count}B");
    }

    let mut scaled = count as f64 / 1024.0;
    let mut unit = 0;
    while scaled >= 1024.0 && unit + 1 < UNITS.len() {
        scaled /= 1024.0;
        unit += 1;
    }

    if scaled < 10.0 {
        format!("{scaled:.1}{}", UNITS[unit])
    } else {
        format!("{scaled:.0}{}", UNITS[unit])
    }
}

/// The local calendar time of `seconds` since the epoch as
/// `YYYY-MM-DD hh:mm`, and an empty string for one no calendar reaches.
///
/// The whole date, every row, whatever the row is: one shape reads down a
/// column, and a listing that abbreviates old files the way `ls` does saves
/// four columns by making the reader work out which shape they are looking at.
/// The four columns are worth having back on a narrow pane, but the answer to
/// a narrow pane is to drop a column, not to shorten one — and eventually to
/// choose which columns fit from the width the pane actually has.
///
/// `Local` is what applies the zone: it reads `TZ`, or the system zone when
/// that is unset, so a file written on the other side of a daylight saving
/// change reads back at the wall clock time it was written at.
fn format_time(seconds: i64) -> String {
    let Some(time) = DateTime::from_timestamp(seconds, 0) else {
        return String::new();
    };

    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
mod columns_tests {
    use super::*;

    use std::{path::Path, sync::Arc};

    use crate::fs::directory::EntryMeta;

    fn entry(kind: DirEntryKind, meta: Option<EntryMeta>) -> DirEntry {
        DirEntry {
            path: Arc::from(Path::new("/one")),
            kind,
            meta,
        }
    }

    fn file(size: u64) -> DirEntry {
        entry(
            DirEntryKind::File,
            Some(EntryMeta {
                size,
                modified: 1_756_400_000,
            }),
        )
    }

    /// What the cell holds without its padding, which is what the tests below
    /// are about.
    fn text_of(cell: Span<'static>) -> String {
        cell.content.trim().to_string()
    }

    #[test]
    fn the_cycle_drops_a_column_at_a_time_and_comes_back_to_all_of_them() {
        assert_eq!(Columns::SizeAndTime.next(), Columns::Size);
        assert_eq!(Columns::SizeAndTime.next().next(), Columns::Name);
        assert_eq!(
            Columns::SizeAndTime.next().next().next(),
            Columns::SizeAndTime
        );
    }

    /// The columns are only worth a `stat` per entry while one of them is
    /// drawn.
    #[test]
    fn only_the_name_is_read_without_metadata() {
        assert_eq!(Columns::Name.detail(), Detail::NamesOnly);
        assert_eq!(Columns::Size.detail(), Detail::WithMetadata);
        assert_eq!(Columns::SizeAndTime.detail(), Detail::WithMetadata);
    }

    #[test]
    fn a_byte_count_is_exact_below_a_kibibyte_and_scaled_above_it() {
        assert_eq!(bytes(0), "0B");
        assert_eq!(bytes(1023), "1023B");
        assert_eq!(bytes(1024), "1.0K");
        assert_eq!(bytes(1536), "1.5K");
        assert_eq!(bytes(10 * 1024), "10K");
        assert_eq!(bytes(3 * 1024 * 1024), "3.0M");
        assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.0G");
    }

    /// The cell is padded to a fixed width, so anything wider would push the
    /// column that follows it out of line.
    #[test]
    fn no_byte_count_outgrows_the_size_column() {
        let mut count: u64 = 1;
        loop {
            let cell = bytes(count);
            assert!(
                cell.len() <= Columns::SIZE_WIDTH,
                "{count} bytes came out as {cell}"
            );
            let Some(next) = count.checked_mul(3) else {
                break;
            };
            count = next;
        }
        assert_eq!(bytes(u64::MAX).len(), 3);
    }

    #[test]
    fn a_directory_draws_what_it_measured_to() {
        assert_eq!(
            text_of(size_cell(&entry(DirEntryKind::Directory, None), Some(2048))),
            "2.0K"
        );
    }

    #[test]
    fn a_directory_still_being_measured_says_so() {
        assert_eq!(
            text_of(size_cell(&entry(DirEntryKind::Directory, None), None)),
            MEASURING
        );
    }

    /// The parent is the way out rather than something in this directory, so
    /// it is never measured.
    #[test]
    fn the_parent_draws_a_dash() {
        assert_eq!(
            text_of(size_cell(&entry(DirEntryKind::Parent, None), None)),
            NO_SIZE
        );
    }

    /// An entry read without metadata, or one that went away before the
    /// `stat`, leaves the cell blank rather than showing a size of zero.
    #[test]
    fn a_file_with_no_metadata_has_an_empty_size_cell() {
        assert_eq!(
            text_of(size_cell(&entry(DirEntryKind::File, None), None)),
            ""
        );
    }

    /// The zone the test runs in is whatever the machine is set to, so the
    /// shape is asserted rather than the hour. The date itself is far enough
    /// from a year boundary to be the same year in every zone on earth.
    #[test]
    fn a_time_comes_out_as_the_whole_local_calendar_date() {
        // Late August 2024.
        let drawn = format_time(1_724_800_000);

        assert_eq!(drawn.len(), Columns::TIME_WIDTH, "it came out as {drawn:?}");
        assert!(drawn.starts_with("2024-08-2"), "it came out as {drawn:?}");
        let separators: String = drawn
            .chars()
            .map(|c| if c.is_ascii_digit() { 'd' } else { c })
            .collect();
        assert_eq!(separators, "dddd-dd-dd dd:dd");
    }

    /// A day the calendar does not reach leaves the cell blank rather than
    /// drawing something wrong beside a real name.
    #[test]
    fn a_time_no_calendar_reaches_has_an_empty_cell() {
        assert_eq!(format_time(i64::MAX), "");
    }

    #[test]
    fn the_cells_of_a_row_are_exactly_as_wide_as_the_columns_they_sit_in() {
        for columns in [Columns::Name, Columns::Size, Columns::SizeAndTime] {
            let drawn: usize = columns
                .cells(&file(1_000_000), None)
                .iter()
                .map(Span::width)
                .sum();
            assert_eq!(drawn, columns.width(), "{columns:?} drew {drawn} columns");
        }
    }

    /// A row whose metadata is missing still has to line up with the rows
    /// around it.
    #[test]
    fn a_row_with_no_metadata_still_fills_its_columns() {
        let drawn: usize = Columns::SizeAndTime
            .cells(&entry(DirEntryKind::File, None), None)
            .iter()
            .map(Span::width)
            .sum();

        assert_eq!(drawn, Columns::SizeAndTime.width());
    }
}
