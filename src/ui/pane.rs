use std::{collections::HashSet, io, mem, path::Path, sync::Arc};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState},
};
use thiserror::Error;

use crate::{
    fs::{
        directory::{Detail, DirEntry, Directory},
        listing::{Kind, Listed},
    },
    ui::{self, columns::Columns, icon::Icon},
};

#[derive(Error, Debug)]
pub enum PaneError {
    #[error("No item was selected")]
    NoItemSelected,
    #[error(transparent)]
    IO(#[from] io::Error),
}

/// Why a listing was asked for. Only entering an entry turns "that is not a
/// directory" into something to open; a pane catching up with the disk finds a
/// directory replaced by a file and says nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intent {
    Enter,
    Refresh,
}

/// A listing the pane has asked for, or is about to.
#[derive(Clone, Debug)]
struct Request {
    path: Arc<Path>,
    /// The entry the cursor lands on once the listing arrives, when the
    /// listing holds it. `None` leaves the cursor to [`Pane::set_directory`].
    focus: Option<Arc<Path>>,
    intent: Intent,
}

/// What the pane wants done about an answer, beyond what it did with it
/// itself.
#[derive(Debug)]
pub enum Entered {
    /// Nothing. The listing is on screen, or the answer was to a question the
    /// pane has moved on from.
    Nothing,
    /// The entry the user entered is not a directory, and is this to open.
    Open { path: Arc<Path>, kind: Kind },
}

/// The listing a pane is waiting for. A pane showing what it wants awaits
/// `Nothing`; otherwise it is one request, either still to be handed over or
/// already on its way.
///
/// The two are held apart so the loop can send a request without sending it
/// again on every pass that follows.
#[derive(Debug)]
enum Awaited {
    Nothing,
    ToSend(Request),
    Sent(Request),
}

#[derive(Debug)]
pub struct Pane {
    directory: Directory,
    list_state: ListState,
    awaited: Awaited,
}

impl Pane {
    /// Border colour of the pane the cursor is in.
    const FOCUSED: Color = Color::Blue;
    /// Border colour of every other pane.
    pub(crate) const UNFOCUSED: Color = Color::White;
    /// Colour of the bar drawn in the mark column of a marked row.
    const MARK: Color = Color::Rgb(0x98, 0xc3, 0x79);
    /// Background of a marked row.
    const MARKED_BG: Color = Color::Rgb(0x2a, 0x2a, 0x2d);
    /// Background of the row the cursor is on.
    const CURSOR_BG: Color = Color::Rgb(0x33, 0x3c, 0x4d);
    /// Drawn in the mark column of a marked row.
    const MARK_BAR: &'static str = "\u{258c} ";
    /// Holds the mark column open on an unmarked row.
    const MARK_BLANK: &'static str = "  ";
    /// Drawn to the left of the row the cursor is on, and so a column every
    /// row gives up whether it is the one or not.
    const HIGHLIGHT: &'static str = ">";
    /// Columns the mark takes. Both of the strings it draws are this wide, so
    /// marking a row cannot shift the name beside it.
    const MARK_WIDTH: usize = 2;

    /// Opens `directory` with its first entry selected, or with nothing
    /// selected while the listing is empty.
    pub fn new(directory: Directory) -> Self {
        Self {
            list_state: ListState::default().with_selected(clamped(None, directory.len())),
            directory,
            awaited: Awaited::Nothing,
        }
    }

    /// A pane with nothing in it, waiting for its first listing. `path` is what
    /// the border shows until that listing arrives.
    ///
    /// The empty listing it stands on is a [`Detail::NamesOnly`] one, which
    /// costs nothing: the pane is already waiting, so the request that goes out
    /// carries whatever detail the columns want by then.
    pub fn empty(path: Arc<Path>) -> Self {
        let mut pane = Self::new(Directory::new(
            Arc::clone(&path),
            Vec::new(),
            Detail::NamesOnly,
        ));
        pane.want(path, None, Intent::Refresh);
        pane
    }

    pub fn get_current_dir(&self) -> &Arc<Path> {
        self.directory.path()
    }

    pub fn selected_entry(&self) -> Result<&DirEntry, PaneError> {
        self.list_state
            .selected()
            .and_then(|idx| self.directory.get(idx))
            .ok_or(PaneError::NoItemSelected)
    }

    /// Waits for the listing of the selected entry, whatever that entry turns
    /// out to be. A path that is not a directory comes back saying what it is
    /// instead, which [`Pane::listed`] hands on as [`Entered::Open`].
    ///
    /// Nothing is read here, not even to find out whether the entry is a
    /// directory at all: a symlink says what it points at only once it is
    /// followed, and that question belongs on the reading thread with the
    /// listing it answers. The entry's [`DirEntryKind`] is not consulted
    /// either, since it reports every symlink as a symlink.
    pub fn change_directory(&mut self) -> Result<(), PaneError> {
        let path = Arc::clone(&self.selected_entry()?.path);
        self.want(path, None, Intent::Enter);
        Ok(())
    }

    /// Moves the cursor one item up, wrapping from the first item to the last.
    /// Does nothing while nothing is selected.
    pub fn select_prev(&mut self) {
        if let Some(idx) = self.list_state.selected() {
            let len = self.directory.len();
            self.list_state.select(Some((idx + len - 1) % len));
        }
    }

    /// Moves the cursor one item down, wrapping from the last item to the first.
    /// Does nothing while nothing is selected.
    pub fn select_next(&mut self) {
        if let Some(idx) = self.list_state.selected() {
            let len = self.directory.len();
            self.list_state.select(Some((idx + 1) % len));
        }
    }

    /// Moves the cursor to the first item. Does nothing while nothing is
    /// selected.
    pub fn select_first(&mut self) {
        if self.list_state.selected().is_some() {
            self.list_state.select(Some(0));
        }
    }

    /// Moves the cursor to the last item. Does nothing while nothing is
    /// selected.
    pub fn select_last(&mut self) {
        if self.list_state.selected().is_some() {
            self.list_state.select(Some(self.directory.len() - 1));
        }
    }

    /// Waits for the listing of the directory holding `path`, with the cursor
    /// landing on `path` itself once it arrives. A path with no parent is its
    /// own directory, which is what the filesystem root is.
    ///
    /// A path that is no longer in the listing leaves the cursor wherever
    /// `set_directory` puts it: the walk that found it ran against a disk that
    /// has since moved on, and that is not an error worth reporting.
    pub fn reveal(&mut self, path: &Path) {
        let parent = path.parent().unwrap_or(path);
        self.want(Arc::from(parent), Some(Arc::from(path)), Intent::Refresh);
    }

    /// Waits for the current directory to be read again when the listing on
    /// screen was read with less than `detail`. A column cannot be filled from
    /// a listing that never read what goes in it, and one read with more than
    /// is wanted needs nothing: turning the columns back off draws fewer of
    /// them rather than reading the directory again.
    ///
    /// A pane already waiting is left alone. Its request goes out under the
    /// detail wanted at the moment it is handed over, and if it went out
    /// already, the answer lands here and the next pass asks again.
    pub fn want_detail(&mut self, detail: Detail) {
        if matches!(self.awaited, Awaited::Nothing) && self.directory.detail() < detail {
            self.refresh();
        }
    }

    /// Waits for the current directory to be read again. The cursor keeps its
    /// index, capped at the last entry of the new listing.
    pub fn refresh(&mut self) {
        let path = Arc::clone(self.directory.path());
        self.want(path, None, Intent::Refresh);
    }

    /// Puts down what the pane is waiting for, replacing whatever it waited
    /// for before. A request that had already gone out is left behind: its
    /// answer belongs to an older generation, and the reader drops it.
    fn want(&mut self, path: Arc<Path>, focus: Option<Arc<Path>>, intent: Intent) {
        self.awaited = Awaited::ToSend(Request {
            path,
            focus,
            intent,
        });
    }

    /// The listing the pane is waiting for and has not asked for yet, marked
    /// as asked on the way out so the next pass of the loop does not send it
    /// again.
    pub fn take_unsent(&mut self) -> Option<Arc<Path>> {
        let Awaited::ToSend(request) = &self.awaited else {
            return None;
        };

        let request = request.clone();
        let path = Arc::clone(&request.path);
        self.awaited = Awaited::Sent(request);
        Some(path)
    }

    /// Puts a request that has gone out back to being unsent, so it is asked
    /// again. What the pane waits for does not change, only whether the reader
    /// has been told about it — a panel serves every one of its tabs from one
    /// reader, and the tab that loses it has to ask a second time.
    pub fn unsend(&mut self) {
        if let Awaited::Sent(request) = &self.awaited {
            self.awaited = Awaited::ToSend(request.clone());
        }
    }

    /// Folds in the answer to the read the pane was waiting for: the listing
    /// replaces what is on screen, and a failure leaves the pane on the one it
    /// already had and comes back to be reported.
    ///
    /// An answer of "not a directory" is not a failure. Entering the entry
    /// under the cursor asks for the listing of something that may be a file,
    /// and finding that out is what the question was for; it comes back as
    /// [`Entered::Open`] for the caller to open. A pane merely catching up with
    /// the disk asked no such question and is told nothing.
    ///
    /// Either way the pane stops waiting, so a directory that cannot be read
    /// is not asked for again on every pass that follows.
    pub fn listed(&mut self, listing: io::Result<Listed>) -> Result<Entered, PaneError> {
        let request = match mem::replace(&mut self.awaited, Awaited::Nothing) {
            Awaited::Sent(request) => Some(request),
            Awaited::Nothing | Awaited::ToSend(_) => None,
        };

        match listing? {
            Listed::Directory(directory) => {
                self.set_directory(directory);
                if let Some(focus) = request.and_then(|request| request.focus) {
                    self.select_path(&focus);
                }
                Ok(Entered::Nothing)
            }
            Listed::NotADirectory(kind) => Ok(match request {
                Some(request) if request.intent == Intent::Enter => Entered::Open {
                    path: request.path,
                    kind,
                },
                _ => Entered::Nothing,
            }),
        }
    }

    /// Replaces the listing and brings the selection back into range.
    fn set_directory(&mut self, directory: Directory) {
        let selected = clamped(self.list_state.selected(), directory.len());
        self.directory = directory;
        self.list_state.select(selected);
    }

    /// Puts the cursor on `path` when the listing holds it, and leaves it
    /// where `set_directory` put it when it does not.
    fn select_path(&mut self, path: &Path) {
        if let Some(index) = self
            .directory
            .entries()
            .iter()
            .position(|entry| entry.path.as_ref() == path)
        {
            self.list_state.select(Some(index));
        }
    }

    /// Draws the directory listing. `focused` colours the border and `columns`
    /// says what each row shows beside its name; both are passed in every frame
    /// rather than stored, so neither can drift from what the caller holds.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        selected_items: &HashSet<Arc<Path>>,
        focused: bool,
        columns: Columns,
    ) {
        let title = Line::from(self.directory.path().to_string_lossy().to_string().bold());
        let block = Block::bordered()
            .title_top(title.centered())
            .border_style(if focused {
                Self::FOCUSED
            } else {
                Self::UNFOCUSED
            });

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // What a row has to lay itself out in: the highlight symbol is drawn
        // beside every row, whether the cursor is on it or not.
        let row_width = usize::from(inner_area.width).saturating_sub(Self::HIGHLIGHT.len());

        let list = List::new(self.directory.entries().iter().map(|entry| {
            // Every row, the parent included, is labelled by the last component
            // of its path.
            let label = ui::name_of(&entry.path);
            let icon = Icon::icon_for(entry);
            let marked = selected_items.contains(&entry.path);
            let item = ListItem::new(Self::row(entry, marked, columns, row_width));
            if marked {
                item.style(Style::new().bg(Self::MARKED_BG))
            } else {
                item
            }
        }))
        .style(Color::White)
        .highlight_style(Style::new().bg(Self::CURSOR_BG))
        .highlight_symbol(Self::HIGHLIGHT);
        frame.render_stateful_widget(list, inner_area, &mut self.list_state);
    }

    /// One row, `row_width` columns wide: the mark, the icon, the name padded
    /// out to whatever is left, and the cells of `columns` behind it.
    ///
    /// The name is padded rather than the cells positioned, so a row is built
    /// once, left to right, and the columns line up because everything else on
    /// the row is measured.
    ///
    /// The icon is measured rather than counted on: a Nerd Font glyph from the
    /// supplementary private use area is two columns to `Span::width`, one from
    /// the basic plane is one, and a row that assumed either would put the
    /// columns of the rows below it somewhere else.
    fn row(entry: &DirEntry, marked: bool, columns: Columns, row_width: usize) -> Line<'static> {
        // Every row, the parent included, is labelled by the last component of
        // its path. The filesystem root has none, so it labels itself.
        let label = match entry.path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => entry.path.to_string_lossy().to_string(),
        };
        let icon = Icon::icon_for(entry);
        let icon = Span::styled(format!("{} ", icon.glyph), Style::new().fg(icon.color));

        // A pane too narrow to hold the columns leaves the name none at all
        // rather than borrowing any back from them.
        let name_width =
            row_width.saturating_sub(Self::MARK_WIDTH + icon.width() + columns.width());
        let label = ui::clip(&label, name_width);
        let padding = name_width.saturating_sub(Span::raw(&label).width());

        let mut spans = vec![
            Span::styled(
                if marked {
                    Self::MARK_BAR
                } else {
                    Self::MARK_BLANK
                },
                Style::new().fg(Self::MARK),
            ),
            icon,
            Span::raw(label),
            Span::raw(" ".repeat(padding)),
        ];
        spans.extend(columns.cells(entry));

        Line::from(spans)
    }
}

/// Fits a selection to a listing of `len` entries: nothing while the listing is
/// empty, the first entry when nothing was selected, otherwise the previous
/// index capped at the last entry.
fn clamped(selected: Option<usize>, len: usize) -> Option<usize> {
    match len {
        0 => None,
        len => Some(selected.unwrap_or(0).min(len - 1)),
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    use ratatui::{buffer::Buffer, widgets::Widget};

    use crate::fs::directory::{DirEntryKind, EntryMeta};

    /// Builds a pane over `count` file entries at `/0`, `/1`, … in that order.
    fn pane(count: usize) -> Pane {
        Pane::new(directory(count))
    }

    fn directory(count: usize) -> Directory {
        directory_at("/", count)
    }

    /// Builds a listing of `count` file entries directly under `path`, read
    /// with names alone.
    fn directory_at(path: &str, count: usize) -> Directory {
        detailed_directory_at(path, count, Detail::NamesOnly)
    }

    fn detailed_directory_at(path: &str, count: usize, detail: Detail) -> Directory {
        let root: Arc<Path> = Arc::from(Path::new(path));
        let entries = (0..count)
            .map(|i| DirEntry {
                path: Arc::from(root.join(i.to_string()).as_path()),
                kind: DirEntryKind::File,
                meta: None,
            })
            .collect();
        Directory::new(root, entries, detail)
    }

    #[test]
    fn the_cursor_wraps_from_the_last_item_to_the_first() {
        let mut pane = pane(3);
        for _ in 0..3 {
            pane.select_next();
        }
        assert_eq!(pane.list_state.selected(), Some(0));
    }

    #[test]
    fn the_cursor_wraps_from_the_first_item_to_the_last() {
        let mut pane = pane(3);
        pane.select_prev();
        assert_eq!(pane.list_state.selected(), Some(2));
    }

    #[test]
    fn the_cursor_reaches_either_end_from_anywhere() {
        let mut pane = pane(5);
        pane.select_next();
        pane.select_next();

        pane.select_last();
        assert_eq!(pane.list_state.selected(), Some(4));

        pane.select_first();
        assert_eq!(pane.list_state.selected(), Some(0));
    }

    /// The index has to be worked out here rather than left to
    /// `ListState::select_last`, which stores `usize::MAX` until a render cuts
    /// it down; everything reading the selection before that render would find
    /// no entry there.
    #[test]
    fn jumping_to_the_last_item_leaves_an_entry_under_the_cursor() {
        let mut pane = pane(3);
        pane.select_last();

        assert_eq!(
            pane.selected_entry().unwrap().path.as_ref(),
            Path::new("/2")
        );
    }

    #[test]
    fn an_empty_listing_has_no_end_to_jump_to() {
        let mut pane = pane(0);
        pane.select_first();
        pane.select_last();

        assert_eq!(pane.list_state.selected(), None);
    }

    #[test]
    fn a_shorter_listing_pulls_the_selection_back_to_the_last_item() {
        let mut pane = pane(5);
        pane.select_prev();
        pane.set_directory(directory(2));
        assert_eq!(pane.list_state.selected(), Some(1));
        assert_eq!(
            pane.selected_entry().unwrap().path.as_ref(),
            Path::new("/1")
        );
    }

    #[test]
    fn an_empty_listing_selects_nothing_and_the_cursor_stays_put() {
        let mut pane = pane(0);
        pane.select_next();
        pane.select_prev();
        assert_eq!(pane.list_state.selected(), None);
        assert!(matches!(
            pane.selected_entry(),
            Err(PaneError::NoItemSelected)
        ));
    }

    #[test]
    fn a_pane_with_no_listing_shows_its_path_and_asks_for_it() {
        let mut pane = Pane::empty(Arc::from(Path::new("/a")));

        assert_eq!(pane.get_current_dir().as_ref(), Path::new("/a"));
        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/a")));
    }

    #[test]
    fn a_listing_is_asked_for_once() {
        let mut pane = Pane::empty(Arc::from(Path::new("/a")));
        pane.take_unsent();

        assert!(pane.take_unsent().is_none());
    }

    #[test]
    fn an_answer_replaces_the_listing_and_ends_the_wait() {
        let mut pane = Pane::empty(Arc::from(Path::new("/a")));
        pane.take_unsent();

        pane.listed(Ok(Listed::Directory(directory_at("/a", 2))))
            .unwrap();

        assert_eq!(pane.directory.len(), 2);
        assert!(pane.take_unsent().is_none());
    }

    #[test]
    fn a_failed_answer_leaves_the_pane_where_it_was_and_does_not_ask_again() {
        let mut pane = pane(3);
        pane.refresh();
        pane.take_unsent();

        let result = pane.listed(Err(io::Error::from(io::ErrorKind::PermissionDenied)));

        assert!(result.is_err());
        assert_eq!(pane.get_current_dir().as_ref(), Path::new("/"));
        assert_eq!(pane.directory.len(), 3);
        assert!(pane.take_unsent().is_none());
    }

    #[test]
    fn entering_asks_for_the_entry_under_the_cursor() {
        let mut pane = pane(3);
        pane.select_next();
        pane.change_directory().unwrap();

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/1")));
    }

    #[test]
    fn entering_something_that_is_not_a_directory_hands_it_back_to_be_opened() {
        let mut pane = pane(3);
        pane.select_next();
        pane.change_directory().unwrap();
        pane.take_unsent();

        let entered = pane.listed(Ok(Listed::NotADirectory(Kind::Text))).unwrap();

        assert!(matches!(entered, Entered::Open { path, kind }
                if path.as_ref() == Path::new("/1") && kind == Kind::Text));
        assert_eq!(pane.get_current_dir().as_ref(), Path::new("/"));
        assert!(pane.take_unsent().is_none());
    }

    /// A pane catching up with the disk asked no question about opening
    /// anything. Finding a directory replaced by a file leaves it where it is.
    #[test]
    fn a_refresh_onto_something_that_is_not_a_directory_opens_nothing() {
        let mut pane = pane(3);
        pane.refresh();
        pane.take_unsent();

        let entered = pane.listed(Ok(Listed::NotADirectory(Kind::Text))).unwrap();

        assert!(matches!(entered, Entered::Nothing));
        assert_eq!(pane.get_current_dir().as_ref(), Path::new("/"));
        assert!(pane.take_unsent().is_none());
    }

    #[test]
    fn revealing_puts_the_cursor_on_the_target_once_the_listing_arrives() {
        let mut pane = pane(3);
        pane.reveal(Path::new("/b/2"));

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/b")));
        pane.listed(Ok(Listed::Directory(directory_at("/b", 4))))
            .unwrap();

        assert_eq!(
            pane.selected_entry().unwrap().path.as_ref(),
            Path::new("/b/2")
        );
    }

    #[test]
    fn a_pane_asks_again_for_a_listing_that_is_thinner_than_the_columns_need() {
        let mut pane = pane(3);

        pane.want_detail(Detail::WithMetadata);

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/")));
    }

    /// Turning the columns back off draws fewer of them. Reading the directory
    /// again for that would charge the whole listing for showing less.
    #[test]
    fn a_pane_holding_more_than_the_columns_need_asks_for_nothing() {
        let mut pane = Pane::new(detailed_directory_at("/", 3, Detail::WithMetadata));

        pane.want_detail(Detail::NamesOnly);
        assert!(pane.take_unsent().is_none());

        pane.want_detail(Detail::WithMetadata);
        assert!(pane.take_unsent().is_none());
    }

    /// The request that is already on its way goes out under whatever detail
    /// is wanted when it is handed over, so replacing it here would only throw
    /// away where the pane was going.
    #[test]
    fn a_pane_already_waiting_keeps_what_it_was_waiting_for() {
        let mut pane = pane(3);
        pane.reveal(Path::new("/b/2"));

        pane.want_detail(Detail::WithMetadata);

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/b")));
    }

    #[test]
    fn a_request_replaced_before_it_was_sent_asks_only_for_the_last_one() {
        let mut pane = pane(3);
        pane.reveal(Path::new("/b/2"));
        pane.refresh();

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/")));
        assert!(pane.take_unsent().is_none());
    }

    /// Draws one row the way the list would, into a buffer as wide as the
    /// room a row is given, and reads the cells back. Alignment is the whole
    /// point of the columns and cannot be seen anywhere else.
    fn drawn_row(entry: &DirEntry, columns: Columns, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);

        Pane::row(entry, false, columns, usize::from(width)).render(area, &mut buf);
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    fn file(name: &str, size: u64) -> DirEntry {
        DirEntry {
            path: Arc::from(Path::new("/").join(name).as_path()),
            kind: DirEntryKind::File,
            meta: Some(EntryMeta {
                size,
                modified: 1_756_400_000,
            }),
        }
    }

    #[test]
    fn a_row_showing_the_name_alone_draws_no_size() {
        let row = drawn_row(&file("one.txt", 2048), Columns::Name, 30);

        assert!(row.contains("one.txt"), "the row was {row:?}");
        assert!(!row.contains("2.0K"), "the row was {row:?}");
    }

    #[test]
    fn the_size_ends_at_the_right_edge_of_the_row() {
        let row = drawn_row(&file("one.txt", 2048), Columns::Size, 30);

        assert!(row.contains("one.txt"), "the row was {row:?}");
        assert!(row.ends_with(" 2.0K"), "the row was {row:?}");
    }

    /// The name gives way, not the columns: a column pushed off the edge is
    /// the one thing the row cannot afford to lose.
    #[test]
    fn a_name_too_long_for_what_is_left_is_clipped_rather_than_pushing_the_size_out() {
        let row = drawn_row(&file("a-very-long-file-name.txt", 4096), Columns::Size, 24);

        assert!(row.contains('\u{2026}'), "the row was {row:?}");
        assert!(row.ends_with(" 4.0K"), "the row was {row:?}");
    }

    /// Same extension on both, so the icons are the same width: a row is laid
    /// out around whatever the icon measures, and two different icons are two
    /// different questions.
    #[test]
    fn two_rows_of_different_name_lengths_line_their_columns_up() {
        let short = drawn_row(&file("a.txt", 1024), Columns::SizeAndTime, 48);
        let long = drawn_row(&file("a-longer-name.txt", 1024), Columns::SizeAndTime, 48);

        assert_eq!(
            short.find("1.0K"),
            long.find("1.0K"),
            "the rows were {short:?} and {long:?}"
        );
    }
}
