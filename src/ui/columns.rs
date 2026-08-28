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

use std::mem::MaybeUninit;

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use crate::fs::directory::{Detail, DirEntry, DirEntryKind};

/// What each row of a listing shows besides its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Columns {
    Name,
    Size,
    SizeAndTime,
}

impl Columns {
    /// Colour of the size cell.
    const SIZE: Color = Color::Gray;
    /// Colour of the modification time, which is the column a reader scans
    /// past most often.
    const TIME: Color = Color::DarkGray;
    /// Columns of the size cell. `1023B` and `<DIR>` are the widest things it
    /// holds; [`size_cell`] keeps everything else inside them.
    const SIZE_WIDTH: usize = 5;
    /// Columns of the time cell, which is `2026-08-28 18:45`.
    const TIME_WIDTH: usize = 16;
    /// Blank columns between the name and a cell, and between two cells.
    const GAP: usize = 1;

    /// The next setting in the cycle, wrapping back to the name alone.
    pub fn next(self) -> Self {
        match self {
            Columns::Name => Columns::Size,
            Columns::Size => Columns::SizeAndTime,
            Columns::SizeAndTime => Columns::Name,
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
    /// them: the row still has to line up with the rows around it.
    pub fn cells(self, entry: &DirEntry) -> Vec<Span<'static>> {
        let mut cells = Vec::new();
        if self == Columns::Name {
            return cells;
        }

        cells.push(Span::styled(
            format!(
                "{:>width$}",
                size_cell(entry),
                width = Self::GAP + Self::SIZE_WIDTH
            ),
            Style::new().fg(Self::SIZE),
        ));

        if self == Columns::SizeAndTime {
            let time = entry.meta.map(|meta| time_cell(meta.modified));
            cells.push(Span::styled(
                format!(
                    "{:>width$}",
                    time.unwrap_or_default(),
                    width = Self::GAP + Self::TIME_WIDTH
                ),
                Style::new().fg(Self::TIME),
            ));
        }

        cells
    }
}

/// What goes in the size column. A directory has none worth showing: its own
/// `st_size` is the size of the record the filesystem keeps, not of anything
/// inside it, so it says what it is instead.
fn size_cell(entry: &DirEntry) -> String {
    match entry.kind {
        DirEntryKind::Parent | DirEntryKind::Directory => String::from("<DIR>"),
        DirEntryKind::Symlink | DirEntryKind::File => {
            entry.meta.map(|meta| bytes(meta.size)).unwrap_or_default()
        }
    }
}

/// A byte count in at most [`Columns::SIZE_WIDTH`] columns: exact below a
/// kibibyte, then scaled to the largest unit it reaches, with one decimal
/// while it stays under ten of them.
fn bytes(count: u64) -> String {
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

/// The local calendar time of `seconds` since the epoch, and an empty cell for
/// one that cannot be broken down.
fn time_cell(seconds: i64) -> String {
    local(seconds).map(format_local).unwrap_or_default()
}

/// `seconds` in the local zone.
///
/// `localtime_r` is what applies that zone: it reads `TZ`, or the system zone
/// when that is unset, so a file written on the other side of a daylight saving
/// change reads back at the wall clock time it was written at.
fn local(seconds: i64) -> Option<libc::tm> {
    let seconds = seconds as libc::time_t;
    let mut broken = MaybeUninit::<libc::tm>::uninit();

    // SAFETY: both pointers are to locals that outlive the call, and
    // `localtime_r` writes through the second alone.
    let filled = unsafe { libc::localtime_r(&seconds, broken.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }

    // SAFETY: a non-null return says the struct was filled in.
    Some(unsafe { broken.assume_init() })
}

/// A broken-down time as `YYYY-MM-DD hh:mm`. `tm_year` counts from 1900 and
/// `tm_mon` from zero, which is the whole of what this puts back.
fn format_local(broken: libc::tm) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        broken.tm_year + 1900,
        broken.tm_mon + 1,
        broken.tm_mday,
        broken.tm_hour,
        broken.tm_min
    )
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

    #[test]
    fn the_cycle_comes_back_to_the_name_alone() {
        assert_eq!(Columns::Name.next().next().next(), Columns::Name);
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
    fn a_directory_says_so_rather_than_showing_the_size_of_its_record() {
        assert_eq!(size_cell(&entry(DirEntryKind::Directory, None)), "<DIR>");
        assert_eq!(size_cell(&entry(DirEntryKind::Parent, None)), "<DIR>");
    }

    /// An entry read without metadata, or one that went away before the
    /// `stat`, leaves the cell blank rather than showing a size of zero.
    #[test]
    fn a_file_with_no_metadata_has_an_empty_size_cell() {
        assert_eq!(size_cell(&entry(DirEntryKind::File, None)), "");
    }

    #[test]
    fn a_broken_down_time_is_written_out_as_a_calendar_date() {
        // SAFETY: `libc::tm` is a struct of integers, and a zeroed one is the
        // start of an ordinary day.
        let mut broken: libc::tm = unsafe { std::mem::zeroed() };
        broken.tm_year = 126;
        broken.tm_mon = 7;
        broken.tm_mday = 28;
        broken.tm_hour = 18;
        broken.tm_min = 45;

        assert_eq!(format_local(broken), "2026-08-28 18:45");
    }

    #[test]
    fn a_time_cell_is_as_wide_as_the_column_that_holds_it() {
        assert_eq!(time_cell(1_756_400_000).len(), Columns::TIME_WIDTH);
    }

    #[test]
    fn the_cells_of_a_row_are_exactly_as_wide_as_the_columns_they_sit_in() {
        for columns in [Columns::Name, Columns::Size, Columns::SizeAndTime] {
            let drawn: usize = columns
                .cells(&file(1_000_000))
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
            .cells(&entry(DirEntryKind::File, None))
            .iter()
            .map(Span::width)
            .sum();

        assert_eq!(drawn, Columns::SizeAndTime.width());
    }
}
