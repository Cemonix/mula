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
    ui::{
        columns::Columns,
        pane::{Pane, PaneError},
    },
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

    /// Closes the active tab, leaving the cursor on the one that followed it or
    /// on the new last tab, and answers with the id it closed under.
    ///
    /// The only tab of a panel is never closed, which is what keeps
    /// [`Self::active_tab`] and [`Self::toggle`] indexing a list that has
    /// something in it. Nothing is said about it: closing the last tab is a
    /// key that does nothing, not a failure.
    ///
    /// Whatever the tab was marking goes with it, and so does any listing on
    /// its way — a job queued from it still reports, and finds its tab gone.
    pub fn close_active(&mut self) -> Option<TabId> {
        if self.tabs.len() == 1 {
            return None;
        }

        let closed = self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len() - 1);
        Some(closed.id)
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
    pub fn render(&mut self, frame: &mut Frame, layout: Rect, focused: bool, columns: Columns) {
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

        self.active_tab_mut()
            .render(frame, tabs_area[1], focused, columns);
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

    /// Marks every entry the pane is showing, leaving the parent alone the way
    /// [`Tab::apply_mark`] does.
    ///
    /// What is on screen, not what the directory holds: a filter narrows the
    /// view, and marking through it would take names the user cannot see.
    pub fn mark_visible(&mut self) {
        let visible: Vec<Arc<Path>> = self
            .pane
            .visible_entries()
            .filter(|entry| entry.kind != DirEntryKind::Parent)
            .map(|entry| Arc::clone(&entry.path))
            .collect();
        self.selected_items.extend(visible);
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

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, columns: Columns) {
        self.pane
            .render(frame, area, &self.selected_items, focused, columns);
    }
}

impl<'a> From<&'a Tab> for Line<'a> {
    fn from(tab: &'a Tab) -> Line<'a> {
        Line::from(tab.title.as_str())
    }
}

#[cfg(test)]
mod tab_list_tests {
    use super::*;

    /// A list of `count` tabs titled `0`, `1`, … in that order, with the first
    /// one active.
    fn tabs(count: usize) -> TabList {
        TabList::new(
            (0..count)
                .map(|i| Tab::new(i.to_string()).unwrap())
                .collect(),
        )
    }

    #[test]
    fn closing_a_tab_leaves_the_cursor_on_the_one_that_followed_it() {
        let mut tabs = tabs(3);
        tabs.toggle(ToggleDirection::Next);

        assert!(tabs.close_active().is_some());
        assert_eq!(tabs.active_tab().get_title(), "2");
    }

    #[test]
    fn closing_the_last_tab_steps_the_cursor_back() {
        let mut tabs = tabs(2);
        tabs.toggle(ToggleDirection::Next);

        tabs.close_active();

        assert_eq!(tabs.active_tab().get_title(), "0");
    }

    #[test]
    fn the_only_tab_is_never_closed() {
        let mut tabs = tabs(1);

        // Both `active_tab` and `toggle` index the list, so an empty one is
        // not a state to draw but a panic waiting to happen.
        assert!(tabs.close_active().is_none());
        assert_eq!(tabs.active_tab().get_title(), "0");
    }

    #[test]
    fn a_closed_tab_can_no_longer_be_found_by_its_id() {
        let mut tabs = tabs(2);
        let closed = tabs.close_active().unwrap();

        // Work queued from a tab outlives the tab, and this is how it finds
        // out: marks that fail have nowhere to go home to.
        assert!(tabs.tab_mut(closed).is_none());
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;

    use crate::fs::{
        directory::{Detail, DirEntry, Directory},
        listing::Listed,
    };

    /// A tab standing on a listing of `names` under `/a`, prefixed with a
    /// parent entry. Built through `listed` because that is the only way a
    /// pane takes a directory.
    fn tab_over(names: &[&str]) -> Tab {
        let root: Arc<Path> = Arc::from(Path::new("/a"));
        let mut entries = vec![DirEntry {
            path: Arc::from(Path::new("/")),
            kind: DirEntryKind::Parent,
            meta: None,
        }];
        entries.extend(names.iter().map(|name| DirEntry {
            path: Arc::from(root.join(name).as_path()),
            kind: DirEntryKind::File,
            meta: None,
        }));

        let mut tab = Tab::new(String::from("t")).unwrap();
        tab.get_pane_mut()
            .listed(Ok(Listed::Directory(Directory::new(
                root,
                entries,
                Detail::NamesOnly,
            ))))
            .unwrap();
        tab
    }

    fn marked_names(tab: &Tab) -> Vec<String> {
        let mut names: Vec<String> = tab
            .get_selected_items()
            .iter()
            .map(|path| crate::ui::name_of(path))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn marking_everything_takes_every_row_on_screen() {
        let mut tab = tab_over(&["a", "b", "c"]);

        tab.mark_visible();

        assert_eq!(marked_names(&tab), ["a", "b", "c"]);
    }

    /// The parent is a way out of the directory, not an item in it. Marking it
    /// would put the directory above into a batch that meant to take what is
    /// inside this one.
    #[test]
    fn marking_everything_leaves_the_parent_alone() {
        let mut tab = tab_over(&["a"]);

        tab.mark_visible();

        assert_eq!(marked_names(&tab), ["a"]);
    }

    /// The whole reason this exists: it is what the filter narrows down to
    /// that gets marked, never what the listing behind it still holds.
    #[test]
    fn marking_everything_skips_what_the_view_is_hiding() {
        let mut tab = tab_over(&["a", ".env", "b"]);

        tab.mark_visible();

        assert_eq!(marked_names(&tab), ["a", "b"]);
    }

    #[test]
    fn marking_everything_twice_marks_each_item_once() {
        let mut tab = tab_over(&["a", "b"]);

        tab.mark_visible();
        tab.mark_visible();

        assert_eq!(marked_names(&tab), ["a", "b"]);
    }

    /// Marks made elsewhere are absolute paths and none of the view's
    /// business, so marking what is on screen adds to them.
    #[test]
    fn marking_everything_keeps_the_marks_made_in_another_directory() {
        let mut tab = tab_over(&["a"]);
        tab.mark_paths([PathBuf::from("/elsewhere/x")]);

        tab.mark_visible();

        assert_eq!(marked_names(&tab), ["a", "x"]);
    }
}
