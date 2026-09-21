use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span, ToSpan},
    widgets::Widget,
};

use crate::keys::{Binding, KeyBinding};

/// The bottom row. Draws `help_key` followed by `Help`, then the entries of
/// `bindings` that carry a `bar` label, each as `key label`.
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
        if let Some(help) = self.help_key {
            // Owned, since the key is a copy on the stack rather than part of
            // the table the other spans borrow from.
            spans.push(Span::raw(help.to_string()).bold());
            spans.push(Span::raw(Self::HELP_LABEL));
        }
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

        // The styling stays on the spans: a `Line` lays its own style over the
        // whole area first, which would take the row's background with it.
        Widget::render(Line::from(spans), area, buf);
    }
}

#[cfg(test)]
mod keybar_tests {
    use super::*;
    use ratatui::crossterm::event::KeyCode;

    use crate::keys::{self, BROWSE_ACTIONS, GLOBAL_KEYS, GlobalMsg, find};
    use crate::{
        ui::dialog::Dialog, ui::favorites::FavoritesView, ui::filter::Filter, ui::finder::Finder,
        ui::help, ui::prompt::Prompt,
    };

    /// The narrowest terminal the bar is curated against.
    const COLUMNS: u16 = 80;

    /// The key the hint is drawn from, taken from the globals the way `App`
    /// takes it.
    fn help_key() -> Option<KeyBinding> {
        Some(
            find(GLOBAL_KEYS, |m| matches!(m, GlobalMsg::ShowHelp))
                .expect("the globals reach the help overlay")
                .key,
        )
    }

    /// Columns the row takes, read off the cells `render` drew rather than by
    /// adding its layout up a second time. The buffer is wide enough that a
    /// row reaching its end would mean the measurement was clipped, which the
    /// last cell being blank rules out.
    fn row_width<T>(bindings: &[Binding<T>], help: Option<KeyBinding>) -> u16 {
        let area = Rect::new(0, 0, 200, 1);
        let mut buf = Buffer::empty(area);

        Keybar::new(bindings).help_key(help).render(area, &mut buf);
        let row: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();

        assert!(row.ends_with(' '), "the row filled the buffer: {row:?}");
        Span::raw(row.trim_end()).width() as u16
    }

    /// The bar is curated rather than truncated: an entry that does not fit
    /// belongs under the help overlay instead. The hint leads the same row, so
    /// it counts against the same 80 columns.
    #[test]
    fn the_browse_bar_fits_eighty_columns_beside_the_help_hint() {
        let width = row_width(&keys::table(BROWSE_ACTIONS, |_| None), help_key());

        assert!(width <= COLUMNS, "the browse bar takes {width} columns");
    }

    /// Every mode draws the help hint into its row, so every mode's bar is
    /// measured beside it. The help overlay is the exception: its own key
    /// closes it, so it draws no hint.
    #[test]
    fn every_overlay_bar_fits_eighty_columns() {
        for (name, width) in [
            ("dialog", row_width(Dialog::DIALOG_KEYS, help_key())),
            ("prompt", row_width(Prompt::PROMPT_KEYS, help_key())),
            ("finder", row_width(Finder::FIND_KEYS, help_key())),
            (
                "favorites",
                row_width(FavoritesView::FAVORITE_KEYS, help_key()),
            ),
            ("filter", row_width(Filter::FILTER_KEYS, help_key())),
            ("help", row_width(help::HELP_KEYS, None)),
        ] {
            assert!(width <= COLUMNS, "the {name} bar takes {width} columns");
        }
    }

    /// The hint leads the row. It is the one key a user who knows nothing else
    /// has to be able to find, and the right edge of a wide terminal is where
    /// it used to sit on its own, far from everything it belongs with.
    #[test]
    fn the_help_hint_leads_the_row() {
        let table = [Binding {
            key: KeyBinding::plain(KeyCode::Char('q')),
            msg: (),
            bar: Some("Quit"),
            help: "labelled",
        }];
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);

        Keybar::new(&table)
            .help_key(Some(KeyBinding::plain(KeyCode::F(1))))
            .render(area, &mut buf);
        let row: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();

        assert!(row.starts_with("F1 Help  q Quit"), "the row was {row:?}");
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
