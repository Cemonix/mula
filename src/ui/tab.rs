use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    symbols,
    text::Line,
    widgets::Tabs,
};

use crate::{
    fs::directory::DirEntryKind,
    ui::pane::{Pane, PaneError},
};

/// Which neighbour of the active tab becomes active, wrapping around the ends.
#[derive(Clone, Copy, Debug)]
pub enum ToggleDirection {
    Previous,
    Next,
}

/// What a mark action does to the item under the cursor.
#[derive(Clone, Copy, Debug)]
pub enum MarkOp {
    Toggle,
    Mark,
    Unmark,
}

/// Names one tab for as long as it exists. Work queued from a tab outlives the
/// cursor being there, so its marks need a way home that neither the active
/// index nor the panel can invalidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabId(u64);

#[derive(Debug)]
pub struct TabList {
    tabs: Vec<Tab>,
    active: usize,
}

impl TabList {
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self { tabs, active: 0 }
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

    /// Finds a tab by the id it was given when it was created, which is `None`
    /// once that tab is gone.
    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    pub fn toggle(&mut self, dir: ToggleDirection) {
        self.active = match dir {
            ToggleDirection::Previous => self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1),
            ToggleDirection::Next => (self.active + 1) % self.tabs.len(),
        };
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
    id: TabId,
    title: String,
    pane: Pane,
    selected_items: HashSet<Arc<Path>>,
}

impl Tab {
    pub fn new(title: String) -> Result<Self, PaneError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let curr_dir: Arc<Path> = env::current_dir()?.into();

        Ok(Self {
            id: TabId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            title,
            pane: Pane::empty(curr_dir),
            selected_items: HashSet::new(),
        })
    }

    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn get_pane(&self) -> &Pane {
        &self.pane
    }

    pub fn get_pane_mut(&mut self) -> &mut Pane {
        &mut self.pane
    }

    pub fn get_selected_items(&self) -> &HashSet<Arc<Path>> {
        &self.selected_items
    }

    pub fn get_title(&self) -> &str {
        &self.title
    }

    pub fn rename(&mut self, title: String) {
        self.title = title;
    }

    pub fn apply_mark(&mut self, op: MarkOp) -> Result<(), PaneError> {
        let selected = self.pane.selected_entry()?;
        // The parent entry is never marked.
        if selected.kind == DirEntryKind::Parent {
            return Ok(());
        }

        match op {
            MarkOp::Toggle => {
                if !self.selected_items.insert(Arc::clone(&selected.path)) {
                    self.selected_items.remove(&selected.path);
                }
            }
            MarkOp::Mark => {
                self.selected_items.insert(Arc::clone(&selected.path));
            }
            MarkOp::Unmark => {
                self.selected_items.remove(&selected.path);
            }
        }
        Ok(())
    }

    pub fn deselect_items(&mut self) {
        self.selected_items.clear();
    }

    /// Marks paths that need not be in this tab's directory, which is how the
    /// items a batch could not handle come back: marks are absolute and simply
    /// stay invisible until their own directory is on screen again.
    pub fn mark_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selected_items
            .extend(paths.into_iter().map(Arc::<Path>::from));
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
