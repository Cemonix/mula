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

    /// Whether `name` matches the filter, ignoring case.
    ///
    /// The filter is a pattern over the whole name, where `*` stands for any
    /// run of characters and `?` for exactly one. A pattern holding neither is
    /// read as `*pattern*`, so three letters go on finding a name by its
    /// middle — which is the whole reason to type three letters — while
    /// `*.log` means the extension and not merely those characters somewhere.
    ///
    /// One rule rather than two: the plain case is the wildcard case with the
    /// stars left implied.
    pub fn matches(&self, name: &str) -> bool {
        if self.text.is_empty() {
            return true;
        }

        let pattern = self.text.to_lowercase();
        let pattern = match pattern.contains(['*', '?']) {
            true => pattern,
            false => format!("*{pattern}*"),
        };

        matches_pattern(
            &pattern.chars().collect::<Vec<char>>(),
            &name.to_lowercase().chars().collect::<Vec<char>>(),
        )
    }
}

/// Matches `name` against `pattern` from end to end.
///
/// A `*` is remembered along with how much of the name it had taken, so a
/// later mismatch hands it one more character and tries again rather than
/// giving up. That backtracking is what lets a single pass handle a pattern
/// with several stars in it.
fn matches_pattern(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;

    while n < name.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, n));
                p += 1;
            }
            Some('?') => {
                p += 1;
                n += 1;
            }
            Some(c) if *c == name[n] => {
                p += 1;
                n += 1;
            }
            _ => match star {
                Some((at, taken)) => {
                    p = at + 1;
                    n = taken + 1;
                    star = Some((at, taken + 1));
                }
                None => return false,
            },
        }
    }

    // What is left of the pattern has to be able to match nothing at all.
    pattern[p.min(pattern.len())..].iter().all(|c| *c == '*')
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

    fn filter(text: &str) -> Filter {
        let mut filter = Filter::default();
        for c in text.chars() {
            filter.push(c);
        }
        filter
    }

    #[test]
    fn a_filter_matches_anywhere_in_the_name_and_ignores_case() {
        let filter = filter("LoG");

        assert!(filter.matches("mula.log"));
        assert!(filter.matches("LOGBOOK"));
        assert!(!filter.matches("notes.txt"));
    }

    /// The reason to type `*.log` rather than `log` is to mean the extension.
    /// A pattern anchors, so what merely holds those characters is out.
    #[test]
    fn a_star_pattern_matches_the_whole_name() {
        let filter = filter("*.log");

        assert!(filter.matches("mula.log"));
        assert!(filter.matches("A.LOG"));
        assert!(!filter.matches("logbook.txt"));
        assert!(!filter.matches("mula.log.gz"));
    }

    #[test]
    fn a_star_matches_a_run_of_any_length_including_none() {
        assert!(filter("log*").matches("log"));
        assert!(filter("log*").matches("logbook"));
        assert!(!filter("log*").matches("catalog"));
        assert!(filter("*").matches("anything"));
    }

    #[test]
    fn a_question_mark_matches_exactly_one_character() {
        let filter = filter("?.log");

        assert!(filter.matches("a.log"));
        assert!(!filter.matches("ab.log"));
        assert!(!filter.matches(".log"));
    }

    /// Backtracking: the first star has to give characters back once the rest
    /// of the pattern fails further along.
    #[test]
    fn several_stars_match_across_the_whole_name() {
        let filter = filter("*mula*log*");

        assert!(filter.matches("the-mula-file.log.gz"));
        assert!(filter.matches("mulalog"));
        assert!(!filter.matches("mula.txt"));
    }

    /// Typing `*.log` one key at a time passes through `*.l`, which is a
    /// pattern of its own and matches nothing here. The listing coming back
    /// empty for a keystroke is the pattern being honest, not a bug.
    #[test]
    fn a_half_typed_pattern_is_matched_as_it_stands() {
        assert!(!filter("*.l").matches("mula.log"));
        assert!(filter("*.l").matches("script.l"));
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
