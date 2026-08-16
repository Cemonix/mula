use std::{collections::HashSet, io, path::Path, rc::Rc};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Stylize},
    text::Line,
    widgets::{Block, List, ListState},
};
use thiserror::Error;

use crate::directory::{DirEntry, DirEntryKind, Directory};

#[derive(Error, Debug)]
pub enum PaneError {
    #[error("No item was selected")]
    NoItemSelected,
    #[error(transparent)]
    IO(#[from] io::Error),
}

#[derive(Debug)]
pub struct Pane {
    directory: Directory,
    list_state: ListState,
}

impl Pane {
    /// Border colour of the pane the cursor is in.
    const FOCUSED: Color = Color::Blue;
    /// Border colour of every other pane.
    const UNFOCUSED: Color = Color::White;

    /// Opens `directory` with its first entry selected, or with nothing
    /// selected while the listing is empty.
    pub fn new(directory: Directory) -> Self {
        Self {
            list_state: ListState::default().with_selected(clamped(None, directory.len())),
            directory,
        }
    }

    pub fn get_current_dir(&self) -> &Rc<Path> {
        self.directory.path()
    }

    pub fn selected_entry(&self) -> Result<&DirEntry, PaneError> {
        self.list_state
            .selected()
            .and_then(|idx| self.directory.get(idx))
            .ok_or(PaneError::NoItemSelected)
    }

    /// Reads the selected entry when it is a directory or the parent, and moves
    /// the pane into it. Leaves the pane alone for any other kind.
    pub fn change_directory(&mut self) -> Result<(), PaneError> {
        let entry = self.selected_entry()?;
        match entry.kind {
            DirEntryKind::Parent | DirEntryKind::Directory => {
                let path = Rc::clone(&entry.path);
                self.set_directory(Directory::read(path)?);
            }
            _ => (),
        }
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

    /// Reads the current directory again. The cursor keeps its index, capped at
    /// the last entry of the new listing.
    pub fn refresh(&mut self) -> Result<(), PaneError> {
        let path = Rc::clone(self.directory.path());
        self.set_directory(Directory::read(path)?);
        Ok(())
    }

    /// Replaces the listing and brings the selection back into range.
    fn set_directory(&mut self, directory: Directory) {
        let selected = clamped(self.list_state.selected(), directory.len());
        self.directory = directory;
        self.list_state.select(selected);
    }

    /// Draws the directory listing. `focused` colours the border and is passed
    /// in every frame rather than stored, so it cannot drift from the focus the
    /// caller holds.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        selected_items: &HashSet<Rc<Path>>,
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
            let file_name = match &entry.kind {
                DirEntryKind::Parent => "..".to_string(),
                _ => match entry.path.file_name() {
                    Some(n) => n.to_string_lossy().to_string(),
                    None => {
                        tracing::warn!(path = ?entry.path, "no file name");
                        entry.path.to_string_lossy().to_string()
                    }
                },
            };
            if selected_items.contains(&entry.path) {
                format!("• {}", file_name)
            } else {
                file_name
            }
        }))
        .style(Color::White)
        .highlight_style(Modifier::REVERSED)
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

    /// Builds a pane over `count` file entries at `/0`, `/1`, … in that order.
    fn pane(count: usize) -> Pane {
        Pane::new(directory(count))
    }

    fn directory(count: usize) -> Directory {
        let entries = (0..count)
            .map(|i| DirEntry {
                path: Rc::from(Path::new(&format!("/{i}"))),
                kind: DirEntryKind::File,
            })
            .collect();
        Directory::new(Rc::from(Path::new("/")), entries)
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
}
