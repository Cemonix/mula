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
    fs::directory::{DirEntry, Directory},
    ui::icon::Icon,
};

#[derive(Error, Debug)]
pub enum PaneError {
    #[error("No item was selected")]
    NoItemSelected,
    #[error(transparent)]
    IO(#[from] io::Error),
}

/// A listing the pane has asked for, or is about to.
#[derive(Clone, Debug)]
struct Request {
    path: Arc<Path>,
    /// The entry the cursor lands on once the listing arrives, when the
    /// listing holds it. `None` leaves the cursor to [`Pane::set_directory`].
    focus: Option<Arc<Path>>,
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
    pub fn empty(path: Arc<Path>) -> Self {
        let mut pane = Self::new(Directory::new(Arc::clone(&path), Vec::new()));
        pane.want(path, None);
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
    /// out to be. A path that is not a directory comes back as
    /// [`io::ErrorKind::NotADirectory`] and leaves the pane alone.
    ///
    /// Nothing is read here, not even to find out whether the entry is a
    /// directory at all: a symlink says what it points at only once it is
    /// followed, and that question belongs on the reading thread with the
    /// listing it answers. The entry's [`DirEntryKind`] is not consulted
    /// either, since it reports every symlink as a symlink.
    pub fn change_directory(&mut self) -> Result<(), PaneError> {
        let path = Arc::clone(&self.selected_entry()?.path);
        self.want(path, None);
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

    /// Waits for the listing of the directory holding `path`, with the cursor
    /// landing on `path` itself once it arrives. A path with no parent is its
    /// own directory, which is what the filesystem root is.
    ///
    /// A path that is no longer in the listing leaves the cursor wherever
    /// `set_directory` puts it: the walk that found it ran against a disk that
    /// has since moved on, and that is not an error worth reporting.
    pub fn reveal(&mut self, path: &Path) {
        let parent = path.parent().unwrap_or(path);
        self.want(Arc::from(parent), Some(Arc::from(path)));
    }

    /// Waits for the current directory to be read again. The cursor keeps its
    /// index, capped at the last entry of the new listing.
    pub fn refresh(&mut self) {
        let path = Arc::clone(self.directory.path());
        self.want(path, None);
    }

    /// Puts down what the pane is waiting for, replacing whatever it waited
    /// for before. A request that had already gone out is left behind: its
    /// answer belongs to an older generation, and the reader drops it.
    fn want(&mut self, path: Arc<Path>, focus: Option<Arc<Path>>) {
        self.awaited = Awaited::ToSend(Request { path, focus });
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
    /// [`io::ErrorKind::NotADirectory`] is the one failure that is not
    /// reported. Entering the entry under the cursor asks for the listing of
    /// something that may be a file, and finding that out is what the question
    /// was for; every other failure, a directory that cannot be opened
    /// included, is the pane's news to tell.
    ///
    /// Either way the pane stops waiting, so a directory that cannot be read
    /// is not asked for again on every pass that follows.
    pub fn listed(&mut self, listing: io::Result<Directory>) -> Result<(), PaneError> {
        let focus = match mem::replace(&mut self.awaited, Awaited::Nothing) {
            Awaited::Sent(request) => request.focus,
            Awaited::Nothing | Awaited::ToSend(_) => None,
        };

        let directory = match listing {
            Ok(directory) => directory,
            Err(e) if e.kind() == io::ErrorKind::NotADirectory => return Ok(()),
            Err(e) => return Err(e.into()),
        };

        self.set_directory(directory);
        if let Some(focus) = focus {
            self.select_path(&focus);
        }
        Ok(())
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

    /// Draws the directory listing. `focused` colours the border and is passed
    /// in every frame rather than stored, so it cannot drift from the focus the
    /// caller holds.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        selected_items: &HashSet<Arc<Path>>,
        focused: bool,
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

        let list = List::new(self.directory.entries().iter().map(|entry| {
            // Every row, the parent included, is labelled by the last component
            // of its path. The filesystem root has none, so it labels itself.
            let label = match entry.path.file_name() {
                Some(name) => name.to_string_lossy().to_string(),
                None => entry.path.to_string_lossy().to_string(),
            };
            let icon = Icon::icon_for(entry);
            let marked = selected_items.contains(&entry.path);
            let item = ListItem::new(Line::from(vec![
                Span::styled(
                    if marked {
                        Self::MARK_BAR
                    } else {
                        Self::MARK_BLANK
                    },
                    Style::new().fg(Self::MARK),
                ),
                Span::styled(format!("{} ", icon.glyph), Style::new().fg(icon.color)),
                Span::raw(label),
            ]));
            if marked {
                item.style(Style::new().bg(Self::MARKED_BG))
            } else {
                item
            }
        }))
        .style(Color::White)
        .highlight_style(Style::new().bg(Self::CURSOR_BG))
        .highlight_symbol(">");
        frame.render_stateful_widget(list, inner_area, &mut self.list_state);
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
    use crate::fs::directory::DirEntryKind;

    /// Builds a pane over `count` file entries at `/0`, `/1`, … in that order.
    fn pane(count: usize) -> Pane {
        Pane::new(directory(count))
    }

    fn directory(count: usize) -> Directory {
        directory_at("/", count)
    }

    /// Builds a listing of `count` file entries directly under `path`.
    fn directory_at(path: &str, count: usize) -> Directory {
        let root: Arc<Path> = Arc::from(Path::new(path));
        let entries = (0..count)
            .map(|i| DirEntry {
                path: Arc::from(root.join(i.to_string()).as_path()),
                kind: DirEntryKind::File,
            })
            .collect();
        Directory::new(root, entries)
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

        pane.listed(Ok(directory_at("/a", 2))).unwrap();

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
    fn an_entry_that_is_not_a_directory_is_not_worth_reporting() {
        let mut pane = pane(3);
        pane.change_directory().unwrap();
        pane.take_unsent();

        let result = pane.listed(Err(io::Error::from(io::ErrorKind::NotADirectory)));

        assert!(result.is_ok());
        assert_eq!(pane.get_current_dir().as_ref(), Path::new("/"));
        assert!(pane.take_unsent().is_none());
    }

    #[test]
    fn revealing_puts_the_cursor_on_the_target_once_the_listing_arrives() {
        let mut pane = pane(3);
        pane.reveal(Path::new("/b/2"));

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/b")));
        pane.listed(Ok(directory_at("/b", 4))).unwrap();

        assert_eq!(
            pane.selected_entry().unwrap().path.as_ref(),
            Path::new("/b/2")
        );
    }

    #[test]
    fn a_request_replaced_before_it_was_sent_asks_only_for_the_last_one() {
        let mut pane = pane(3);
        pane.reveal(Path::new("/b/2"));
        pane.refresh();

        assert_eq!(pane.take_unsent().as_deref(), Some(Path::new("/")));
        assert!(pane.take_unsent().is_none());
    }
}
