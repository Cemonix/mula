use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::Widget,
};

/// The row above the key bar. Draws the state of the focused tab as segments
/// separated by `SEPARATOR`; today only the number of marked items.
///
/// Every field is a count or a flag; the widget turns it into text. Segments are
/// laid out in the order `segments` returns them, and the first one that does
/// not fit in `area` ends the row — it and everything after it are left out.
#[derive(Debug, Default)]
pub struct InfoBar {
    marked: usize,
}

impl InfoBar {
    /// Drawn between two segments, never before the first or after the last.
    const SEPARATOR: &'static str = " | ";

    pub fn new() -> Self {
        Self::default()
    }

    pub fn marked(mut self, count: usize) -> Self {
        self.marked = count;
        self
    }

    /// Returns the segments in the order they are drawn, most important first.
    /// A segment with nothing to report is omitted, so an idle bar is empty.
    fn segments(&self) -> Vec<Span<'static>> {
        let mut segments = Vec::new();

        if self.marked > 0 {
            segments.push(Span::raw(format!("{} marked", self.marked)).bold());
        }

        segments
    }
}

impl Widget for InfoBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Sets the foreground only; the row keeps the terminal background.
        buf.set_style(area, Style::new().fg(Color::White));

        let mut spans: Vec<Span> = Vec::new();
        let mut width = 0;
        for segment in self.segments() {
            let separator = if spans.is_empty() {
                0
            } else {
                Span::raw(Self::SEPARATOR).width()
            };
            if width + separator + segment.width() > area.width as usize {
                break;
            }

            width += separator + segment.width();
            if separator > 0 {
                spans.push(Span::raw(Self::SEPARATOR));
            }
            spans.push(segment);
        }

        Line::from(spans).render(area, buf);
    }
}

#[cfg(test)]
mod infobar_tests {
    use super::*;

    /// Renders the bar into a one-row buffer of the given width and returns the
    /// symbols of that row as a single string.
    fn render(bar: InfoBar, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        (0..width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn the_marked_count_is_drawn() {
        assert_eq!(render(InfoBar::new().marked(3), 40), "3 marked");
    }

    #[test]
    fn nothing_marked_leaves_the_row_empty() {
        assert_eq!(render(InfoBar::new(), 40), "");
    }

    #[test]
    fn a_segment_that_does_not_fit_is_dropped() {
        assert_eq!(render(InfoBar::new().marked(12), 5), "");
    }
}
