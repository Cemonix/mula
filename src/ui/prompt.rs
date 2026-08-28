use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Clear, Padding, Paragraph, Widget},
};

use crate::{
    keys::{Binding, KeyBinding},
    ui::text_input::{HorizontalDir, TextInput},
};

#[derive(Clone, Copy, Debug)]
pub enum InputMsg {
    MoveCursor(HorizontalDir),
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub struct Prompt {
    title: String,
    input: TextInput,
}

impl Prompt {
    pub const PROMPT_KEYS: &[Binding<InputMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: InputMsg::MoveCursor(HorizontalDir::Left),
            bar: None,
            help: "Moves cursor left",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: InputMsg::MoveCursor(HorizontalDir::Right),
            bar: None,
            help: "Moves cursor right",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: InputMsg::Confirm,
            bar: Some("Confirm"),
            help: "Accepts the name that was typed",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: InputMsg::Cancel,
            bar: Some("Cancel"),
            help: "Closes the prompt and does nothing",
        },
    ];

    const WIDTH: u16 = 60;
    const HEIGHT: u16 = 3;

    pub fn new(title: impl Into<String>) -> Self {
        Prompt {
            title: title.into(),
            input: TextInput::new(),
        }
    }

    pub fn move_cursor(&mut self, dir: HorizontalDir) {
        self.input.move_cursor(dir);
    }

    pub fn text(&self) -> &str {
        self.input.text()
    }

    /// Replaces the buffer outright and puts the cursor after the new text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.input.set_text(text);
    }

    pub fn insert(&mut self, c: char) {
        self.input.insert(c);
    }

    /// Removes the character before the cursor, if any.
    pub fn backspace(&mut self) {
        self.input.backspace();
    }

    /// The `Block` both `render` and `cursor_screen_position` lay their area
    /// out against, so the two never compute the border/padding differently.
    fn block(&self) -> Block<'_> {
        Block::bordered()
            .title_top(Line::from(self.title.as_str()).centered())
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

    /// Where the terminal's own cursor belongs for the given outer `area`,
    /// scrolled the same way `render` scrolls the text it draws.
    pub fn cursor_screen_position(&self, area: Rect) -> (u16, u16) {
        let inner = self.block().inner(Self::outer_area(area));
        (inner.x + self.input.cursor_column(inner.width), inner.y)
    }
}

impl Widget for &Prompt {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let outer = Prompt::outer_area(area);
        Clear.render(outer, buf);

        let block = self.block();
        let inner = block.inner(outer);
        block.render(outer, buf);

        Paragraph::new(self.input.visible_text(inner.width))
            .style(Color::White)
            .render(inner, buf);
    }
}
