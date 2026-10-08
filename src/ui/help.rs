use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Text, ToSpan},
    widgets::{Block, Clear, Padding, Widget},
};

use crate::{
    action::VerticalDir,
    keys::{Binding, GlobalMsg, KeyBinding},
    ui,
};

/// What a key does while the help overlay is open. Only closing leaves the
/// overlay; scrolling stays inside it.
#[derive(Clone, Copy, Debug)]
pub enum HelpMsg {
    Scroll(VerticalDir),
    ScrollPage(VerticalDir),
    Close,
}

/// The keys the overlay answers to while it is open. Anything else closes it,
/// so every key still gets the reader out of the way.
///
/// Every entry carries a `bar` label, unlike the tables of the other overlays.
/// This is the one overlay that lists somebody else's keys rather than its
/// own, so the bar is the only place its own are named — and the thing a
/// reader cannot see here is that the list goes on below the border.
pub const HELP_KEYS: &[Binding<HelpMsg>] = &[
    Binding {
        key: KeyBinding::plain(KeyCode::Up),
        msg: HelpMsg::Scroll(VerticalDir::Up),
        bar: Some("Scroll"),
        help: "Scrolls one line up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Down),
        msg: HelpMsg::Scroll(VerticalDir::Down),
        bar: Some("Scroll"),
        help: "Scrolls one line down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::PageUp),
        msg: HelpMsg::ScrollPage(VerticalDir::Up),
        bar: Some("Screen"),
        help: "Scrolls one screen up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::PageDown),
        msg: HelpMsg::ScrollPage(VerticalDir::Down),
        bar: Some("Screen"),
        help: "Scrolls one screen down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Esc),
        msg: HelpMsg::Close,
        bar: Some("Close"),
        help: "Closes the help",
    },
];

/// What the list is narrowed to, how far it is scrolled, and how many rows the
/// frame that drew it had room for.
///
/// The row count is written down by the render because that is where the size
/// of the terminal is known; a key pressed before the first frame scrolls by a
/// single line. Every change to the query puts the scroll back at the top.
#[derive(Debug, Default)]
pub struct HelpState {
    query: String,
    offset: usize,
    rows: usize,
}

impl HelpState {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.offset = 0;
    }

    pub fn pop(&mut self) {
        self.query.pop();
        self.offset = 0;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.offset = 0;
    }

    pub fn scroll(&mut self, dir: VerticalDir) {
        self.shift(1, dir);
    }

    pub fn scroll_page(&mut self, dir: VerticalDir) {
        self.shift(self.rows.max(1), dir);
    }

    fn shift(&mut self, by: usize, dir: VerticalDir) {
        self.offset = match dir {
            VerticalDir::Up => self.offset.saturating_sub(by),
            VerticalDir::Down => self.offset.saturating_add(by),
        };
    }

    /// Notes how many rows one screen holds and pulls the offset back to the
    /// last screenful of `total`. Returns the first entry to draw.
    fn fit(&mut self, rows: usize, total: usize) -> usize {
        self.rows = rows;
        self.offset = self.offset.min(total.saturating_sub(rows));
        self.offset
    }
}

/// Every key working right now that the query matches, one per row, as many as
/// the terminal has room for: the table of the mode underneath, and then the
/// globals, which work there as much as anywhere else.
///
/// A row matches when the query, ignoring case, is a substring of its help
/// text, of its key as [`KeyBinding`] spells it, or of the name `names` gives
/// its message. An empty query matches every row.
///
/// `GlobalMsg` is named outright rather than being a second type parameter.
/// There is one table of globals and the widget reads no message from it, only
/// the key and the text.
pub struct Help<'b, T> {
    bindings: &'b [Binding<T>],
    globals: &'b [Binding<GlobalMsg>],
    names: fn(&T) -> Option<&'static str>,
    state: &'b mut HelpState,
}

/// One row of the overlay: a key, the catalogue name of what it does where
/// there is one, and its help text.
type Row<'b> = (KeyBinding, Option<&'static str>, &'b str);

/// Whether `query`, already lowercased, is a substring of any field of `row`.
fn matches(query: &str, (key, name, help): &Row) -> bool {
    help.to_lowercase().contains(query)
        || key.to_string().to_lowercase().contains(query)
        || name.is_some_and(|name| name.to_lowercase().contains(query))
}

impl<'b, T> Help<'b, T> {
    const WIDTH: u16 = 80;
    /// Rows left free above and below, so the overlay reads as a box over the
    /// screen rather than as a screen of its own.
    const MARGIN: u16 = 1;
    /// Columns between a key and its description.
    const GAP: u16 = 2;

    pub fn new(
        bindings: &'b [Binding<T>],
        globals: &'b [Binding<GlobalMsg>],
        state: &'b mut HelpState,
    ) -> Self {
        Self {
            bindings,
            globals,
            names: |_| None,
            state,
        }
    }

    /// Gives the overlay the catalogue name of each message, so a query can
    /// find a row by it. Without it no row has a name.
    pub fn names(self, names: fn(&T) -> Option<&'static str>) -> Self {
        Self { names, ..self }
    }
}

impl<'b, T> Widget for Help<'b, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Two tables of different message types, laid down as the rows the
        // overlay draws. The box is measured against every row and filled with
        // the matching ones, so it holds still while the query is typed.
        let rows: Vec<Row> = self
            .bindings
            .iter()
            .map(|b| (b.key, (self.names)(&b.msg), b.help))
            .chain(self.globals.iter().map(|b| (b.key, None, b.help)))
            .collect();
        let query = self.state.query.to_lowercase();
        let matching: Vec<&Row> = rows.iter().filter(|row| matches(&query, row)).collect();
        let total = matching.len();

        let [area] = Layout::horizontal([Constraint::Max(Self::WIDTH)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Max(rows.len() as u16 + 2)])
            .flex(Flex::Center)
            .vertical_margin(Self::MARGIN)
            .areas(area);

        Clear.render(area, buf);

        let block = Block::bordered()
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue));
        let inner = block.inner(area);

        let first = self.state.fit(inner.height as usize, total);
        let shown = &matching[first..(first + inner.height as usize).min(total)];

        let search = match self.state.query.as_str() {
            "" => Line::from(" Type to search "),
            query => Line::from(vec![" Search: ".into(), query.bold().white(), " ".into()]),
        };
        let block = block
            .title_top(Line::from("Help").centered())
            .title_bottom(search.left_aligned());
        let block = match ui::scroll_position(first, shown.len(), total) {
            Some(position) => block.title_bottom(Line::from(position).right_aligned()),
            None => block,
        };
        block.render(area, buf);

        if shown.is_empty() {
            Line::from("No key matches".white()).render(inner, buf);
            return;
        }

        // Measured over every row rather than the ones on screen, so the column
        // holds still while the list scrolls.
        let key_width = rows
            .iter()
            .map(|(key, _, _)| key.to_span().width() as u16)
            .max()
            .unwrap_or(0);

        let columns = Layout::horizontal([
            Constraint::Length(key_width + Self::GAP),
            Constraint::Min(0),
        ])
        .split(inner);

        Text::from_iter(
            shown
                .iter()
                .map(|(key, _, _)| Line::from(key.to_span().bold().white())),
        )
        .render(columns[0], buf);

        Text::from_iter(
            shown
                .iter()
                .map(|(_, _, help)| Line::from(help.to_span().white())),
        )
        .render(columns[1], buf);
    }
}

#[cfg(test)]
mod help_tests {
    use super::*;
    use crate::action::Action;
    use crate::keys::{self, BROWSE_ACTIONS, GLOBAL_KEYS};

    /// The smallest terminal that counts, the same one the key bar is curated
    /// against.
    const COLUMNS: u16 = 80;
    const ROWS: u16 = 24;

    /// The Browse table as it stands with no config to override it.
    fn browse() -> Vec<Binding<Action>> {
        keys::table(BROWSE_ACTIONS, |_| None)
    }

    /// Draws the overlay over an 80x24 terminal, with the catalogue names
    /// given, and returns its rows as text.
    fn frame(bindings: &[Binding<Action>], state: &mut HelpState) -> Vec<String> {
        let area = Rect::new(0, 0, COLUMNS, ROWS);
        let mut buf = Buffer::empty(area);

        Help::new(bindings, GLOBAL_KEYS, state)
            .names(keys::catalogue_name)
            .render(area, &mut buf);

        (0..ROWS)
            .map(|y| (0..COLUMNS).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// Draws a page, scrolls one screen down, and goes on until the offset
    /// stops moving. Returns every frame drawn, as text.
    fn every_page(bindings: &[Binding<Action>], state: &mut HelpState) -> String {
        let mut seen = String::new();
        let mut offset = usize::MAX;

        while state.offset != offset {
            offset = state.offset;
            seen.push_str(&frame(bindings, state).join("\n"));
            state.scroll_page(VerticalDir::Down);
        }
        seen
    }

    fn searching(query: &str) -> HelpState {
        let mut state = HelpState::default();
        for c in query.chars() {
            state.push(c);
        }
        state
    }

    #[test]
    fn help_keys_bind_every_key_once() {
        keys::validate(HELP_KEYS).unwrap();
    }

    /// The overlay draws the keys of the mode underneath, never its own, so a
    /// key of its own that is missing from the bar is named nowhere at all.
    #[test]
    fn every_help_key_reaches_the_bar() {
        for binding in HELP_KEYS {
            assert!(binding.bar.is_some(), "{} is named nowhere", binding.key);
        }
    }

    #[test]
    fn help_keys_cover_every_message() {
        for msg in [
            HelpMsg::Scroll(VerticalDir::Up),
            HelpMsg::ScrollPage(VerticalDir::Up),
            HelpMsg::Close,
        ] {
            assert!(
                keys::find(HELP_KEYS, |m| std::mem::discriminant(m)
                    == std::mem::discriminant(&msg))
                .is_some(),
                "{msg:?} has no key"
            );
        }
    }

    /// The guard the fixed height could never give: every binding has to be
    /// readable on the smallest terminal that counts, however long the table
    /// grows and whatever the user later binds. The globals are in it too —
    /// the overlay claims to list every key working right now.
    #[test]
    fn every_key_is_reachable_at_eighty_by_twenty_four() {
        let browse = browse();
        let seen = every_page(&browse, &mut HelpState::default());

        let listed = browse
            .iter()
            .map(|b| (b.key, b.help))
            .chain(GLOBAL_KEYS.iter().map(|b| (b.key, b.help)));

        for (key, help) in listed {
            assert!(seen.contains(help), "{key} is never on screen");
        }
    }

    #[test]
    fn a_table_that_fits_is_not_given_a_position() {
        let mut state = HelpState::default();

        let rows = frame(&browse()[..3], &mut state).join("\n");

        assert!(!rows.contains(" of "), "the frame was {rows:?}");
    }

    #[test]
    fn a_table_that_does_not_fit_says_where_the_reader_is() {
        let mut state = HelpState::default();

        let browse = browse();
        let rows = frame(&browse, &mut state).join("\n");

        // Twenty rows is what the frame this builds has room for; the total
        // comes from the tables, so adding a key does not fail this.
        let total = browse.len() + GLOBAL_KEYS.len();
        assert!(
            rows.contains(&format!("1-20 of {total}")),
            "the frame was {rows:?}"
        );
    }

    /// Scrolling past the end leaves a full screen rather than one last row,
    /// so the offset stops climbing however long a key is held.
    #[test]
    fn scrolling_past_the_end_stops_at_the_last_screenful() {
        let browse = browse();
        let mut state = HelpState::default();
        frame(&browse, &mut state);

        for _ in 0..100 {
            state.scroll(VerticalDir::Down);
        }
        frame(&browse, &mut state);

        assert_eq!(state.offset, browse.len() + GLOBAL_KEYS.len() - state.rows);
    }

    #[test]
    fn an_empty_query_lists_every_row() {
        let browse = browse();
        let short = &browse[..5];
        let mut state = searching("x");
        state.clear_query();

        let rows = frame(short, &mut state).join("\n");

        let listed = short
            .iter()
            .map(|b| b.help)
            .chain(GLOBAL_KEYS.iter().map(|b| b.help));
        for help in listed {
            assert!(rows.contains(help), "{help} is missing from {rows:?}");
        }
    }

    /// A query matching nothing leaves no row to fit the scroll against,
    /// however far it had been scrolled before.
    #[test]
    fn a_query_matching_nothing_draws_an_empty_list() {
        let browse = browse();
        let mut state = HelpState::default();
        frame(&browse, &mut state);
        state.scroll_page(VerticalDir::Down);
        let query = "\u{2603}\u{2603}\u{2603}";
        for c in query.chars() {
            state.push(c);
        }
        state.scroll_page(VerticalDir::Down);

        let rows = frame(&browse, &mut state).join("\n");

        assert_eq!(state.offset, 0);
        assert!(rows.contains("No key matches"), "the frame was {rows:?}");
        assert!(!rows.contains(" of "), "the frame was {rows:?}");
    }

    #[test]
    fn a_change_of_query_scrolls_back_to_the_top() {
        let browse = browse();
        let mut state = HelpState::default();
        frame(&browse, &mut state);

        state.scroll_page(VerticalDir::Down);
        state.push('e');
        assert_eq!(state.offset, 0);

        state.scroll_page(VerticalDir::Down);
        state.pop();
        assert_eq!(state.offset, 0);

        state.scroll_page(VerticalDir::Down);
        state.clear_query();
        assert_eq!(state.offset, 0);
    }

    #[test]
    fn a_query_finds_a_row_by_its_key_spelled_in_lower_case() {
        let browse = browse();

        for binding in &browse {
            let query = binding.key.to_string().to_lowercase();
            let seen = every_page(&browse, &mut searching(&query));

            assert!(seen.contains(binding.help), "{query} does not find its row");
        }
    }

    #[test]
    fn a_query_finds_a_row_by_its_catalogue_name_spelled_in_upper_case() {
        let browse = browse();

        for entry in BROWSE_ACTIONS {
            let query = entry.name.to_uppercase();
            let seen = every_page(&browse, &mut searching(&query));

            assert!(seen.contains(entry.help), "{query} does not find its row");
        }
    }
}
