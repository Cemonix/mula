use std::{collections::HashSet, env, path::Path, rc::Rc};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols,
    text::Line,
    widgets::Tabs,
};

use crate::{
    fs::directory::{DirEntryKind, Directory},
    ui::pane::{Pane, PaneError},
};

#[derive(Debug)]
pub struct TabList {
    tabs: Vec<Tab>,
    active: usize,
}

impl TabList {
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self { tabs, active: 0 }
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn add_tab(&mut self, tab: Tab) {
        self.tabs.push(tab);
    }

    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn toggle_prev(&mut self) {
        self.active = self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1);
    }

    pub fn toggle_next(&mut self) {
        self.active = (self.active + 1) % self.tabs.len();
    }

    /// Draws the tab strip and the active tab below it. `focused` says whether
    /// the cursor is on this side and only reaches the pane border.
    pub fn render(&mut self, frame: &mut Frame, layout: Rect, focused: bool) {
        let tabs_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(layout);

        let tabs = Tabs::new(self.tabs.iter())
            .style(Color::White)
            .highlight_style(Style::default().blue().on_black().bold())
            .select(self.active)
            .divider(symbols::DOT)
            .padding(" ", " ");

        frame.render_widget(tabs, tabs_area[0]);

        self.active_tab_mut().render(frame, tabs_area[1], focused);
    }
}

#[derive(Debug)]
pub struct Tab {
    pub title: String,
    pane: Pane,
    selected_items: HashSet<Rc<Path>>,
}

impl Tab {
    pub fn new(title: String) -> Result<Self, PaneError> {
        let curr_dir: Rc<Path> = env::current_dir()?.into();

        Ok(Self {
            title,
            pane: Pane::new(Directory::read(curr_dir)?),
            selected_items: HashSet::new(),
        })
    }

    pub fn get_pane(&self) -> &Pane {
        &self.pane
    }

    pub fn get_pane_mut(&mut self) -> &mut Pane {
        &mut self.pane
    }

    pub fn get_selected_items(&self) -> &HashSet<Rc<Path>> {
        &self.selected_items
    }

    pub fn toggle_mark(&mut self) -> Result<(), PaneError> {
        let selected = self.pane.selected_entry()?;
        // The parent entry is never marked.
        if selected.kind == DirEntryKind::Parent {
            return Ok(());
        }

        let path = Rc::clone(&selected.path);
        if self.selected_items.contains(&path) {
            self.selected_items.remove(&path);
        } else {
            self.selected_items.insert(path);
        }
        Ok(())
    }

    /// Marks the item under the cursor, leaving an already marked one marked.
    /// The parent entry is never marked.
    pub fn mark(&mut self) -> Result<(), PaneError> {
        let selected = self.pane.selected_entry()?;
        if selected.kind == DirEntryKind::Parent {
            return Ok(());
        }

        self.selected_items.insert(Rc::clone(&selected.path));
        Ok(())
    }

    pub fn deselect_items(&mut self) {
        self.selected_items.clear();
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        self.pane.render(frame, area, &self.selected_items, focused);
    }
}

impl<'a> From<&'a Tab> for Line<'a> {
    fn from(tab: &'a Tab) -> Line<'a> {
        Line::from(tab.title.as_str())
    }
}
