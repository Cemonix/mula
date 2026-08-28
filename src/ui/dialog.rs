use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Padding, Paragraph, Widget},
};

use crate::{
    action::VerticalDir,
    keys::{Binding, KeyBinding},
    ui,
};

/// Which button currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Yes,
    No,
}

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Choice::Yes => "Yes",
            Choice::No => "No",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum DialogMsg {
    Toggle,
    Scroll(VerticalDir),
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub struct Dialog {
    title: String,
    message: String,
    /// What the answer applies to, listed under the message. Empty for a
    /// question that is not about a set of things.
    items: Vec<String>,
    offset: usize,
    choice: Choice,
}

impl Dialog {
    // Both arrows carry `Toggle`, which moves to the other of the two buttons.
    pub const DIALOG_KEYS: &[Binding<DialogMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: DialogMsg::Toggle,
            bar: None,
            help: "Moves to the other button",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: DialogMsg::Toggle,
            bar: None,
            help: "Moves to the other button",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Up),
            msg: DialogMsg::Scroll(VerticalDir::Up),
            bar: None,
            help: "Scrolls the listed items up",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Down),
            msg: DialogMsg::Scroll(VerticalDir::Down),
            bar: None,
            help: "Scrolls the listed items down",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: DialogMsg::Confirm,
            bar: Some("Confirm"),
            help: "Answers with the highlighted button",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: DialogMsg::Cancel,
            bar: Some("Cancel"),
            help: "Closes the dialog and does nothing",
        },
    ];

    /// The box is as wide as what goes in it, held between these. Narrower
    /// than the shorter one reads as a slot rather than a dialog; wider than
    /// the longer one and the eye stops finding the buttons.
    const MIN_WIDTH: u16 = 44;
    const MAX_WIDTH: u16 = 72;
    /// Rows left free above and below, so the box reads as one.
    const MARGIN: u16 = 1;
    /// Blank columns around a button label. Even, so the label always splits
    /// the leftover space exactly in half.
    const BUTTON_PADDING: u16 = 4;
    /// Borders, and the columns `Padding::horizontal(1)` holds open.
    const CHROME: u16 = 4;

    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            items: Vec::new(),
            offset: 0,
            choice: Choice::No,
        }
    }

    /// Lists what the answer applies to under the message, scrolled when there
    /// are more of them than the terminal has room for.
    pub fn listing(mut self, items: Vec<String>) -> Self {
        self.items = items;
        self
    }

    pub fn choice(&self) -> Choice {
        self.choice
    }

    pub fn toggle(&mut self) {
        self.choice = match self.choice {
            Choice::Yes => Choice::No,
            Choice::No => Choice::Yes,
        };
    }

    /// Moves the listing by a row. Scrolling past either end is pulled back by
    /// the render, which is where the number of rows on screen is known.
    pub fn scroll(&mut self, dir: VerticalDir) {
        self.offset = match dir {
            VerticalDir::Up => self.offset.saturating_sub(1),
            VerticalDir::Down => self.offset.saturating_add(1),
        };
    }

    /// Columns the widest line wants, held between `MIN_WIDTH` and
    /// `MAX_WIDTH`. Measured in columns rather than bytes, since a name can
    /// hold anything the filesystem allowed.
    fn width(&self) -> u16 {
        let widest = self
            .message
            .lines()
            .chain(self.items.iter().map(String::as_str))
            .map(|line| Span::raw(line).width() as u16)
            .max()
            .unwrap_or(0);

        (widest + Self::CHROME).clamp(Self::MIN_WIDTH, Self::MAX_WIDTH)
    }

    /// Width in terminal columns, not bytes — `Span::width` is the same
    /// measurement the renderer uses when it lays the label out.
    fn button_width(label: &'static str) -> u16 {
        Span::from(label).width() as u16 + Self::BUTTON_PADDING
    }

    fn button(&self, choice: Choice) -> Paragraph<'static> {
        let style = if self.choice == choice {
            Style::new().bg(Color::Blue).fg(Color::White).bold()
        } else {
            Style::new().bg(Color::DarkGray).fg(Color::Gray)
        };
        Paragraph::new(choice.label()).centered().style(style)
    }
}

impl Widget for &mut Dialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let message_rows = self.message.lines().count() as u16;
        // A blank row parts the message from the listing, and is not there
        // when there is no listing to part it from.
        let gap = u16::from(!self.items.is_empty());
        let wanted = 2 + message_rows + gap + self.items.len() as u16 + 1;

        let [area] = Layout::horizontal([Constraint::Max(self.width())])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Max(wanted)])
            .flex(Flex::Center)
            .vertical_margin(Dialog::MARGIN)
            .areas(area);

        Clear.render(area, buf);

        let block = Block::bordered()
            .title_top(Line::from(self.title.as_str()).centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue));
        let inner = block.inner(area);

        let [message_area, _, list_area, buttons_area] = Layout::vertical([
            Constraint::Length(message_rows),
            Constraint::Length(gap),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

        let total = self.items.len();
        let rows = list_area.height as usize;
        self.offset = self.offset.min(total.saturating_sub(rows));
        let shown = &self.items[self.offset..(self.offset + rows).min(total)];

        let block = match ui::scroll_position(self.offset, shown.len(), total) {
            Some(position) => block.title_bottom(Line::from(position).centered()),
            None => block,
        };
        block.render(area, buf);

        Paragraph::new(self.message.as_str())
            .centered()
            .style(Color::White)
            .render(message_area, buf);

        Text::from_iter(shown.iter().map(|item| Line::from(item.as_str())))
            .style(Color::White)
            .render(list_area, buf);

        let [yes_area, no_area] = Layout::horizontal(
            [Choice::Yes, Choice::No].map(|c| Constraint::Length(Dialog::button_width(c.label()))),
        )
        .spacing(1)
        .flex(Flex::End)
        .areas(buttons_area);

        self.button(Choice::Yes).render(yes_area, buf);
        self.button(Choice::No).render(no_area, buf);
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::*;
    use crate::keys;

    #[test]
    fn dialog_keys_bind_every_key_once() {
        keys::validate(Dialog::DIALOG_KEYS).unwrap();
    }

    #[test]
    fn dialog_keys_cover_every_message() {
        for msg in [DialogMsg::Toggle, DialogMsg::Confirm, DialogMsg::Cancel] {
            assert!(
                keys::find(Dialog::DIALOG_KEYS, |m| std::mem::discriminant(m)
                    == std::mem::discriminant(&msg))
                .is_some(),
                "{msg:?} has no key"
            );
        }
    }

    /// Draws the dialog over an 80x24 terminal and returns its rows as text.
    fn frame(dialog: &mut Dialog) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);

        dialog.render(area, &mut buf);

        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn listing(count: usize) -> Dialog {
        let items = (0..count).map(|i| format!("file-{i}.txt")).collect();
        Dialog::new("Delete", "Permanently delete these items?").listing(items)
    }

    #[test]
    fn a_fresh_dialog_starts_on_no() {
        assert_eq!(
            Dialog::new("Delete", "Delete 3 items?").choice(),
            Choice::No
        );
    }

    #[test]
    fn a_question_without_a_listing_shows_only_its_message() {
        let mut dialog = Dialog::new("Quit", "One operation still running. Quit?");

        let rows = frame(&mut dialog).join("\n");

        assert!(rows.contains("One operation still running. Quit?"));
        assert!(!rows.contains(" of "), "the frame was {rows:?}");
    }

    #[test]
    fn a_listing_that_fits_is_shown_whole_and_is_not_scrolled() {
        let mut dialog = listing(3);

        let rows = frame(&mut dialog).join("\n");

        for name in ["file-0.txt", "file-1.txt", "file-2.txt"] {
            assert!(rows.contains(name), "{name} is missing from {rows:?}");
        }
        assert!(!rows.contains(" of "), "the frame was {rows:?}");
    }

    /// What the answer applies to has to be readable in full, however many
    /// items were marked, so the listing scrolls rather than being cut off.
    #[test]
    fn every_item_can_be_reached_by_scrolling() {
        let mut dialog = listing(60);
        let mut seen = String::new();
        let mut offset = usize::MAX;

        while dialog.offset != offset {
            offset = dialog.offset;
            seen.push_str(&frame(&mut dialog).join("\n"));
            dialog.scroll(VerticalDir::Down);
        }

        for i in 0..60 {
            let name = format!("file-{i}.txt");
            assert!(seen.contains(&name), "{name} is never on screen");
        }
    }

    #[test]
    fn scrolling_past_the_end_stops_at_the_last_screenful() {
        let mut dialog = listing(60);
        frame(&mut dialog);

        for _ in 0..200 {
            dialog.scroll(VerticalDir::Down);
        }
        let rows = frame(&mut dialog).join("\n");

        assert!(rows.contains("file-59.txt"), "the frame was {rows:?}");
        assert!(rows.contains("of 60"), "the frame was {rows:?}");
    }

    /// The buttons are the point of a dialog; a listing long enough to fill the
    /// screen must not push them off it.
    #[test]
    fn the_buttons_survive_a_listing_taller_than_the_terminal() {
        let mut dialog = listing(200);

        let rows = frame(&mut dialog).join("\n");

        assert!(rows.contains("Yes"), "the frame was {rows:?}");
        assert!(rows.contains("No"), "the frame was {rows:?}");
    }
}
