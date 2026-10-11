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
    keys::{Binding, KeyBinding},
    settings::Step,
    ui,
};

#[derive(Clone, Copy, Debug)]
pub enum ConfigMsg {
    MoveSelection(VerticalDir),
    /// Moves the setting under the cursor to the choice beside it.
    Step(Step),
    Close,
}

/// One setting as the overlay draws it.
#[derive(Debug)]
pub struct ConfigRow {
    /// The heading the row is listed under. Rows of one section are next to
    /// each other.
    pub section: &'static str,
    pub title: &'static str,
    pub value: &'static str,
    /// Every value the setting can take, which the box is measured against so
    /// that stepping through them never resizes it.
    pub choices: Vec<&'static str>,
    pub help: &'static str,
}

/// The config overlay: a cursor over settings the overlay does not own.
///
/// The settings live on `App`, which writes them to the config file at every
/// change. What is here is where the cursor stands while the overlay is up.
#[derive(Debug, Default)]
pub struct ConfigView {
    /// Index of the row under the cursor.
    selected: usize,
    /// Where the list is scrolled to, kept from one frame to the next.
    list_state: ListState,
}

impl ConfigView {
    pub const CONFIG_KEYS: &[Binding<ConfigMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Up),
            msg: ConfigMsg::MoveSelection(VerticalDir::Up),
            bar: None,
            help: "Moves the cursor one setting up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Down),
            msg: ConfigMsg::MoveSelection(VerticalDir::Down),
            bar: None,
            help: "Moves the cursor one setting down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: ConfigMsg::Step(Step::Next),
            bar: Some("Change"),
            help: "Changes the setting to its next value and saves it",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: ConfigMsg::Step(Step::Next),
            bar: None,
            help: "Changes the setting to its next value and saves it",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: ConfigMsg::Step(Step::Previous),
            bar: None,
            help: "Changes the setting to its previous value and saves it",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: ConfigMsg::Close,
            bar: Some("Close"),
            help: "Closes the overlay; every change is already saved",
        },
    ];

    const CURSOR_BG: Color = Color::Rgb(0x33, 0x3c, 0x4d);

    /// The cursor's own column to the left of every row, held open whether the
    /// row is the one or not.
    const HIGHLIGHT: &'static str = "> ";

    /// Blank columns between a setting's title and its value.
    const GAP: usize = 2;

    const TITLE: &'static str = "Config";

    pub fn selected(&self) -> usize {
        self.selected
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

    fn block() -> Block<'static> {
        Block::bordered()
            .title_top(Line::from(Self::TITLE).centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue))
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
            items.push(ListItem::new(Line::from(vec![
                Span::styled(row.title, Style::new().fg(Color::White)),
                Span::raw(" ".repeat(padding)),
                Span::styled(row.value, Style::new().fg(Color::Yellow)),
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

    /// The box, centred, as large as what it draws and never larger than
    /// `area`. What does not fit in its height scrolls.
    fn outer_area(rows: &[ConfigRow], area: Rect) -> Rect {
        let highlight = Span::raw(Self::HIGHLIGHT).width();
        let title_width = Self::title_width(rows);
        let widest = rows
            .iter()
            .flat_map(|row| {
                let value = row
                    .choices
                    .iter()
                    .map(|choice| Span::raw(*choice).width())
                    .max()
                    .unwrap_or(0);
                [
                    highlight + title_width + Self::GAP + value,
                    Span::raw(row.help).width(),
                    highlight + Span::raw(row.section).width(),
                ]
            })
            .chain([Span::raw(Self::TITLE).width()])
            .max()
            .unwrap_or(0);

        // What the block takes for its borders and padding, measured off an
        // area it is given rather than counted.
        let inner = Self::block().inner(area);
        let chrome_x = area.width - inner.width;
        let chrome_y = area.height - inner.height;

        // Every row and heading, a blank line, the help line, and the chrome.
        let lines = rows.len() + Self::headings(rows) + 2;
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

    /// Draws the box over `area`, listing `rows`, with the help of the one
    /// under the cursor at the bottom.
    pub fn render(&mut self, rows: &[ConfigRow], area: Rect, buf: &mut Buffer) {
        let outer = Self::outer_area(rows, area);
        Clear.render(outer, buf);
        Self::block().render(outer, buf);

        let [list_area, _, help_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(Self::block().inner(outer));

        let (items, cursor) = self.items(rows, Self::title_width(rows));
        self.list_state.select(Some(cursor));
        StatefulWidget::render(
            List::new(items)
                .highlight_style(Style::new().bg(Self::CURSOR_BG))
                .highlight_symbol(Self::HIGHLIGHT),
            list_area,
            buf,
            &mut self.list_state,
        );

        if let Some(row) = rows.get(self.selected) {
            let help = ui::clip(row.help, usize::from(help_area.width));
            Line::from(Span::raw(help).dim()).render(help_area, buf);
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
                value: "on",
                choices: vec!["on", "off"],
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
    /// list longer than the box is still one every setting can be reached in.
    #[test]
    fn every_setting_can_be_reached_on_an_eighty_by_twenty_four_terminal() {
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
    fn the_setting_under_the_cursor_says_what_it_decides() {
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

    /// Stepping through a setting's values never moves the box: it was
    /// measured against the widest of them from the start.
    #[test]
    fn the_box_holds_still_while_a_value_changes() {
        let mut rows = rows(1);
        rows[0].choices = vec!["on", "a much longer value"];

        let before = ConfigView::outer_area(&rows, SMALL);
        rows[0].value = "a much longer value";
        let after = ConfigView::outer_area(&rows, SMALL);

        assert_eq!(before, after);
    }

    #[test]
    fn config_keys_bind_every_key_once() {
        crate::keys::validate(ConfigView::CONFIG_KEYS).unwrap();
    }

    #[test]
    fn the_cursor_wraps_at_either_end() {
        let mut view = ConfigView::default();

        view.move_selection(VerticalDir::Up, 3);
        assert_eq!(view.selected(), 2);
        view.move_selection(VerticalDir::Down, 3);
        assert_eq!(view.selected(), 0);
    }
}
