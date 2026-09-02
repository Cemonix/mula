use ratatui::crossterm::event::KeyCode;

use crate::keys::{Binding, KeyBinding};

/// What a filter key does. There is no cursor to move: a filter is typed and
/// backspaced, and the listing narrows under it as it goes.
#[derive(Clone, Copy, Debug)]
pub enum FilterMsg {
    Confirm,
    Cancel,
}

/// The text a pane's listing is narrowed to.
///
/// Empty is no filter rather than a filter matching nothing, so the state the
/// user leaves behind by deleting what they typed is the state they started
/// from.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    text: String,
}

impl Filter {
    /// The keys that work while a filter is being typed. Everything printable
    /// goes into the filter instead, which is why none of these is a character.
    ///
    /// `Confirm` keeps what was typed and hands the keys back to Browse, so the
    /// narrowed listing can be marked and operated on. `Cancel` drops it.
    pub const FILTER_KEYS: &[Binding<FilterMsg>] = &[
        Binding {
            key: KeyBinding::plain(KeyCode::Enter),
            msg: FilterMsg::Confirm,
            bar: Some("Keep"),
            help: "Keeps the filter and goes back to browsing what is left",
        },
        Binding {
            key: KeyBinding::plain(KeyCode::Esc),
            msg: FilterMsg::Cancel,
            bar: Some("Clear"),
            help: "Drops the filter and shows the whole listing again",
        },
    ];

    pub fn push(&mut self, c: char) {
        self.text.push(c);
    }

    pub fn pop(&mut self) {
        self.text.pop();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether `name` is in the filter, ignoring ASCII case.
    ///
    /// A substring rather than a prefix: what a name is recognised by is as
    /// often its extension as its beginning, and `log` finding `mula.log` is
    /// the reason to type three letters at all.
    pub fn matches(&self, name: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }
        name.to_lowercase().contains(&self.text.to_lowercase())
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::keys;

    #[test]
    fn filter_keys_bind_every_key_once() {
        keys::validate(Filter::FILTER_KEYS).unwrap();
    }

    /// The filter draws no overlay of its own, so a key of its own that is
    /// missing from the bar is named nowhere at all.
    #[test]
    fn every_filter_key_reaches_the_bar() {
        for binding in Filter::FILTER_KEYS {
            assert!(binding.bar.is_some(), "{} is named nowhere", binding.key);
        }
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        let filter = Filter::default();

        assert!(filter.matches("anything.txt"));
        assert!(filter.is_empty());
    }

    #[test]
    fn a_filter_matches_anywhere_in_the_name_and_ignores_case() {
        let mut filter = Filter::default();
        for c in "LoG".chars() {
            filter.push(c);
        }

        assert!(filter.matches("mula.log"));
        assert!(filter.matches("LOGBOOK"));
        assert!(!filter.matches("notes.txt"));
    }

    #[test]
    fn backspacing_everything_leaves_no_filter() {
        let mut filter = Filter::default();
        filter.push('a');
        filter.push('b');
        filter.pop();
        filter.pop();
        filter.pop();

        assert!(filter.is_empty());
        assert!(filter.matches("anything"));
    }
}
