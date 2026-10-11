use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    symbols,
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, StatefulWidget, Tabs, Widget},
};

use crate::{
    action::VerticalDir,
    config::Clash,
    keys::{self, Binding, KeyBinding},
    settings::Step,
    ui,
};

#[derive(Clone, Copy, Debug)]
pub enum ConfigMsg {
    MoveSelection(VerticalDir),
    /// Moves the cursor by the rows the list had room for, stopping at either
    /// end.
    MovePage(VerticalDir),
    /// Shows the tab beside the one on screen, wrapping at either end.
    SwitchTab(Step),
    /// Moves the setting under the cursor to the choice beside it. An action
    /// has no choices and is left alone.
    Step(Step),
    /// Moves a setting to its next choice, or waits for the key to give an
    /// action.
    Change,
    /// Puts the row under the cursor back to its default.
    Reset,
    /// Leaves a question the overlay is asking — the key being waited for, or
    /// a clash — with nothing changed.
    Back,
    /// Gives the action the keys it was waiting for, taking them from the
    /// actions holding them.
    Take,
    Close,
}

/// One row as the overlay draws it: a setting, or an action and its keys.
#[derive(Debug)]
pub struct ConfigRow {
    pub title: &'static str,
    pub value: String,
    /// Every value the row can show, which the box is measured against so
    /// that stepping through them never resizes it.
    pub choices: Vec<String>,
    pub help: &'static str,
}

/// One tab of the overlay and the rows it lists.
#[derive(Debug)]
pub struct ConfigTab {
    pub title: &'static str,
    pub rows: Vec<ConfigRow>,
}

/// What the overlay is doing besides showing the rows.
#[derive(Debug, Default)]
pub enum Question {
    /// The cursor is the user's.
    #[default]
    None,
    /// The next key pressed goes to `action`.
    Capturing { action: &'static str },
    /// The keys `action` was to be given are held by other actions, and the
    /// user is asked whether to take them.
    Clashing {
        action: &'static str,
        keys: Vec<KeyBinding>,
        clashes: Vec<Clash>,
        /// The holders the move would leave with no key at all.
        orphans: Vec<&'static str>,
    },
}

/// The config overlay: a tab and a cursor over settings and keys the overlay
/// does not own, and the question it is asking about them, if any.
///
/// The settings and keys live on `App`, which writes them to the config file
/// at every change.
#[derive(Debug, Default)]
pub struct ConfigView {
    /// Index of the tab on screen.
    tab: usize,
    /// Index of the row under the cursor, within the tab on screen.
    selected: usize,
    /// Where the list is scrolled to, kept from one frame to the next.
    list_state: ListState,
    /// Rows of the list in the last frame, and so how far a page moves.
    page: usize,
    question: Question,
}

impl ConfigView {
    pub const CONFIG_KEYS: &[Binding<ConfigMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Up),
            msg: ConfigMsg::MoveSelection(VerticalDir::Up),
            bar: None,
            help: "Moves the cursor one row up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Down),
            msg: ConfigMsg::MoveSelection(VerticalDir::Down),
            bar: None,
            help: "Moves the cursor one row down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::PageUp),
            msg: ConfigMsg::MovePage(VerticalDir::Up),
            bar: None,
            help: "Moves the cursor one screen up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::PageDown),
            msg: ConfigMsg::MovePage(VerticalDir::Down),
            bar: None,
            help: "Moves the cursor one screen down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Tab),
            msg: ConfigMsg::SwitchTab(Step::Next),
            bar: Some("Next tab"),
            help: "Shows the next tab, wrapping to the first",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::BackTab).shift(),
            msg: ConfigMsg::SwitchTab(Step::Previous),
            bar: None,
            help: "Shows the previous tab, wrapping to the last",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: ConfigMsg::Change,
            bar: Some("Change"),
            help: "Changes the setting to its next value, or binds a key to the action",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: ConfigMsg::Step(Step::Next),
            bar: None,
            help: "Changes the setting to its next value",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: ConfigMsg::Step(Step::Previous),
            bar: None,
            help: "Changes the setting to its previous value",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Char('r')),
            msg: ConfigMsg::Reset,
            bar: Some("Default"),
            help: "Puts the setting or the action's keys back to the default",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: ConfigMsg::Close,
            bar: Some("Close"),
            help: "Closes the overlay; every change is already saved",
        },
    ];

    /// Every other key pressed while capturing is the one being bound.
    pub const CAPTURE_KEYS: &[Binding<ConfigMsg>] = &[Binding {
        key: KeyBinding::plain(KeyCode::Esc),
        msg: ConfigMsg::Back,
        bar: Some("Cancel"),
        help: "Stops waiting for a key and leaves the action as it was",
    }];

    pub const CLASH_KEYS: &[Binding<ConfigMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: ConfigMsg::Take,
            bar: Some("Take it"),
            help: "Takes the key from the action holding it and binds it here",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: ConfigMsg::Back,
            bar: Some("Cancel"),
            help: "Leaves both actions as they were",
        },
    ];

    const CURSOR_BG: Color = Color::Rgb(0x33, 0x3c, 0x4d);

    /// The cursor's own column to the left of every row, held open whether the
    /// row is the one or not.
    const HIGHLIGHT: &'static str = "> ";

    /// Blank columns between a row's title and its value.
    const GAP: usize = 2;

    /// Lines kept for the bottom of the box whatever it says, so a question
    /// replacing a help line does not resize the box. A longer one takes
    /// lines from the list instead.
    const FOOTER: usize = 3;

    const TITLE: &'static str = "Config";

    pub fn tab(&self) -> usize {
        self.tab
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn question(&self) -> &Question {
        &self.question
    }

    pub fn ask(&mut self, question: Question) {
        self.question = question;
    }

    /// The question being asked, leaving none asked.
    pub fn take_question(&mut self) -> Question {
        std::mem::take(&mut self.question)
    }

    /// The table the keys are resolved against while the overlay asks what it
    /// is asking.
    pub fn keys(&self) -> &'static [Binding<ConfigMsg>] {
        match self.question {
            Question::None => Self::CONFIG_KEYS,
            Question::Capturing { .. } => Self::CAPTURE_KEYS,
            Question::Clashing { .. } => Self::CLASH_KEYS,
        }
    }

    /// Moves the cursor through `count` rows, wrapping at either end.
    pub fn move_selection(&mut self, dir: VerticalDir, count: usize) {
        if count == 0 {
            return;
        }
        self.selected = match dir {
            VerticalDir::Up => (self.selected + count - 1) % count,
            VerticalDir::Down => (self.selected + 1) % count,
        };
    }

    /// Moves the cursor a page through `count` rows, stopping at either end.
    /// Before the first frame a page is one row.
    pub fn move_page(&mut self, dir: VerticalDir, count: usize) {
        let page = self.page.max(1);
        self.selected = match dir {
            VerticalDir::Up => self.selected.saturating_sub(page),
            VerticalDir::Down => (self.selected + page).min(count.saturating_sub(1)),
        };
    }

    /// Shows the tab beside this one among `count`, with the cursor on its
    /// first row.
    pub fn switch_tab(&mut self, step: Step, count: usize) {
        if count == 0 {
            return;
        }
        self.tab = match step {
            Step::Previous => (self.tab + count - 1) % count,
            Step::Next => (self.tab + 1) % count,
        };
        self.selected = 0;
        self.list_state = ListState::default();
    }

    fn block() -> Block<'static> {
        Block::bordered()
            .title_top(Line::from(Self::TITLE).centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue))
    }

    /// The key `table` answers `msg` with, by name.
    fn key_for(table: &[Binding<ConfigMsg>], msg: ConfigMsg) -> String {
        keys::find(table, |m| {
            std::mem::discriminant(m) == std::mem::discriminant(&msg)
        })
        .map(|binding| binding.key.to_string())
        .unwrap_or_default()
    }

    /// What the bottom of the box says: the help of the row under the cursor,
    /// or the question being asked.
    fn footer(&self, rows: &[ConfigRow]) -> String {
        match &self.question {
            Question::None => rows
                .get(self.selected)
                .map(|row| row.help.to_string())
                .unwrap_or_default(),
            Question::Capturing { action } => format!(
                "Press the key for {action}. {} cancels. A key the terminal keeps for itself never reaches Mula.",
                Self::key_for(Self::CAPTURE_KEYS, ConfigMsg::Back)
            ),
            Question::Clashing {
                action,
                clashes,
                orphans,
                ..
            } => {
                let held: Vec<String> = clashes
                    .iter()
                    .map(|clash| format!("{} is {}'s", clash.key, clash.holder))
                    .collect();
                let them = match clashes.len() {
                    1 => "it",
                    _ => "them",
                };
                let mut text = format!(
                    "{}. {} takes {them} for {action}, {} leaves every action as it was.",
                    held.join(", "),
                    Self::key_for(Self::CLASH_KEYS, ConfigMsg::Take),
                    Self::key_for(Self::CLASH_KEYS, ConfigMsg::Back),
                );
                if !orphans.is_empty() {
                    text.push_str(&format!(
                        " That leaves {} with no key at all.",
                        orphans.join(", ")
                    ));
                }
                text
            }
        }
    }

    fn title_width(rows: &[ConfigRow]) -> usize {
        rows.iter()
            .map(|row| Span::raw(row.title).width())
            .max()
            .unwrap_or(0)
    }

    /// The rows of the tab on screen, as they are drawn.
    fn items(&self, rows: &[ConfigRow]) -> Vec<ListItem<'static>> {
        let title_width = Self::title_width(rows);
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                let padding = title_width + Self::GAP - Span::raw(row.title).width();
                let value = match (&self.question, index == self.selected) {
                    (Question::Capturing { .. }, true) => {
                        Span::styled("press a key…", Style::new().fg(Color::Yellow).italic())
                    }
                    _ => Span::styled(row.value.clone(), Style::new().fg(Color::Yellow)),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(row.title, Style::new().fg(Color::White)),
                    Span::raw(" ".repeat(padding)),
                    value,
                ]))
            })
            .collect()
    }

    fn tab_bar(&self, tabs: &[ConfigTab]) -> Tabs<'static> {
        Tabs::new(tabs.iter().map(|tab| tab.title))
            .select(self.tab)
            .style(Style::new().fg(Color::White))
            .highlight_style(Style::new().fg(Color::Black).bg(Color::Blue))
            .divider(symbols::line::VERTICAL)
    }

    /// The box, centred, measured against every tab at once so that neither
    /// switching tabs nor a question changes its size. Never larger than
    /// `area`; what does not fit in its height scrolls.
    fn outer_area(tabs: &[ConfigTab], area: Rect) -> Rect {
        let highlight = Span::raw(Self::HIGHLIGHT).width();
        let rows = || tabs.iter().flat_map(|tab| &tab.rows);
        let widest_row = tabs.iter().flat_map(|tab| {
            let title_width = Self::title_width(&tab.rows);
            tab.rows.iter().map(move |row| {
                let value = row
                    .choices
                    .iter()
                    .map(|choice| Span::raw(choice.as_str()).width())
                    .max()
                    .unwrap_or(0);
                highlight + title_width + Self::GAP + value
            })
        });
        // What `Tabs` draws: each title padded by a space either side, and a
        // divider between two titles.
        let tab_bar = tabs
            .iter()
            .map(|tab| Span::raw(tab.title).width() + 2)
            .sum::<usize>()
            + tabs.len().saturating_sub(1);
        let widest = widest_row
            .chain(rows().map(|row| Span::raw(row.help).width()))
            .chain([Span::raw(Self::TITLE).width(), tab_bar])
            .max()
            .unwrap_or(0);

        let inner = Self::block().inner(area);
        let chrome_x = area.width - inner.width;
        let chrome_y = area.height - inner.height;

        // The tab bar and a blank line, the longest tab, a blank line and the
        // footer.
        let longest = tabs.iter().map(|tab| tab.rows.len()).max().unwrap_or(0);
        let lines = 2 + longest + 1 + Self::FOOTER;
        let width = u16::try_from(widest)
            .unwrap_or(u16::MAX)
            .saturating_add(chrome_x)
            .min(area.width);
        let height = u16::try_from(lines)
            .unwrap_or(u16::MAX)
            .saturating_add(chrome_y)
            .min(area.height);

        let [area] = Layout::horizontal([Constraint::Length(width)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(height)])
            .flex(Flex::Center)
            .areas(area);
        area
    }

    /// Draws the box over `area`: the tabs, the rows of the one on screen, and
    /// the help of the row under the cursor or the question being asked.
    pub fn render(&mut self, tabs: &[ConfigTab], area: Rect, buf: &mut Buffer) {
        let outer = Self::outer_area(tabs, area);
        Clear.render(outer, buf);
        Self::block().render(outer, buf);
        let inner = Self::block().inner(outer);

        let rows: &[ConfigRow] = tabs
            .get(self.tab)
            .map(|tab| tab.rows.as_slice())
            .unwrap_or_default();

        // As many lines as the footer needs, never fewer than it keeps and
        // never more than half the box, so the row it is about stays in view.
        let mut footer = ui::wrap(&self.footer(rows), usize::from(inner.width));
        let most = usize::from(inner.height / 2).max(1);
        footer.truncate(most);
        let footer_height = u16::try_from(footer.len().max(Self::FOOTER).min(most)).unwrap_or(1);

        let [tab_area, _, list_area, _, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(footer_height),
        ])
        .areas(inner);

        self.tab_bar(tabs).render(tab_area, buf);

        let items = self.items(rows);
        let total = items.len();
        self.list_state.select(Some(self.selected));
        StatefulWidget::render(
            List::new(items)
                .highlight_style(Style::new().bg(Self::CURSOR_BG))
                .highlight_symbol(Self::HIGHLIGHT),
            list_area,
            buf,
            &mut self.list_state,
        );
        self.page = usize::from(list_area.height);

        let first = self.list_state.offset();
        let shown = self.page.min(total.saturating_sub(first));
        if let Some(position) = ui::scroll_position(first, shown, total) {
            let bottom = Rect::new(
                outer.x + 1,
                outer.bottom().saturating_sub(1),
                outer.width.saturating_sub(2),
                1,
            );
            Line::from(position).right_aligned().render(bottom, buf);
        }

        let style = match self.question {
            Question::None => Style::new().dim(),
            _ => Style::new().fg(Color::White),
        };
        for (line, y) in footer.into_iter().zip(footer_area.top()..) {
            let row = Rect::new(footer_area.x, y, footer_area.width, 1);
            Line::from(Span::styled(line, style)).render(row, buf);
        }
    }
}

#[cfg(test)]
mod config_view_tests {
    use super::*;

    /// The smallest terminal that counts.
    const SMALL: Rect = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };

    /// `count` rows, each named for its index.
    fn rows(count: usize) -> Vec<ConfigRow> {
        (0..count)
            .map(|i| ConfigRow {
                title: Box::leak(format!("Setting {i}").into_boxed_str()),
                value: String::from("on"),
                choices: vec![String::from("on"), String::from("off")],
                help: "What the setting decides",
            })
            .collect()
    }

    /// A short tab of settings and a long one of keys, the way the overlay
    /// is used.
    fn tabs() -> Vec<ConfigTab> {
        vec![
            ConfigTab {
                title: "Listing",
                rows: rows(3),
            },
            ConfigTab {
                title: "Keys",
                rows: rows(60),
            },
        ]
    }

    fn rendered(view: &mut ConfigView, tabs: &[ConfigTab], area: Rect) -> Vec<String> {
        let mut buf = Buffer::empty(area);
        view.render(tabs, area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn a_clash() -> Question {
        let clash = Clash {
            key: KeyBinding::plain(KeyCode::Char('c')),
            holder: "transfer.copy",
        };
        Question::Clashing {
            action: "entry.delete",
            keys: vec![clash.key],
            clashes: vec![clash],
            orphans: vec!["transfer.copy"],
        }
    }

    /// The box is sized to the terminal and the list scrolls inside it, so a
    /// tab longer than the box is still one every row can be reached in —
    /// with a question at the bottom too, however long it is.
    #[test]
    fn every_row_of_every_tab_can_be_reached_on_an_eighty_by_twenty_four_terminal() {
        let tabs = tabs();
        let long_clash = {
            let clash = Clash {
                key: KeyBinding::plain(KeyCode::Char('c')),
                holder: "transfer.copy",
            };
            Question::Clashing {
                action: "entry.delete",
                keys: vec![clash.key],
                clashes: vec![clash; 10],
                orphans: vec!["transfer.copy"; 10],
            }
        };

        for question in [Question::None, long_clash] {
            let mut view = ConfigView::default();
            view.ask(question);
            for (index, tab) in tabs.iter().enumerate() {
                view.tab = index;
                for (selected, row) in tab.rows.iter().enumerate() {
                    view.selected = selected;
                    let drawn = rendered(&mut view, &tabs, SMALL);
                    assert!(
                        drawn
                            .iter()
                            .any(|line| line.contains(&format!("{}  ", row.title))),
                        "{} of {} is on no row of {drawn:#?}",
                        row.title,
                        tab.title
                    );
                }
            }
        }
    }

    /// The point of measuring every tab at once and keeping room for the
    /// footer: what the user does inside the box never moves its edges.
    #[test]
    fn the_box_holds_still_across_tabs_and_questions() {
        let tabs = tabs();
        let mut view = ConfigView::default();
        let edges = |drawn: &[String]| -> Vec<(usize, usize)> {
            drawn
                .iter()
                .enumerate()
                .filter_map(|(y, line)| line.find('┌').map(|x| (y, x)))
                .collect()
        };

        let first = edges(&rendered(&mut view, &tabs, SMALL));
        view.switch_tab(Step::Next, tabs.len());
        let second = edges(&rendered(&mut view, &tabs, SMALL));
        view.ask(a_clash());
        let asking = edges(&rendered(&mut view, &tabs, SMALL));

        assert_eq!(first, second);
        assert_eq!(first, asking);
    }

    #[test]
    fn a_short_tab_on_a_tall_terminal_still_holds_the_longest_one() {
        let tabs = tabs();
        let area = Rect::new(0, 0, 100, 100);

        let outer = ConfigView::outer_area(&tabs, area);

        assert!(usize::from(outer.height) > 60, "{outer:?}");
    }

    #[test]
    fn every_tab_is_named_in_the_bar() {
        let tabs = tabs();
        let mut view = ConfigView::default();

        let drawn = rendered(&mut view, &tabs, SMALL).join("\n");

        assert!(drawn.contains("Listing"), "{drawn}");
        assert!(drawn.contains("Keys"), "{drawn}");
    }

    #[test]
    fn switching_tabs_wraps_and_starts_at_the_top() {
        let mut view = ConfigView::default();
        view.move_selection(VerticalDir::Down, 3);

        view.switch_tab(Step::Previous, 2);

        assert_eq!(view.tab(), 1);
        assert_eq!(view.selected(), 0);
        view.switch_tab(Step::Next, 2);
        assert_eq!(view.tab(), 0);
    }

    #[test]
    fn a_list_that_scrolls_says_which_rows_are_on_screen() {
        let tabs = tabs();
        let mut view = ConfigView::default();
        view.switch_tab(Step::Next, tabs.len());

        let drawn = rendered(&mut view, &tabs, SMALL);

        assert!(
            drawn.iter().any(|line| line.contains(" of 60 ")),
            "{drawn:#?}"
        );
    }

    #[test]
    fn the_row_under_the_cursor_says_what_it_decides() {
        let mut tabs = tabs();
        tabs[0].rows[1].help = "Only the second one says this";
        let mut view = ConfigView::default();
        view.move_selection(VerticalDir::Down, tabs[0].rows.len());

        let drawn = rendered(&mut view, &tabs, SMALL);

        assert!(
            drawn
                .iter()
                .any(|line| line.contains("Only the second one says this")),
            "{drawn:#?}"
        );
    }

    /// While a key is awaited, the box says so and what will not arrive.
    #[test]
    fn waiting_for_a_key_says_what_it_is_waiting_for() {
        let tabs = tabs();
        let mut view = ConfigView::default();
        view.ask(Question::Capturing {
            action: "transfer.copy",
        });

        let drawn = rendered(&mut view, &tabs, SMALL).join("\n");

        assert!(drawn.contains("transfer.copy"), "{drawn}");
        assert!(drawn.contains("press a key"), "{drawn}");
        assert!(drawn.contains("terminal"), "{drawn}");
    }

    /// Nothing is taken from an action in silence: the question names the key,
    /// its holder, and a holder the move leaves with nothing.
    #[test]
    fn a_clash_names_the_key_its_holder_and_who_is_left_without_one() {
        let tabs = tabs();
        let mut view = ConfigView::default();
        view.ask(a_clash());

        // The text inside the box, its wrapped lines joined back up.
        let drawn = rendered(&mut view, &tabs, SMALL)
            .iter()
            .map(|line| line.trim().trim_matches('│').trim().to_string())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(drawn.contains("c is transfer.copy's"), "{drawn}");
        assert!(drawn.contains("no key at all"), "{drawn}");
    }

    #[test]
    fn every_table_binds_every_key_once() {
        for table in [
            ConfigView::CONFIG_KEYS,
            ConfigView::CAPTURE_KEYS,
            ConfigView::CLASH_KEYS,
        ] {
            crate::keys::validate(table).unwrap();
        }
    }

    #[test]
    fn the_cursor_wraps_at_either_end() {
        let mut view = ConfigView::default();

        view.move_selection(VerticalDir::Up, 3);
        assert_eq!(view.selected(), 2);
        view.move_selection(VerticalDir::Down, 3);
        assert_eq!(view.selected(), 0);
    }

    #[test]
    fn a_page_stops_at_either_end() {
        let tabs = tabs();
        let count = tabs[1].rows.len();
        let mut view = ConfigView::default();
        view.switch_tab(Step::Next, tabs.len());
        rendered(&mut view, &tabs, SMALL);

        view.move_page(VerticalDir::Up, count);
        assert_eq!(view.selected(), 0);
        for _ in 0..10 {
            view.move_page(VerticalDir::Down, count);
        }
        assert_eq!(view.selected(), count - 1);
    }
}
