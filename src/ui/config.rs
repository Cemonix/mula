use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Padding, StatefulWidget, Widget},
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
    /// The heading the row is listed under. Rows of one section are next to
    /// each other.
    pub section: &'static str,
    pub title: &'static str,
    pub value: String,
    /// Every value the row can show, which the box is measured against so
    /// that stepping through them never resizes it.
    pub choices: Vec<String>,
    pub help: &'static str,
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

/// The config overlay: a cursor over settings and keys the overlay does not
/// own, and the question it is asking about them, if any.
///
/// The settings and keys live on `App`, which writes them to the config file
/// at every change.
#[derive(Debug, Default)]
pub struct ConfigView {
    /// Index of the row under the cursor.
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

    const TITLE: &'static str = "Config";

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

    /// The list as it is drawn: a heading wherever the section changes, then
    /// its rows. Answers the items and which of them the cursor is on.
    fn items(&self, rows: &[ConfigRow], title_width: usize) -> (Vec<ListItem<'static>>, usize) {
        let mut items = Vec::new();
        let mut cursor = 0;
        let mut section = None;
        for (index, row) in rows.iter().enumerate() {
            if section != Some(row.section) {
                section = Some(row.section);
                items.push(ListItem::new(Line::from(Span::styled(
                    row.section,
                    Style::new().fg(Color::Blue).bold(),
                ))));
            }
            if index == self.selected {
                cursor = items.len();
            }
            let padding = title_width + Self::GAP - Span::raw(row.title).width();
            let value = match (&self.question, index == self.selected) {
                (Question::Capturing { .. }, true) => {
                    Span::styled("press a key…", Style::new().fg(Color::Yellow).italic())
                }
                _ => Span::styled(row.value.clone(), Style::new().fg(Color::Yellow)),
            };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(row.title, Style::new().fg(Color::White)),
                Span::raw(" ".repeat(padding)),
                value,
            ])));
        }
        (items, cursor)
    }

    fn title_width(rows: &[ConfigRow]) -> usize {
        rows.iter()
            .map(|row| Span::raw(row.title).width())
            .max()
            .unwrap_or(0)
    }

    fn headings(rows: &[ConfigRow]) -> usize {
        let mut count = 0;
        let mut section = None;
        for row in rows {
            if section != Some(row.section) {
                section = Some(row.section);
                count += 1;
            }
        }
        count
    }

    /// The width of the box: as wide as what any row can show and as what
    /// the footer says on one line, never wider than `area`.
    fn width(rows: &[ConfigRow], footer: &str, area: Rect) -> u16 {
        let highlight = Span::raw(Self::HIGHLIGHT).width();
        let title_width = Self::title_width(rows);
        let widest = rows
            .iter()
            .flat_map(|row| {
                let value = row
                    .choices
                    .iter()
                    .map(|choice| Span::raw(choice.as_str()).width())
                    .max()
                    .unwrap_or(0);
                [
                    highlight + title_width + Self::GAP + value,
                    Span::raw(row.help).width(),
                    highlight + Span::raw(row.section).width(),
                ]
            })
            .chain([Span::raw(Self::TITLE).width(), Span::raw(footer).width()])
            .max()
            .unwrap_or(0);

        let inner = Self::block().inner(area);
        let chrome_x = area.width - inner.width;
        u16::try_from(widest)
            .unwrap_or(u16::MAX)
            .saturating_add(chrome_x)
            .min(area.width)
    }

    /// The box, centred, as large as what it draws and never larger than
    /// `area`, and the footer wrapped to its width. What does not fit in its
    /// height scrolls.
    fn layout(&self, rows: &[ConfigRow], area: Rect) -> (Rect, Vec<String>) {
        let text = self.footer(rows);
        let width = Self::width(rows, &text, area);
        let inner = Self::block().inner(Rect::new(0, 0, width, area.height));
        let chrome_y = area.height - inner.height;

        // Half the box at most, so the row the footer is about stays in view.
        let mut footer = ui::wrap(&text, usize::from(inner.width));
        footer.truncate(usize::from(inner.height / 2).max(1));

        // Every row and heading, a blank line, the footer, and the chrome.
        let lines = rows.len() + Self::headings(rows) + 1 + footer.len();
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
        (area, footer)
    }

    /// Draws the box over `area`, listing `rows`, with the help of the one
    /// under the cursor or the question being asked at the bottom.
    pub fn render(&mut self, rows: &[ConfigRow], area: Rect, buf: &mut Buffer) {
        let (outer, footer) = self.layout(rows, area);
        Clear.render(outer, buf);
        Self::block().render(outer, buf);

        let footer_height = u16::try_from(footer.len()).unwrap_or(u16::MAX);
        let [list_area, _, footer_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(footer_height),
        ])
        .areas(Self::block().inner(outer));

        let (items, cursor) = self.items(rows, Self::title_width(rows));
        let total = items.len();
        self.list_state.select(Some(cursor));
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

    /// `count` rows, a section of ten at a time, each named for its index.
    fn rows(count: usize) -> Vec<ConfigRow> {
        const SECTIONS: [&str; 6] = ["One", "Two", "Three", "Four", "Five", "Six"];
        (0..count)
            .map(|i| ConfigRow {
                section: SECTIONS[i / 10 % SECTIONS.len()],
                title: Box::leak(format!("Setting {i}").into_boxed_str()),
                value: String::from("on"),
                choices: vec![String::from("on"), String::from("off")],
                help: "What the setting decides",
            })
            .collect()
    }

    fn rendered(view: &mut ConfigView, rows: &[ConfigRow], area: Rect) -> Vec<String> {
        let mut buf = Buffer::empty(area);
        view.render(rows, area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The box is sized to the terminal and the list scrolls inside it, so a
    /// list longer than the box is still one every row can be reached in.
    #[test]
    fn every_row_can_be_reached_on_an_eighty_by_twenty_four_terminal() {
        let rows = rows(60);
        let mut view = ConfigView::default();

        for (index, row) in rows.iter().enumerate() {
            view.selected = index;
            let drawn = rendered(&mut view, &rows, SMALL);
            assert!(
                drawn
                    .iter()
                    .any(|line| line.contains(&format!("{}  ", row.title))),
                "{} is on no row of {drawn:#?}",
                row.title
            );
        }
    }

    /// The same, for the question about a clash: however many keys it names,
    /// the box still holds the row it is about.
    #[test]
    fn a_long_question_leaves_the_row_it_is_about_on_screen() {
        let rows = rows(60);
        let mut view = ConfigView::default();
        let clash = Clash {
            key: KeyBinding::plain(KeyCode::Char('c')),
            holder: "transfer.copy",
        };
        view.ask(Question::Clashing {
            action: "entry.delete",
            keys: vec![clash.key],
            clashes: vec![clash; 10],
            orphans: vec!["transfer.copy"; 10],
        });

        for (index, row) in rows.iter().enumerate() {
            view.selected = index;
            let drawn = rendered(&mut view, &rows, SMALL);
            assert!(
                drawn
                    .iter()
                    .any(|line| line.contains(&format!("{}  ", row.title))),
                "{} is on no row of {drawn:#?}",
                row.title
            );
        }
    }

    #[test]
    fn a_list_that_scrolls_says_which_rows_are_on_screen() {
        let rows = rows(60);
        let mut view = ConfigView::default();

        let drawn = rendered(&mut view, &rows, SMALL);

        assert!(
            drawn.iter().any(|line| line.contains(" of 66 ")),
            "{drawn:#?}"
        );
    }

    /// Every heading is drawn once, above the rows it heads.
    #[test]
    fn a_section_is_headed_once() {
        let rows = rows(3);
        let mut view = ConfigView::default();

        let drawn = rendered(&mut view, &rows, SMALL);

        let headings = drawn.iter().filter(|line| line.contains("One")).count();
        assert_eq!(headings, 1, "{drawn:#?}");
    }

    #[test]
    fn the_row_under_the_cursor_says_what_it_decides() {
        let mut rows = rows(2);
        rows[1].help = "Only the second one says this";
        let mut view = ConfigView::default();
        view.move_selection(VerticalDir::Down, rows.len());

        let drawn = rendered(&mut view, &rows, SMALL);

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
        let rows = rows(2);
        let mut view = ConfigView::default();
        view.ask(Question::Capturing {
            action: "transfer.copy",
        });

        let drawn = rendered(&mut view, &rows, SMALL).join("\n");

        assert!(drawn.contains("transfer.copy"), "{drawn}");
        assert!(drawn.contains("press a key"), "{drawn}");
        assert!(drawn.contains("terminal"), "{drawn}");
    }

    /// Nothing is taken from an action in silence: the question names the key,
    /// its holder, and a holder the move leaves with nothing.
    #[test]
    fn a_clash_names_the_key_its_holder_and_who_is_left_without_one() {
        let rows = rows(2);
        let mut view = ConfigView::default();
        let clash = Clash {
            key: KeyBinding::plain(KeyCode::Char('c')),
            holder: "transfer.copy",
        };
        view.ask(Question::Clashing {
            action: "entry.delete",
            keys: vec![clash.key],
            clashes: vec![clash],
            orphans: vec!["transfer.copy"],
        });

        let drawn = rendered(&mut view, &rows, SMALL).join(" ");

        assert!(drawn.contains("c is transfer.copy's"), "{drawn}");
        assert!(drawn.contains("no key at all"), "{drawn}");
    }

    /// Stepping through a setting's values never moves the box: it was
    /// measured against the widest of them from the start.
    #[test]
    fn the_box_holds_still_while_a_value_changes() {
        let mut rows = rows(1);
        rows[0].choices = vec![String::from("on"), String::from("a much longer value")];
        let view = ConfigView::default();

        let (before, _) = view.layout(&rows, SMALL);
        rows[0].value = String::from("a much longer value");
        let (after, _) = view.layout(&rows, SMALL);

        assert_eq!(before, after);
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
        let rows = rows(60);
        let mut view = ConfigView::default();
        rendered(&mut view, &rows, SMALL);

        view.move_page(VerticalDir::Up, rows.len());
        assert_eq!(view.selected(), 0);
        for _ in 0..10 {
            view.move_page(VerticalDir::Down, rows.len());
        }
        assert_eq!(view.selected(), rows.len() - 1);
    }
}
