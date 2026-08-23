use std::{path::Path, sync::Arc};

use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, StatefulWidget, Widget},
};

use crate::{
    action::VerticalDir,
    fs::{directory::DirEntry, find::Ended},
    keys::{Binding, KeyBinding},
    ui::{
        icon::Icon,
        text_input::{HorizontalDir, TextInput},
    },
};

#[derive(Clone, Copy, Debug)]
pub enum FindMsg {
    /// Moves through the hits.
    MoveSelection(VerticalDir),
    /// Moves through the typed query.
    MoveCursor(HorizontalDir),
    Confirm,
    Cancel,
}

/// The find overlay: a query being typed and the hits found for it so far.
///
/// Holds its own state the way `Prompt` and `Dialog` do. The hits are appended
/// as the walk reports them and are never reordered, so the cursor stays on
/// what it was put on while the list grows underneath.
#[derive(Debug)]
pub struct Finder {
    /// Where the walk starts. Taken when the overlay opens and never read from
    /// a panel again, so moving the panel underneath cannot move the search.
    root: Arc<Path>,
    input: TextInput,
    hits: Vec<DirEntry>,
    list_state: ListState,
    /// How the walk ended, or `None` while one is still running.
    ended: Option<Ended>,
}

impl Finder {
    pub const FIND_KEYS: &[Binding<FindMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Up),
            msg: FindMsg::MoveSelection(VerticalDir::Up),
            bar: None,
            help: "Moves the cursor one result up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Down),
            msg: FindMsg::MoveSelection(VerticalDir::Down),
            bar: None,
            help: "Moves the cursor one result down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: FindMsg::MoveCursor(HorizontalDir::Left),
            bar: None,
            help: "Moves cursor left",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: FindMsg::MoveCursor(HorizontalDir::Right),
            bar: None,
            help: "Moves cursor right",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: FindMsg::Confirm,
            bar: Some("Go to"),
            help: "Opens the folder holding the result and puts the cursor on it",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: FindMsg::Cancel,
            bar: Some("Cancel"),
            help: "Stops the search and closes the overlay",
        },
    ];

    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 20;

    const CURSOR_BG: Color = Color::Rgb(0x33, 0x3c, 0x4d);

    pub fn new(root: Arc<Path>) -> Self {
        Self {
            root,
            input: TextInput::new(),
            hits: Vec::new(),
            list_state: ListState::default(),
            ended: None,
        }
    }

    /// Where the search runs, so the caller can queue a walk without asking a
    /// panel again.
    pub fn root(&self) -> &Arc<Path> {
        &self.root
    }

    pub fn query(&self) -> &str {
        self.input.text()
    }

    pub fn insert(&mut self, c: char) {
        self.input.insert(c);
    }

    pub fn backspace(&mut self) {
        self.input.backspace();
    }

    pub fn move_cursor(&mut self, dir: HorizontalDir) {
        self.input.move_cursor(dir);
    }

    /// Throws away the hits of the previous query. Called when the query
    /// changes, which is also when the walk behind it is replaced.
    pub fn restart(&mut self) {
        self.hits.clear();
        self.list_state.select(None);
        self.ended = None;
    }

    /// Appends what the walk has just reported, putting the cursor on the
    /// first hit of a search that had none yet.
    pub fn extend(&mut self, hits: impl IntoIterator<Item = DirEntry>) {
        self.hits.extend(hits);
        if self.list_state.selected().is_none() && !self.hits.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    pub fn finish(&mut self, ended: Ended) {
        self.ended = Some(ended);
    }

    /// Moves the cursor through the hits, wrapping at either end. Does nothing
    /// while there are none.
    pub fn move_selection(&mut self, dir: VerticalDir) {
        let Some(idx) = self.list_state.selected() else {
            return;
        };
        let len = self.hits.len();
        let next = match dir {
            VerticalDir::Up => (idx + len - 1) % len,
            VerticalDir::Down => (idx + 1) % len,
        };
        self.list_state.select(Some(next));
    }

    pub fn selected(&self) -> Option<&DirEntry> {
        self.list_state
            .selected()
            .and_then(|idx| self.hits.get(idx))
    }

    /// The hit as it is listed: its path from the search root down, so the
    /// depth a breadth-first walk orders by is visible in the row itself.
    fn label(&self, hit: &DirEntry) -> String {
        hit.path
            .strip_prefix(&self.root)
            .unwrap_or(hit.path.as_ref())
            .to_string_lossy()
            .into_owned()
    }

    fn status(&self) -> String {
        if self.query().is_empty() {
            return String::from("Type to search");
        }

        let found = self.hits.len();
        match self.ended {
            None => format!("Searching, {found} found"),
            Some(Ended::HitLimit) => format!("{found} found, stopped at the limit"),
            // Exhausted is the whole tree read. Cancelled cannot reach here,
            // since replacing a search drops its messages, but it needs no
            // wording of its own either.
            Some(_) => format!("{found} found"),
        }
    }

    fn block(&self) -> Block<'_> {
        Block::bordered()
            .title_top(Line::from("Find").centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue))
    }

    fn outer_area(area: Rect) -> Rect {
        let [area] = Layout::horizontal([Constraint::Length(Self::WIDTH)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(Self::HEIGHT)])
            .flex(Flex::Center)
            .areas(area);
        area
    }

    /// Splits the inside of the box into the query, the rule under it, the
    /// hits, and the status line. Both `render` and `cursor_screen_position`
    /// lay out against it, so neither can place the query row on its own.
    fn rows(&self, area: Rect) -> [Rect; 4] {
        let inner = self.block().inner(Self::outer_area(area));
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner)
    }

    /// Where the terminal's own cursor belongs, scrolled the same way the
    /// query row scrolls the text it draws.
    pub fn cursor_screen_position(&self, area: Rect) -> (u16, u16) {
        let [query, ..] = self.rows(area);
        (query.x + self.input.cursor_column(query.width), query.y)
    }
}

impl Widget for &mut Finder {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let outer = Finder::outer_area(area);
        Clear.render(outer, buf);

        let block = self.block();
        block.render(outer, buf);

        let [query_area, rule_area, list_area, status_area] = self.rows(area);

        Line::from(Span::styled(
            self.input.visible_text(query_area.width),
            Style::new().fg(Color::White),
        ))
        .render(query_area, buf);

        Line::from(Span::raw("─".repeat(rule_area.width as usize))).render(rule_area, buf);

        let items: Vec<ListItem> = self
            .hits
            .iter()
            .map(|hit| {
                let icon = Icon::icon_for(hit);
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{} ", icon.glyph), Style::new().fg(icon.color)),
                    Span::styled(self.label(hit), Style::new().fg(Color::White)),
                ]))
            })
            .collect();

        StatefulWidget::render(
            List::new(items)
                .highlight_style(Style::new().bg(Finder::CURSOR_BG))
                .highlight_symbol(">"),
            list_area,
            buf,
            &mut self.list_state,
        );

        Line::from(Span::raw(self.status()).dim()).render(status_area, buf);
    }
}

#[cfg(test)]
mod finder_tests {
    use super::*;
    use std::path::PathBuf;

    use crate::fs::directory::DirEntryKind;
    use crate::keys::validate;

    fn finder(hits: &[&str]) -> Finder {
        let mut finder = Finder::new(Arc::from(Path::new("/root")));
        finder.insert('a');
        finder.extend(hits.iter().map(|name| DirEntry {
            path: Arc::from(PathBuf::from("/root").join(name).as_path()),
            kind: DirEntryKind::File,
        }));
        finder
    }

    fn rendered(finder: &mut Finder, area: Rect) -> Vec<String> {
        let mut buf = Buffer::empty(area);
        finder.render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn find_keys_bind_every_key_once() {
        validate(Finder::FIND_KEYS).unwrap();
    }

    #[test]
    fn the_first_hit_of_a_search_takes_the_cursor() {
        let finder = finder(&["one", "two"]);

        assert_eq!(
            finder.selected().map(|hit| hit.path.to_path_buf()),
            Some(PathBuf::from("/root/one"))
        );
    }

    #[test]
    fn later_hits_do_not_move_the_cursor() {
        let mut finder = finder(&["one", "two"]);
        finder.move_selection(VerticalDir::Down);

        finder.extend([DirEntry {
            path: Arc::from(Path::new("/root/three")),
            kind: DirEntryKind::File,
        }]);

        assert_eq!(finder.list_state.selected(), Some(1));
    }

    #[test]
    fn the_cursor_wraps_at_both_ends_of_the_hits() {
        let mut finder = finder(&["one", "two"]);

        finder.move_selection(VerticalDir::Up);
        assert_eq!(finder.list_state.selected(), Some(1));
        finder.move_selection(VerticalDir::Down);
        assert_eq!(finder.list_state.selected(), Some(0));
    }

    /// Moving through an empty list would divide by zero if it were not for the
    /// cursor being absent while there is nothing to put it on.
    #[test]
    fn moving_through_no_hits_at_all_does_nothing() {
        let mut finder = Finder::new(Arc::from(Path::new("/root")));

        finder.move_selection(VerticalDir::Down);
        finder.move_selection(VerticalDir::Up);

        assert_eq!(finder.selected().map(|hit| hit.path.to_path_buf()), None);
    }

    #[test]
    fn restarting_drops_the_hits_and_the_cursor_with_them() {
        let mut finder = finder(&["one", "two"]);
        finder.finish(Ended::Exhausted);

        finder.restart();

        assert!(finder.selected().is_none());
        assert_eq!(finder.hits.len(), 0);
        assert_eq!(finder.ended, None);
    }

    #[test]
    fn a_hit_is_listed_by_its_path_below_the_search_root() {
        let mut finder = finder(&["sub/deeper/photo.png"]);
        let area = Rect::new(0, 0, Finder::WIDTH, Finder::HEIGHT);

        let rows = rendered(&mut finder, area);

        // Row 0 is the border, 1 the query, 2 the rule, 3 the first hit.
        assert!(
            rows[3].contains("sub/deeper/photo.png"),
            "the row was {:?}",
            rows[3]
        );
        assert!(!rows[3].contains("/root"), "the row was {:?}", rows[3]);
    }

    #[test]
    fn the_status_line_says_the_walk_stopped_at_the_limit() {
        let mut finder = finder(&["one"]);
        finder.finish(Ended::HitLimit);
        let area = Rect::new(0, 0, Finder::WIDTH, Finder::HEIGHT);

        let rows = rendered(&mut finder, area);

        assert!(
            rows[Finder::HEIGHT as usize - 2].contains("limit"),
            "the row was {:?}",
            rows[Finder::HEIGHT as usize - 2]
        );
    }

    #[test]
    fn the_overlay_paints_over_what_it_covers() {
        let mut finder = finder(&["one"]);
        let area = Rect::new(0, 0, Finder::WIDTH, Finder::HEIGHT);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 0, "x".repeat(Finder::WIDTH as usize), Style::new());

        finder.render(area, &mut buf);

        assert!(!rendered(&mut finder, area)[0].contains('x'));
        assert!(!buf[(0, 0)].symbol().contains('x'));
    }

    #[test]
    fn the_typing_cursor_sits_after_the_query() {
        let mut finder = Finder::new(Arc::from(Path::new("/root")));
        finder.insert('a');
        finder.insert('b');
        let area = Rect::new(0, 0, Finder::WIDTH, Finder::HEIGHT);

        let [query, ..] = finder.rows(area);
        assert_eq!(
            finder.cursor_screen_position(area),
            (query.x + 2, query.y),
            "the cursor belongs two columns into the query row"
        );
    }
}
