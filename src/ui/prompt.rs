use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Widget},
};

use crate::{
    action::NavDirection,
    keys::{Binding, KeyBinding},
};

#[derive(Clone, Copy, Debug)]
pub enum InputMsg {
    MoveCursor(NavDirection),
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub struct Prompt {
    title: String,
    text_buf: String,
    cursor_pos: usize,
}

impl Prompt {
    pub const PROMPT_KEYS: &[Binding<InputMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Left),
            msg: InputMsg::MoveCursor(NavDirection::Left),
            bar: None,
            help: "Moves cursor left",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Right),
            msg: InputMsg::MoveCursor(NavDirection::Right),
            bar: None,
            help: "Moves cursor right",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: InputMsg::Confirm,
            bar: Some("Confirm"),
            help: "Answers with the highlighted button",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: InputMsg::Cancel,
            bar: Some("Cancel"),
            help: "Closes the dialog and does nothing",
        },
    ];

    const WIDTH: u16 = 60;
    const HEIGHT: u16 = 3;

    pub fn new(title: impl Into<String>) -> Self {
        Prompt {
            title: title.into(),
            text_buf: String::new(),
            cursor_pos: 0,
        }
    }

    pub fn move_cursor(&mut self, nav_dir: NavDirection) {
        let char_len = self.text_buf.chars().count();
        match nav_dir {
            NavDirection::Left => self.cursor_pos = self.cursor_pos.saturating_sub(1),
            NavDirection::Right => self.cursor_pos = (self.cursor_pos + 1).min(char_len),
            _ => (),
        }
    }

    pub fn text(&self) -> &str {
        &self.text_buf
    }

    /// Replaces the buffer outright and puts the cursor after the new text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text_buf = text.into();
        self.cursor_pos = self.text_buf.chars().count();
    }

    pub fn insert(&mut self, c: char) {
        let byte_pos = self.byte_offset(self.cursor_pos);
        self.text_buf.insert(byte_pos, c);
        self.cursor_pos += 1;
    }

    /// Removes the character before the cursor, if any.
    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let byte_pos = self.byte_offset(self.cursor_pos - 1);
        self.text_buf.remove(byte_pos);
        self.cursor_pos -= 1;
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

    /// Byte offset of the `char_idx`-th character, or the end of the buffer
    /// once `char_idx` reaches the character count.
    fn byte_offset(&self, char_idx: usize) -> usize {
        self.text_buf
            .char_indices()
            .nth(char_idx)
            .map(|(pos, _)| pos)
            .unwrap_or(self.text_buf.len())
    }

    /// Char-index bounds `[start, end)` of width `visible_width` that contain
    /// the cursor, shifted only as far as needed to keep it in view.
    fn window(&self, visible_width: u16) -> (usize, usize) {
        let visible_width = visible_width as usize;
        let total = self.text_buf.chars().count();
        if total <= visible_width {
            return (0, total);
        }
        let start = self
            .cursor_pos
            .saturating_sub(visible_width.saturating_sub(1))
            .min(total - visible_width);
        (start, start + visible_width)
    }

    fn visible_text(&self, visible_width: u16) -> &str {
        let (start, end) = self.window(visible_width);
        &self.text_buf[self.byte_offset(start)..self.byte_offset(end)]
    }

    /// Where the terminal's own cursor belongs for the given outer `area`,
    /// scrolled the same way `render` scrolls the text it draws.
    pub fn cursor_screen_position(&self, area: Rect) -> (u16, u16) {
        let inner = self.block().inner(Self::outer_area(area));
        let (start, _) = self.window(inner.width);
        let before = &self.text_buf[self.byte_offset(start)..self.byte_offset(self.cursor_pos)];
        (inner.x + Span::from(before).width() as u16, inner.y)
    }
}

impl Widget for &Prompt {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let outer = Prompt::outer_area(area);
        Clear.render(outer, buf);

        let block = self.block();
        let inner = block.inner(outer);
        block.render(outer, buf);

        Paragraph::new(self.visible_text(inner.width))
            .style(Color::White)
            .render(inner, buf);
    }
}
