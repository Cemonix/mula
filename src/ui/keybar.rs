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

#[cfg(test)]
mod keybar_tests {
    use super::*;
    use ratatui::crossterm::event::KeyCode;

    use crate::keys::{BROWSE_KEYS, GLOBAL_KEYS, GlobalMsg, find};
    use crate::{ui::dialog::Dialog, ui::finder::Finder, ui::help, ui::prompt::Prompt};

    /// The narrowest terminal the bar is curated against.
    const COLUMNS: u16 = 80;

    /// Columns the entries carrying a `bar` label take, plus the separators
    /// between them. Measured the way `render` lays them out.
    fn bar_width<T>(bindings: &[Binding<T>]) -> u16 {
        let labelled: Vec<&Binding<T>> = bindings.iter().filter(|b| b.bar.is_some()).collect();
        let entries: u16 = labelled
            .iter()
            .map(|b| b.key.to_span().width() as u16 + 1 + Span::raw(b.bar.unwrap()).width() as u16)
            .sum();
        let separators = labelled.len().saturating_sub(1) as u16;
        entries + separators * Span::raw(Keybar::<T>::SEPARATOR).width() as u16
    }

    fn help_width() -> u16 {
        let key = find(GLOBAL_KEYS, |m| matches!(m, GlobalMsg::ShowHelp))
            .expect("the globals reach the help overlay");
        key.key.to_span().width() as u16 + Span::raw(Keybar::<()>::HELP_LABEL).width() as u16
    }

    /// The bar is curated rather than truncated: an entry that does not fit
    /// belongs under `?` instead. The help hint is right-aligned into the same
    /// row, so it counts against the same 80 columns.
    #[test]
    fn the_browse_bar_fits_eighty_columns_beside_the_help_hint() {
        let width = bar_width(BROWSE_KEYS) + help_width();

        assert!(width <= COLUMNS, "the browse bar takes {width} columns");
    }

    /// Every mode draws the help hint into its row, so every mode's bar is
    /// measured beside it. The help overlay is the exception: its own key
    /// closes it, so it draws no hint.
    #[test]
    fn every_overlay_bar_fits_eighty_columns() {
        for (name, width) in [
            ("dialog", bar_width(Dialog::DIALOG_KEYS) + help_width()),
            ("prompt", bar_width(Prompt::PROMPT_KEYS) + help_width()),
            ("finder", bar_width(Finder::FIND_KEYS) + help_width()),
            ("help", bar_width(help::HELP_KEYS)),
        ] {
            assert!(width <= COLUMNS, "the {name} bar takes {width} columns");
        }
    }

    #[test]
    fn only_bindings_carrying_a_label_reach_the_bar() {
        let table = [
            Binding {
                key: KeyBinding::plain(KeyCode::F(5)),
                msg: (),
                bar: Some("Copy"),
                help: "labelled",
            },
            Binding {
                key: KeyBinding::plain(KeyCode::F(6)),
                msg: (),
                bar: None,
                help: "unlabelled",
            },
        ];
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);

        Keybar::new(&table).render(area, &mut buf);
        let row: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();

        assert!(row.contains("F5 Copy"), "the row was {row:?}");
        assert!(!row.contains("F6"), "the row was {row:?}");
    }
}
