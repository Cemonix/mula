use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Span, Text},
    widgets::{Block, Clear, Paragraph, Widget},
};

use crate::ui;

#[derive(Debug)]
pub enum ToastLevel {
    Info,
    Warning,
    Error,
}

impl ToastLevel {
    /// How long a toast of this level stays up when it was created without an
    /// explicit duration.
    fn duration(&self) -> Duration {
        match self {
            ToastLevel::Info => Duration::from_secs(3),
            ToastLevel::Warning => Duration::from_secs(5),
            ToastLevel::Error => Duration::from_secs(8),
        }
    }

    fn color(&self) -> Color {
        match self {
            ToastLevel::Info => Color::Blue,
            ToastLevel::Warning => Color::Yellow,
            ToastLevel::Error => Color::Red,
        }
    }
}

#[derive(Debug)]
pub struct Toast {
    message: String,
    level: ToastLevel,
    created_at: Instant,
    duration: Option<Duration>,
}

impl Toast {
    /// Widest a toast gets, borders included. A longer message wraps.
    const MAX_WIDTH: u16 = 48;
    /// Rows and columns the two opposite borders take together.
    const BORDER: u16 = 2;

    pub fn new(message: impl Into<String>, level: ToastLevel, duration: Option<Duration>) -> Self {
        Self {
            message: message.into(),
            level,
            created_at: Instant::now(),
            duration,
        }
    }

    /// Reports whether the toast has outlived its duration, falling back to the
    /// duration of its level when it was created without one.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration.unwrap_or(self.level.duration())
    }

    /// Columns the toast asks for, borders included, capped at `MAX_WIDTH` and
    /// at `available`. A message shorter than the cap gets a narrower toast.
    pub fn width(&self, available: u16) -> u16 {
        let longest = self
            .lines(available.min(Self::MAX_WIDTH))
            .iter()
            .map(|line| Span::raw(line.as_str()).width())
            .max()
            .unwrap_or(0) as u16;
        longest + Self::BORDER
    }

    /// Rows the message needs at `width`, borders included.
    pub fn height(&self, width: u16) -> u16 {
        self.lines(width).len() as u16 + Self::BORDER
    }

    /// The message wrapped to the columns `width` leaves inside the borders.
    /// Both the size the caller lays out and the text `render` draws come from
    /// here, so they cannot disagree.
    fn lines(&self, width: u16) -> Vec<String> {
        ui::wrap(&self.message, Self::inner(width) as usize)
    }

    /// The columns left for text inside `width`. A `width` with no room for the
    /// borders keeps all of them, so the result is never zero.
    fn inner(width: u16) -> u16 {
        match width.checked_sub(Self::BORDER) {
            Some(0) | None => width.max(1),
            Some(inner) => inner,
        }
    }
}

impl Widget for &Toast {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        Paragraph::new(Text::from_iter(self.lines(area.width)))
            .block(Block::bordered().border_style(Style::new().fg(self.level.color())))
            .render(area, buf);
    }
}

#[cfg(test)]
mod toast_tests {
    use super::*;

    fn toast(message: &str) -> Toast {
        Toast::new(message, ToastLevel::Info, None)
    }

    #[test]
    fn a_short_message_asks_only_for_what_it_needs() {
        assert_eq!(toast("boom").width(80), 4 + Toast::BORDER);
    }

    #[test]
    fn width_is_capped_by_the_maximum_and_by_the_area() {
        let long = toast(&"x".repeat(200));
        assert_eq!(long.width(80), Toast::MAX_WIDTH);
        assert_eq!(long.width(20), 20);
    }

    /// `width` shrinks the toast to its longest line, and `height` is then asked
    /// about that narrower width. Wrapping again at it has to give the same
    /// lines, or the box would not match the text drawn into it.
    #[test]
    fn shrinking_to_the_longest_line_does_not_rewrap() {
        let toast = toast("Deleted 3 of 5 items");
        let width = toast.width(12);
        assert_eq!(width, 12);
        assert_eq!(toast.height(width), 2 + Toast::BORDER);
    }

    #[test]
    fn a_width_too_narrow_for_the_borders_still_leaves_a_column() {
        assert_eq!(Toast::inner(0), 1);
        assert_eq!(Toast::inner(2), 2);
        assert_eq!(Toast::inner(3), 1);
    }

    #[test]
    fn the_level_decides_how_long_a_toast_without_a_duration_lives() {
        assert!(!Toast::new("boom", ToastLevel::Error, None).is_expired());
        assert!(Toast::new("boom", ToastLevel::Info, Some(Duration::ZERO)).is_expired());
    }
}
