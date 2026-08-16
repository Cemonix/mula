use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Widget},
};

use crate::keys::{Binding, KeyBinding};

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
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub struct Dialog {
    title: String,
    message: String,
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

    const WIDTH: u16 = 44;
    const HEIGHT: u16 = 8;
    /// Blank columns around a button label. Even, so the label always splits
    /// the leftover space exactly in half.
    const BUTTON_PADDING: u16 = 4;

    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            // Start on the harmless answer, so a stray Enter never deletes anything.
            choice: Choice::No,
        }
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

impl Widget for &Dialog {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [area] = Layout::horizontal([Constraint::Length(Dialog::WIDTH)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(Dialog::HEIGHT)])
            .flex(Flex::Center)
            .areas(area);

        Clear.render(area, buf);

        let block = Block::bordered()
            .title_top(Line::from(self.title.as_str()).centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue));
        let inner = block.inner(area);
        block.render(area, buf);

        let [message_area, buttons_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

        // Paragraph aligns horizontally but never vertically, so shrink the area
        // down to the text and let Flex center what is left over.
        let [text_area] =
            Layout::vertical([Constraint::Length(self.message.lines().count() as u16)])
                .flex(Flex::Center)
                .areas(message_area);

        Paragraph::new(self.message.as_str())
            .centered()
            .style(Color::White)
            .render(text_area, buf);

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

    #[test]
    fn a_fresh_dialog_starts_on_no() {
        assert_eq!(
            Dialog::new("Delete", "Delete 3 items?").choice(),
            Choice::No
        );
    }
}
