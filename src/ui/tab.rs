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
    fs::{directory::DirEntryKind, sizes::DirSizes},
    settings::Settings,
    ui::{
        self,
        columns::Columns,
        icon::Icons,
        pane::{Pane, PaneError},
    },
};

/// What a set of marks adds up to on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Weight {
    /// Every mark has a size, and this is their sum.
    Exact(u64),
    /// Some marks have a size and add up to this; the rest have none yet.
    AtLeast(u64),
    /// No mark has a size.
    #[default]
    Unknown,
}

/// Which neighbour of the active tab becomes active, wrapping around the ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleDirection {
    Previous,
    Next,
}

/// What a mark action does to each item it reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    /// The pane of every tab, the ones behind the active one included.
    pub fn panes_mut(&mut self) -> impl Iterator<Item = &mut Pane> {
        self.tabs.iter_mut().map(|tab| &mut tab.pane)
    }

    pub fn toggle(&mut self, dir: ToggleDirection) {
        self.active = match dir {
            ToggleDirection::Previous => self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1),
            ToggleDirection::Next => (self.active + 1) % self.tabs.len(),
        };
    }

    /// Draws the tab strip and the active tab below it. `focused` says whether
    /// the cursor is on this side and only reaches the pane border.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        layout: Rect,
        focused: bool,
        columns: Columns,
        icons: Icons,
    ) {
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
            .render(frame, tabs_area[1], focused, columns, icons);
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
    /// A tab on the current directory, its pane opened the way `settings` say.
    pub fn new(title: String, settings: &Settings) -> Result<Self, PaneError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let curr_dir: Arc<Path> = env::current_dir()?.into();
        let mut pane = Pane::empty(curr_dir);
        pane.take_settings(settings);

        Ok(Self {
            id: TabId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            title,
            pane,
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

        mark(&mut self.selected_items, op, &selected.path);
        Ok(())
    }

    /// Applies `op` to every entry the pane is showing whose name `matching`
    /// accepts, leaving the parent alone the way [`Tab::apply_mark`] does.
    /// Marks outside the view, in this directory or any other, stay as they
    /// are.
    ///
    /// What is on screen, not what the directory holds: a filter narrows the
    /// view, and marking through it would take names the user cannot see.
    pub fn mark_visible(&mut self, op: MarkOp, matching: impl Fn(&str) -> bool) {
        for entry in self.pane.visible_entries() {
            if entry.kind != DirEntryKind::Parent && matching(&ui::name_of(&entry.path)) {
                mark(&mut self.selected_items, op, &entry.path);
            }
        }
    }

    /// How many of the marks the view does not show: marks in another
    /// directory, and marks here that the filter or the dot-file setting hides.
    pub fn marked_out_of_sight(&self) -> usize {
        let in_sight = self
            .pane
            .visible_entries()
            .filter(|entry| self.selected_items.contains(&entry.path))
            .count();
        self.selected_items.len() - in_sight
    }

    /// What the marks add up to, as far as `App` knows: a file by the size
    /// its listing read, a directory by the total `sizes` holds for it.
    ///
    /// A mark outside this directory has no listing to read a size from, so
    /// only a directory measured before can count; a file there, a directory
    /// nobody has measured, and a file listed without metadata are unknown.
    pub fn marked_weight(&self, sizes: &DirSizes) -> Weight {
        let mut known = None;
        let mut unknown = 0;
        let mut add = |size: Option<u64>| match size {
            Some(size) => *known.get_or_insert(0) += size,
            None => unknown += 1,
        };

        let mut listed = HashSet::new();
        for entry in self.pane.listed_entries() {
            if let Some(mark) = self.selected_items.get(&entry.path) {
                listed.insert(mark);
                add(match entry.kind {
                    DirEntryKind::Directory => sizes.get(&entry.path),
                    _ => entry.meta.map(|meta| meta.size),
                });
            }
        }
        for mark in &self.selected_items {
            if !listed.contains(mark) {
                add(sizes.get(mark));
            }
        }

        match (known, unknown) {
            (None, _) => Weight::Unknown,
            (Some(size), 0) => Weight::Exact(size),
            (Some(size), _) => Weight::AtLeast(size),
        }
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

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        columns: Columns,
        icons: Icons,
    ) {
        self.pane
            .render(frame, area, &self.selected_items, focused, columns, icons);
    }
}

/// Applies `op` to `path` in `marks`.
fn mark(marks: &mut HashSet<Arc<Path>>, op: MarkOp, path: &Arc<Path>) {
    match op {
        MarkOp::Toggle => {
            if !marks.insert(Arc::clone(path)) {
                marks.remove(path);
            }
        }
        MarkOp::Mark => {
            marks.insert(Arc::clone(path));
        }
        MarkOp::Unmark => {
            marks.remove(path);
        }
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
                .map(|i| Tab::new(i.to_string(), &Settings::default()).unwrap())
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

    use crate::{
        fs::{
            directory::{Detail, DirEntry, Directory, EntryMeta},
            listing::Listed,
        },
        ui::filter,
    };

    /// A tab standing on a listing of `names` under `/a`, prefixed with a
    /// parent entry. Built through `listed` because that is the only way a
    /// pane takes a directory.
    fn tab_over(names: &[&str]) -> Tab {
        let rows: Vec<_> = names
            .iter()
            .map(|&name| (name, DirEntryKind::File, None))
            .collect();
        tab_of(&rows)
    }

    /// A tab like [`tab_over`]'s, each row of a kind and with a size or none.
    fn tab_of(rows: &[(&str, DirEntryKind, Option<u64>)]) -> Tab {
        let root: Arc<Path> = Arc::from(Path::new("/a"));
        let mut entries = vec![DirEntry {
            path: Arc::from(Path::new("/")),
            kind: DirEntryKind::Parent,
            meta: None,
        }];
        entries.extend(rows.iter().map(|&(name, kind, size)| DirEntry {
            path: Arc::from(root.join(name).as_path()),
            kind,
            meta: size.map(|size| EntryMeta { size, modified: 0 }),
        }));

        let mut tab = Tab::new(String::from("t"), &Settings::default()).unwrap();
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

        tab.mark_visible(MarkOp::Mark, everything);

        assert_eq!(marked_names(&tab), ["a", "b", "c"]);
    }

    /// The parent is a way out of the directory, not an item in it. Marking it
    /// would put the directory above into a batch that meant to take what is
    /// inside this one.
    #[test]
    fn marking_everything_leaves_the_parent_alone() {
        let mut tab = tab_over(&["a"]);

        tab.mark_visible(MarkOp::Mark, everything);

        assert_eq!(marked_names(&tab), ["a"]);
    }

    /// The whole reason this exists: it is what the filter narrows down to
    /// that gets marked, never what the listing behind it still holds.
    #[test]
    fn marking_everything_skips_what_the_view_is_hiding() {
        let mut tab = tab_over(&["a", ".env", "b"]);

        tab.mark_visible(MarkOp::Mark, everything);

        assert_eq!(marked_names(&tab), ["a", "b"]);
    }

    #[test]
    fn marking_everything_twice_marks_each_item_once() {
        let mut tab = tab_over(&["a", "b"]);

        tab.mark_visible(MarkOp::Mark, everything);
        tab.mark_visible(MarkOp::Mark, everything);

        assert_eq!(marked_names(&tab), ["a", "b"]);
    }

    /// Marks made elsewhere are absolute paths and none of the view's
    /// business, so marking what is on screen adds to them.
    #[test]
    fn marking_everything_keeps_the_marks_made_in_another_directory() {
        let mut tab = tab_over(&["a"]);
        tab.mark_paths([PathBuf::from("/elsewhere/x")]);

        tab.mark_visible(MarkOp::Mark, everything);

        assert_eq!(marked_names(&tab), ["a", "x"]);
    }

    fn everything(_: &str) -> bool {
        true
    }

    fn pattern(pattern: &'static str) -> impl Fn(&str) -> bool {
        move |name| filter::matches(pattern, name)
    }

    #[test]
    fn a_pattern_marks_only_the_names_it_matches() {
        let mut tab = tab_over(&["a.jpg", "b.png", "c.JPG"]);

        tab.mark_visible(MarkOp::Mark, pattern("*.jpg"));

        assert_eq!(marked_names(&tab), ["a.jpg", "c.JPG"]);
    }

    /// The parent's own name is whatever the directory above is called, so a
    /// pattern can match it; it is still the way out and not an item.
    #[test]
    fn a_pattern_never_marks_the_parent() {
        let mut tab = tab_over(&["a"]);

        tab.mark_visible(MarkOp::Mark, everything);
        tab.mark_visible(MarkOp::Toggle, everything);

        assert!(marked_names(&tab).is_empty());
    }

    #[test]
    fn a_pattern_skips_a_match_the_filter_is_hiding() {
        let mut tab = tab_over(&["a.jpg", "b.jpg"]);
        tab.get_pane_mut().push_filter('a');

        tab.mark_visible(MarkOp::Mark, pattern("*.jpg"));

        assert_eq!(marked_names(&tab), ["a.jpg"]);
    }

    /// Marks are absolute, so a directory left behind keeps its own; unmarking
    /// by a pattern is about the names on screen.
    #[test]
    fn unmarking_by_a_pattern_keeps_the_marks_made_in_another_directory() {
        let mut tab = tab_over(&["a.jpg", "b.txt"]);
        tab.mark_paths([PathBuf::from("/elsewhere/x.jpg")]);
        tab.mark_visible(MarkOp::Mark, everything);

        tab.mark_visible(MarkOp::Unmark, pattern("*.jpg"));

        assert_eq!(marked_names(&tab), ["b.txt", "x.jpg"]);
    }

    #[test]
    fn inverting_swaps_the_marked_and_the_unmarked_on_screen() {
        let mut tab = tab_over(&["a", "b", "c"]);
        tab.mark_paths([PathBuf::from("/a/b"), PathBuf::from("/elsewhere/x")]);

        tab.mark_visible(MarkOp::Toggle, everything);

        assert_eq!(marked_names(&tab), ["a", "c", "x"]);
    }

    #[test]
    fn marks_in_another_directory_are_out_of_sight() {
        let mut tab = tab_over(&["a"]);
        tab.mark_visible(MarkOp::Mark, everything);
        tab.mark_paths([PathBuf::from("/elsewhere/x")]);

        assert_eq!(tab.marked_out_of_sight(), 1);
    }

    fn marking_all(rows: &[(&str, DirEntryKind, Option<u64>)]) -> Tab {
        let mut tab = tab_of(rows);
        tab.mark_visible(MarkOp::Mark, everything);
        tab
    }

    fn measured(dirs: &[(&str, u64)]) -> DirSizes {
        let mut sizes = DirSizes::default();
        for &(dir, total) in dirs {
            sizes.insert(Arc::from(Path::new(dir)), total);
        }
        sizes
    }

    #[test]
    fn files_and_measured_directories_add_up_exactly() {
        let tab = marking_all(&[
            ("d", DirEntryKind::Directory, None),
            ("f", DirEntryKind::File, Some(10)),
        ]);

        assert_eq!(
            tab.marked_weight(&measured(&[("/a/d", 100)])),
            Weight::Exact(110)
        );
    }

    #[test]
    fn a_directory_not_measured_yet_leaves_a_lower_bound() {
        let tab = marking_all(&[
            ("d", DirEntryKind::Directory, None),
            ("f", DirEntryKind::File, Some(10)),
        ]);

        assert_eq!(tab.marked_weight(&measured(&[])), Weight::AtLeast(10));
    }

    #[test]
    fn a_mark_the_filter_hides_still_weighs_what_it_was_listed_with() {
        let mut tab = marking_all(&[
            ("a", DirEntryKind::File, Some(1)),
            ("b", DirEntryKind::File, Some(2)),
        ]);
        tab.get_pane_mut().push_filter('a');

        assert_eq!(tab.marked_weight(&measured(&[])), Weight::Exact(3));
    }

    #[test]
    fn a_mark_elsewhere_counts_only_as_a_measured_directory() {
        let mut tab = marking_all(&[("f", DirEntryKind::File, Some(10))]);
        tab.mark_paths([PathBuf::from("/elsewhere/d"), PathBuf::from("/elsewhere/x")]);

        assert_eq!(
            tab.marked_weight(&measured(&[("/elsewhere/d", 5)])),
            Weight::AtLeast(15)
        );
    }

    #[test]
    fn nothing_with_a_size_weighs_nothing_known() {
        let tab = marking_all(&[("f", DirEntryKind::File, None)]);

        assert_eq!(tab.marked_weight(&measured(&[])), Weight::Unknown);
    }

    #[test]
    fn a_mark_the_filter_hides_is_out_of_sight() {
        let mut tab = tab_over(&["a", "b"]);
        tab.mark_visible(MarkOp::Mark, everything);
        tab.get_pane_mut().push_filter('a');

        assert_eq!(tab.marked_out_of_sight(), 1);
    }
}
