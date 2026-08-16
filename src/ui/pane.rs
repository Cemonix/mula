use std::{collections::HashSet, io, path::Path, rc::Rc};

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Stylize},
    text::Line,
    widgets::{Block, List, ListState},
};
use thiserror::Error;

use crate::fs::{DirEntry, DirEntryKind, list_dir_entries};

#[derive(Error, Debug)]
pub enum PaneError {
    #[error("No item was selected")]
    NoItemSelected,
    #[error(transparent)]
    IO(#[from] io::Error),
}

#[derive(Debug)]
pub struct Pane {
    current_dir: Rc<Path>,
    list_state: ListState,
    items: Vec<DirEntry>,
}

impl Pane {
    /// Border colour of the pane the cursor is in.
    const FOCUSED: Color = Color::Blue;
    /// Border colour of every other pane.
    const UNFOCUSED: Color = Color::White;

    pub fn new(current_dir: Rc<Path>) -> Self {
        Self {
            current_dir,
            list_state: ListState::default().with_selected(Some(0)),
            items: Vec::new(),
        }
    }

    pub fn with_items(mut self, items: Vec<DirEntry>) -> Self {
        self.set_items(items);
        self
    }

    pub fn get_current_dir(&self) -> &Rc<Path> {
        &self.current_dir
    }

    pub fn selected_entry(&self) -> Result<&DirEntry, PaneError> {
        let selected_idx = self
            .list_state
            .selected()
            .ok_or(PaneError::NoItemSelected)?;
        Ok(&self.items[selected_idx])
    }

    pub fn change_directory(&mut self) -> Result<(), PaneError> {
        let entry = self.selected_entry()?;
        match entry.kind {
            DirEntryKind::Parent | DirEntryKind::Directory => {
                let path = Rc::clone(&entry.path);
                self.set_items(list_dir_entries(&path)?);
                self.current_dir = path;
            }
            _ => (),
        }
        Ok(())
    }

    pub fn select_prev(&mut self) {
        self.list_state.select_previous();
    }

    pub fn select_next(&mut self) {
        self.list_state.select_next();
    }

    pub fn refresh(&mut self) -> Result<(), PaneError> {
        self.set_items(list_dir_entries(&self.current_dir)?);
        Ok(())
    }

    fn set_items(&mut self, mut items: Vec<DirEntry>) {
        items.sort_by(|a, b| match (&a.kind, &b.kind) {
            (DirEntryKind::Parent, _) => std::cmp::Ordering::Less,
            (_, DirEntryKind::Parent) => std::cmp::Ordering::Greater,
            (DirEntryKind::Directory, DirEntryKind::File) => std::cmp::Ordering::Less,
            (DirEntryKind::File, DirEntryKind::Directory) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });
        self.items = items;
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
        let title = Line::from(self.current_dir.to_string_lossy().to_string().bold());
        let block = Block::bordered()
            .title_top(title.centered())
            .border_style(if focused {
                Self::FOCUSED
            } else {
                Self::UNFOCUSED
            });

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        let list = List::new(self.items.iter().map(|entry| {
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
