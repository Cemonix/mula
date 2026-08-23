//! One line of editable text: the buffer, the cursor, and the window of it
//! that fits a given width. No box and no border, so an overlay that draws a
//! field of its own reuses the editing rather than writing it again.

use ratatui::text::Span;

/// Which way the cursor steps through the typed text.
#[derive(Clone, Copy, Debug)]
pub enum HorizontalDir {
    Left,
    Right,
}

#[derive(Debug, Default)]
pub struct TextInput {
    text: String,
    /// Character index, not a byte offset: the cursor sits between characters
    /// and a byte offset would land inside a multi-byte one.
    cursor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the buffer outright and puts the cursor after the new text.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.chars().count();
    }

    pub fn move_cursor(&mut self, dir: HorizontalDir) {
        let char_len = self.text.chars().count();
        match dir {
            HorizontalDir::Left => self.cursor = self.cursor.saturating_sub(1),
            HorizontalDir::Right => self.cursor = (self.cursor + 1).min(char_len),
        }
    }

    pub fn insert(&mut self, c: char) {
        let byte_pos = self.byte_offset(self.cursor);
        self.text.insert(byte_pos, c);
        self.cursor += 1;
    }

    /// Removes the character before the cursor, if any.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let byte_pos = self.byte_offset(self.cursor - 1);
        self.text.remove(byte_pos);
        self.cursor -= 1;
    }

    /// The stretch of the buffer that fits `width`, scrolled only as far as
    /// keeping the cursor in view needs.
    pub fn visible_text(&self, width: u16) -> &str {
        let (start, end) = self.window(width);
        &self.text[self.byte_offset(start)..self.byte_offset(end)]
    }

    /// Columns from the left edge of a `width`-wide field to the cursor,
    /// scrolled the same way [`visible_text`](Self::visible_text) scrolls.
    pub fn cursor_column(&self, width: u16) -> u16 {
        let (start, _) = self.window(width);
        let before = &self.text[self.byte_offset(start)..self.byte_offset(self.cursor)];
        Span::from(before).width() as u16
    }

    /// Byte offset of the `char_idx`-th character, or the end of the buffer
    /// once `char_idx` reaches the character count.
    fn byte_offset(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(pos, _)| pos)
            .unwrap_or(self.text.len())
    }

    /// Char-index bounds `[start, end)` of width `visible_width` that contain
    /// the cursor, shifted only as far as needed to keep it in view.
    fn window(&self, visible_width: u16) -> (usize, usize) {
        let visible_width = visible_width as usize;
        let total = self.text.chars().count();
        if total <= visible_width {
            return (0, total);
        }
        let start = self
            .cursor
            .saturating_sub(visible_width.saturating_sub(1))
            .min(total - visible_width);
        (start, start + visible_width)
    }
}

#[cfg(test)]
mod text_input_tests {
    use super::*;

    fn holding(text: &str) -> TextInput {
        let mut input = TextInput::new();
        input.set_text(text);
        input
    }

    #[test]
    fn typing_lands_where_the_cursor_is() {
        let mut input = holding("ac");
        input.move_cursor(HorizontalDir::Left);
        input.insert('b');

        assert_eq!(input.text(), "abc");
    }

    #[test]
    fn the_cursor_counts_characters_rather_than_bytes() {
        let mut input = holding("čau");
        input.move_cursor(HorizontalDir::Left);
        input.backspace();

        assert_eq!(input.text(), "ču");
    }

    #[test]
    fn the_cursor_stops_at_either_end() {
        let mut input = holding("ab");
        for _ in 0..5 {
            input.move_cursor(HorizontalDir::Left);
        }
        input.backspace();
        assert_eq!(input.text(), "ab");

        for _ in 0..5 {
            input.move_cursor(HorizontalDir::Right);
        }
        input.insert('c');
        assert_eq!(input.text(), "abc");
    }

    #[test]
    fn text_that_fits_is_shown_whole_and_is_not_scrolled() {
        let input = holding("abc");

        assert_eq!(input.visible_text(10), "abc");
        assert_eq!(input.cursor_column(10), 3);
    }

    #[test]
    fn a_buffer_longer_than_the_field_scrolls_to_hold_the_cursor() {
        let input = holding("abcdefgh");

        // The cursor sits after the last character, so the window ends there.
        assert_eq!(input.visible_text(3), "fgh");
        assert_eq!(input.cursor_column(3), 3);
    }

    #[test]
    fn scrolling_back_follows_the_cursor_to_the_start() {
        let mut input = holding("abcdefgh");
        for _ in 0..8 {
            input.move_cursor(HorizontalDir::Left);
        }

        assert_eq!(input.visible_text(3), "abc");
        assert_eq!(input.cursor_column(3), 0);
    }
}
