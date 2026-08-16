use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span, ToSpan},
    widgets::Widget,
};

use crate::keys::{Binding, KeyBinding};

/// The bottom row. Draws the entries of `bindings` that carry a `bar` label as
/// `key label`, the number of marked items, and `help_key` followed by `Help`.
///
/// Reads only `key` and `bar`, never `msg`, so `T` needs no bound and any
/// binding table can be passed in. `help_key` is resolved by the caller and
/// simply left out when `None`.
pub struct Keybar<'b, T> {
    bindings: &'b [Binding<T>],
    help_key: Option<KeyBinding>,
}

impl<'b, T> Keybar<'b, T> {
    /// Columns between two entries. Wider than the single space inside an entry,
    /// so `key label` reads as one unit.
    const SEPARATOR: &'static str = "  ";
    const HELP_LABEL: &'static str = " Help";

    pub fn new(bindings: &'b [Binding<T>]) -> Self {
        Self {
            bindings,
            help_key: None,
        }
    }

    pub fn help_key(mut self, key: Option<KeyBinding>) -> Self {
        self.help_key = key;
        self
    }
}

impl<'b, T> Widget for Keybar<'b, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = Vec::new();
        for binding in self.bindings {
            if let Some(bar) = binding.bar {
                if !spans.is_empty() {
                    spans.push(Span::raw(Self::SEPARATOR));
                }
                spans.push(binding.key.to_span().bold());
                spans.push(Span::raw(" "));
                spans.push(Span::raw(bar));
            }
        }

        buf.set_style(area, Style::new().fg(Color::White).bg(Color::DarkGray));

        Widget::render(Line::from(spans), area, buf);

        // Both lines share one area. The second only overwrites the cells it
        // draws into, but a `Line` lays its own style over the whole area first,
        // so the styling stays on the spans and never on the line.
        if let Some(help) = self.help_key {
            let help = Line::from(vec![help.to_span().bold(), Span::raw(Self::HELP_LABEL)]);
            Widget::render(help.right_aligned(), area, buf);
        }
    }
}
