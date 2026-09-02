use std::{fmt, str::FromStr};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

use crate::{
    action::{Action, ListEnd, VerticalDir},
    fs::ops::{DeleteMode, TransferOp},
    open::Opener,
    ui::tab::{MarkOp, ToggleDirection},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl KeyBinding {
    const RELEVANT: KeyModifiers = KeyModifiers::CONTROL
        .union(KeyModifiers::ALT)
        .union(KeyModifiers::SHIFT);

    pub const fn plain(code: KeyCode) -> Self {
        Self {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    pub const fn ctrl(code: KeyCode) -> Self {
        Self {
            code,
            mods: KeyModifiers::CONTROL,
        }
    }

    pub const fn shift(self) -> Self {
        Self {
            code: self.code,
            mods: self.mods.union(KeyModifiers::SHIFT),
        }
    }

    pub const fn alt(self) -> Self {
        Self {
            code: self.code,
            mods: self.mods.union(KeyModifiers::ALT),
        }
    }

    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.code == event.code && self.mods == event.modifiers.intersection(KeyBinding::RELEVANT)
    }
}

/// The three codes crossterm spells differently on macOS (`Delete` for
/// Backspace, `Fwd Del` for Delete, `Return` for Enter). One spelling has to
/// serve both the help overlay and a config file, and a config file is carried
/// between machines, so these are named here and the rest is left to crossterm.
fn code_name(code: KeyCode, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match code {
        KeyCode::Backspace => f.write_str("Backspace"),
        KeyCode::Delete => f.write_str("Delete"),
        KeyCode::Enter => f.write_str("Enter"),
        code => write!(f, "{code}"),
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            write!(f, "Ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            write!(f, "Alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            write!(f, "Shift+")?;
        }
        code_name(self.code, f)
    }
}

/// Strips one leading `Ctrl+`, `Alt+` or `Shift+`, or returns `None` when what
/// comes before the first `+` is not a modifier.
///
/// Splitting the whole string on `+` would lose `+` itself as a key: `Ctrl++`
/// is Ctrl and the plus sign, and stops here with `+` left over.
fn strip_modifier(s: &str) -> Option<(KeyModifiers, &str)> {
    let (head, rest) = s.split_once('+')?;
    let modifier = match head.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => KeyModifiers::CONTROL,
        "alt" | "opt" | "option" => KeyModifiers::ALT,
        "shift" => KeyModifiers::SHIFT,
        _ => return None,
    };
    Some((modifier, rest))
}

/// Reads a key name the way [`code_name`] writes one: case and inner spaces are
/// ignored, so `Page Up`, `PageUp` and `pageup` are one key.
fn parse_code(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }

    let flat: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();

    if let Some(number) = flat.strip_prefix('f')
        && let Ok(n) = number.parse::<u8>()
    {
        return Some(KeyCode::F(n));
    }

    Some(match flat.as_str() {
        "backspace" => KeyCode::Backspace,
        "delete" | "del" | "fwddel" => KeyCode::Delete,
        "enter" | "return" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "insert" => KeyCode::Insert,
        "esc" | "escape" => KeyCode::Esc,
        _ => return None,
    })
}

impl FromStr for KeyBinding {
    type Err = BindingError;

    /// Reads `Ctrl+Alt+Shift+<key>`, the shape [`Display`] writes.
    ///
    /// A capital letter carries `SHIFT` whether or not the name says so:
    /// crossterm sets the bit from `is_uppercase` and leaves the letter as
    /// typed, so `G` alone would otherwise be a binding no key event matches.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = s.trim();
        while let Some((modifier, tail)) = strip_modifier(rest) {
            mods |= modifier;
            rest = tail;
        }

        let code = parse_code(rest.trim()).ok_or_else(|| BindingError::UnknownKey {
            name: s.trim().to_string(),
        })?;
        if let KeyCode::Char(c) = code
            && c.is_uppercase()
        {
            mods |= KeyModifiers::SHIFT;
        }

        Ok(Self { code, mods })
    }
}

#[derive(Error, Debug)]
pub enum BindingError {
    #[error("{key} is bound more than once")]
    Duplicate { key: KeyBinding },
    #[error("{name} is not the name of a key")]
    UnknownKey { name: String },
}

#[derive(Debug)]
pub struct Binding<T> {
    pub key: KeyBinding,
    pub msg: T,
    pub bar: Option<&'static str>,
    pub help: &'static str,
}

/// One action a config file can name, and everything about it that stays in
/// code: how it reads in the bar and the help, and which keys reach it when no
/// config says otherwise.
///
/// A config replaces `keys` and nothing else, so a rebound action keeps its
/// label, its help line and its place in both listings.
pub struct Entry {
    pub name: &'static str,
    pub action: Action,
    pub keys: &'static [KeyBinding],
    pub bar: Option<&'static str>,
    pub help: &'static str,
}

/// Builds the table `resolve` walks, in catalogue order, asking `bound` which
/// keys reach each action and falling back to the entry's own.
///
/// Only the first key of an action carries the bar label: an action reachable
/// two ways is still one entry in the bar, and every one of its keys is listed
/// in the help.
pub fn table<'k>(
    catalogue: &[Entry],
    bound: impl Fn(&str) -> Option<&'k [KeyBinding]>,
) -> Vec<Binding<Action>> {
    let mut table = Vec::new();
    for entry in catalogue {
        let keys = bound(entry.name).unwrap_or(entry.keys);
        for (i, key) in keys.iter().enumerate() {
            table.push(Binding {
                key: *key,
                msg: entry.action,
                bar: entry.bar.filter(|_| i == 0),
                help: entry.help,
            });
        }
    }
    table
}

/// Returns the message the first matching binding carries, or `None` if the key is unbound.
pub fn resolve<T: Copy>(bindings: &[Binding<T>], event: &KeyEvent) -> Option<T> {
    bindings
        .iter()
        .find(|b| b.key.matches(event))
        .map(|b| b.msg)
}

/// Returns the first binding whose message satisfies `pred`, or `None` if no
/// binding does.
pub fn find<T>(bindings: &[Binding<T>], pred: impl Fn(&T) -> bool) -> Option<&Binding<T>> {
    bindings.iter().find(|b| pred(&b.msg))
}

/// Compares every entry against the ones after it and returns the first key that
/// two entries share. Only the key is compared; two entries may carry one message.
pub fn validate<T>(bindings: &[Binding<T>]) -> Result<(), BindingError> {
    for (i, binding) in bindings.iter().enumerate() {
        if bindings[i + 1..]
            .iter()
            .any(|other| other.key == binding.key)
        {
            return Err(BindingError::Duplicate { key: binding.key });
        }
    }
    Ok(())
}

/// What a key does whatever is on screen.
#[derive(Clone, Copy, Debug)]
pub enum GlobalMsg {
    ShowHelp,
}

/// The keys that work in every mode, resolved before the mode's own table so
/// nothing can shadow them. A global carries no `bar` label: the keybar draws
/// it right-aligned into every mode's row instead.
///
/// F1 rather than `?`, because a global has to survive the modes that read
/// text — a prompt and the find overlay would swallow a printable character.
pub const GLOBAL_KEYS: &[Binding<GlobalMsg>] = &[Binding {
    key: KeyBinding::plain(KeyCode::F(1)),
    msg: GlobalMsg::ShowHelp,
    bar: None,
    help: "Lists every key available right now",
}];

/// Every action a key can reach in Browse. `table` walks this in order, so it
/// is also the order `resolve` matches in and the order the help overlay lists.
pub const BROWSE_ACTIONS: &[Entry] = &[
    Entry {
        name: "cursor.up",
        action: Action::MoveCursor(VerticalDir::Up),
        keys: &[KeyBinding::plain(KeyCode::Up)],
        bar: None,
        help: "Moves the cursor one item up",
    },
    Entry {
        name: "cursor.down",
        action: Action::MoveCursor(VerticalDir::Down),
        keys: &[KeyBinding::plain(KeyCode::Down)],
        bar: None,
        help: "Moves the cursor one item down",
    },
    Entry {
        name: "cursor.first",
        action: Action::MoveCursorTo(ListEnd::First),
        keys: &[KeyBinding::plain(KeyCode::Char('g'))],
        bar: None,
        help: "Moves the cursor to the first item",
    },
    Entry {
        name: "cursor.last",
        action: Action::MoveCursorTo(ListEnd::Last),
        keys: &[KeyBinding::plain(KeyCode::Char('G')).shift()],
        bar: None,
        help: "Moves the cursor to the last item",
    },
    Entry {
        name: "panel.toggle",
        action: Action::ToggleSide,
        keys: &[KeyBinding::plain(KeyCode::Tab)],
        bar: None,
        help: "Focuses the other panel",
    },
    Entry {
        name: "entry.open",
        action: Action::OpenSelected,
        keys: &[KeyBinding::plain(KeyCode::Enter)],
        bar: None,
        help: "Enters a directory, or opens a file with what belongs to it",
    },
    Entry {
        name: "panel.parent",
        action: Action::GoToParent,
        keys: &[KeyBinding::plain(KeyCode::Backspace)],
        bar: None,
        help: "Leaves for the directory above, wherever the cursor is",
    },
    Entry {
        name: "mark.toggle",
        action: Action::ToggleMark,
        keys: &[KeyBinding::plain(KeyCode::Char(' '))],
        bar: None,
        help: "Marks or unmarks the item under the cursor",
    },
    Entry {
        name: "mark.up",
        action: Action::MarkAndMove {
            op: MarkOp::Mark,
            nav_dir: VerticalDir::Up,
        },
        keys: &[KeyBinding::plain(KeyCode::Up).shift()],
        bar: None,
        help: "Marks the item under the cursor and moves up",
    },
    Entry {
        name: "mark.down",
        action: Action::MarkAndMove {
            op: MarkOp::Mark,
            nav_dir: VerticalDir::Down,
        },
        keys: &[KeyBinding::plain(KeyCode::Down).shift()],
        bar: None,
        help: "Marks the item under the cursor and moves down",
    },
    Entry {
        name: "unmark.up",
        action: Action::MarkAndMove {
            op: MarkOp::Unmark,
            nav_dir: VerticalDir::Up,
        },
        keys: &[KeyBinding::plain(KeyCode::Up).alt()],
        bar: None,
        help: "Unmarks the item under the cursor and moves up",
    },
    Entry {
        name: "unmark.down",
        action: Action::MarkAndMove {
            op: MarkOp::Unmark,
            nav_dir: VerticalDir::Down,
        },
        keys: &[KeyBinding::plain(KeyCode::Down).alt()],
        bar: None,
        help: "Unmarks the item under the cursor and moves down",
    },
    Entry {
        name: "mark.all",
        action: Action::MarkAll,
        keys: &[KeyBinding::plain(KeyCode::Char('a'))],
        bar: None,
        help: "Marks every item the panel is showing",
    },
    Entry {
        name: "panel.clear",
        action: Action::Clear,
        keys: &[KeyBinding::plain(KeyCode::Esc)],
        bar: None,
        help: "Drops the filter, or unmarks everything when there is no filter",
    },
    Entry {
        name: "entry.rename",
        action: Action::Rename,
        keys: &[KeyBinding::plain(KeyCode::F(2))],
        bar: Some("Rename"),
        help: "Renames the item under the cursor",
    },
    Entry {
        name: "entry.view",
        action: Action::Open(Opener::View),
        keys: &[KeyBinding::plain(KeyCode::F(3))],
        bar: Some("View"),
        help: "Opens the file under the cursor in $PAGER",
    },
    Entry {
        name: "entry.edit",
        action: Action::Open(Opener::Edit),
        keys: &[KeyBinding::plain(KeyCode::F(4))],
        bar: Some("Edit"),
        help: "Opens the file under the cursor in $EDITOR",
    },
    Entry {
        name: "transfer.copy",
        action: Action::Transfer {
            op: TransferOp::Copy,
        },
        keys: &[KeyBinding::plain(KeyCode::F(5))],
        bar: Some("Copy"),
        help: "Copies marked items into the other panel",
    },
    Entry {
        name: "transfer.move",
        action: Action::Transfer {
            op: TransferOp::Move,
        },
        keys: &[KeyBinding::plain(KeyCode::F(6))],
        bar: Some("Move"),
        help: "Moves marked items into the other panel",
    },
    Entry {
        name: "entry.create",
        action: Action::CreateEntry,
        keys: &[KeyBinding::plain(KeyCode::F(7))],
        bar: Some("New"),
        help: "Creates a file, or a folder if the name ends with /",
    },
    Entry {
        name: "entry.delete",
        action: Action::Delete(DeleteMode::Trash),
        keys: &[KeyBinding::plain(KeyCode::F(8))],
        bar: Some("Delete"),
        help: "Moves marked items to the trash, asking first",
    },
    Entry {
        name: "entry.delete-permanent",
        action: Action::Delete(DeleteMode::Permanent),
        keys: &[KeyBinding::plain(KeyCode::F(8)).shift()],
        bar: None,
        help: "Deletes marked items for good, without the trash, asking first",
    },
    Entry {
        name: "job.cancel",
        action: Action::CancelJob,
        keys: &[KeyBinding::plain(KeyCode::F(9))],
        bar: None,
        help: "Stops the running operation",
    },
    Entry {
        name: "view.quick",
        action: Action::ToggleQuickView,
        keys: &[KeyBinding::plain(KeyCode::Char('v'))],
        bar: None,
        help: "Shows what is under the cursor in the other panel",
    },
    Entry {
        name: "view.columns",
        action: Action::CycleColumns,
        keys: &[KeyBinding::plain(KeyCode::Char('c'))],
        bar: None,
        help: "Drops a column from the listing, and brings them all back",
    },
    Entry {
        name: "view.dotfiles",
        action: Action::ToggleDotFiles,
        keys: &[KeyBinding::plain(KeyCode::Char('.'))],
        bar: None,
        help: "Shows or hides the entries whose names begin with a dot",
    },
    Entry {
        name: "panel.find",
        action: Action::Find,
        keys: &[KeyBinding::plain(KeyCode::Char('/'))],
        bar: None,
        help: "Searches the tree below this panel for a name",
    },
    Entry {
        name: "panel.filter",
        action: Action::Filter,
        keys: &[KeyBinding::plain(KeyCode::Char('f'))],
        bar: None,
        help: "Narrows this listing to the names holding what you type",
    },
    Entry {
        name: "tab.new",
        action: Action::NewTab,
        keys: &[KeyBinding::plain(KeyCode::Char('t'))],
        bar: None,
        help: "Opens a new tab in the focused panel",
    },
    Entry {
        name: "tab.close",
        action: Action::CloseTab,
        keys: &[KeyBinding::plain(KeyCode::Char('w'))],
        bar: None,
        help: "Closes the active tab, unless it is the only one",
    },
    Entry {
        name: "tab.rename",
        action: Action::RenameTab,
        keys: &[KeyBinding::plain(KeyCode::Char('r'))],
        bar: None,
        help: "Renames the active tab",
    },
    Entry {
        name: "tab.previous",
        action: Action::ToggleTab(ToggleDirection::Previous),
        keys: &[KeyBinding::plain(KeyCode::Char('['))],
        bar: None,
        help: "Switches to the previous tab, wrapping to the last one",
    },
    Entry {
        name: "tab.next",
        action: Action::ToggleTab(ToggleDirection::Next),
        keys: &[KeyBinding::plain(KeyCode::Char(']'))],
        bar: None,
        help: "Switches to the next tab, wrapping to the first one",
    },
    Entry {
        name: "app.quit",
        action: Action::Quit,
        keys: &[KeyBinding::plain(KeyCode::Char('q'))],
        bar: Some("Quit"),
        help: "Leaves Mula",
    },
];

#[cfg(test)]
mod keys_tests {
    use super::*;
    use crate::ui::{dialog::Dialog, filter::Filter, finder::Finder, help, prompt::Prompt};

    /// The Browse table as it stands with no config to override it.
    fn defaults() -> Vec<Binding<Action>> {
        table(BROWSE_ACTIONS, |_| None)
    }

    #[test]
    fn browse_keys_bind_every_key_once() {
        validate(&defaults()).unwrap();
    }

    /// A config names an action, so two entries answering to one name would
    /// make one of them unreachable.
    #[test]
    fn every_action_is_named_once() {
        let mut names: Vec<&str> = BROWSE_ACTIONS.iter().map(|e| e.name).collect();
        names.sort_unstable();
        let mut unique = names.clone();
        unique.dedup();

        assert_eq!(names, unique, "an action name is used twice");
    }

    /// A name is only useful to someone who can find out it exists, and the
    /// running app never prints one — F1 lists keys. The README is where they
    /// are written down, so it is measured against the catalogue rather than
    /// against a list kept by hand.
    #[test]
    fn the_readme_names_every_action() {
        let readme = include_str!("../README.md");

        for entry in BROWSE_ACTIONS {
            assert!(
                readme.contains(&format!("`{}`", entry.name)),
                "{} is in no README table",
                entry.name
            );
        }
    }

    /// An action reachable two ways is still one entry in the bar, or the bar
    /// would draw `F5 Copy  y Copy`.
    #[test]
    fn only_the_first_key_of_an_action_carries_the_bar_label() {
        let two = [
            KeyBinding::plain(KeyCode::F(5)),
            KeyBinding::plain(KeyCode::Char('y')),
        ];
        let built = table(BROWSE_ACTIONS, |name| {
            (name == "transfer.copy").then_some(&two[..])
        });

        let copy: Vec<Option<&str>> = built
            .iter()
            .filter(|b| {
                matches!(
                    b.msg,
                    Action::Transfer {
                        op: TransferOp::Copy
                    }
                )
            })
            .map(|b| b.bar)
            .collect();

        assert_eq!(copy, [Some("Copy"), None]);
    }

    #[test]
    fn a_bound_action_answers_to_the_given_keys_and_no_longer_to_its_own() {
        let rebound = [KeyBinding::plain(KeyCode::Char('y'))];
        let built = table(BROWSE_ACTIONS, |name| {
            (name == "transfer.copy").then_some(&rebound[..])
        });

        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert!(matches!(
            resolve(&built, &press(KeyCode::Char('y'))),
            Some(Action::Transfer {
                op: TransferOp::Copy
            })
        ));
        assert!(resolve(&built, &press(KeyCode::F(5))).is_none());
    }

    #[test]
    fn validate_names_the_key_that_is_bound_twice() {
        let table = [
            Binding {
                key: KeyBinding::plain(KeyCode::F(8)),
                msg: Action::Delete(DeleteMode::Trash),
                bar: None,
                help: "first",
            },
            Binding {
                key: KeyBinding::plain(KeyCode::F(8)),
                msg: Action::Quit,
                bar: None,
                help: "second, unreachable",
            },
        ];

        let error = validate(&table).unwrap_err();
        assert!(
            matches!(error, BindingError::Duplicate { key } if key == KeyBinding::plain(KeyCode::F(8))),
            "the error was {error}"
        );
    }

    /// The bar, the help and the config file all spell a key the same way, so
    /// every key in the tables has to survive being written out and read back.
    /// A key that does not is one a user cannot name in their config.
    #[test]
    fn every_bound_key_round_trips_through_its_name() {
        fn round_trip<T>(bindings: &[Binding<T>]) {
            for binding in bindings {
                let name = binding.key.to_string();
                let parsed: KeyBinding = name.parse().unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(parsed, binding.key, "{name} read back as {parsed}");
            }
        }

        round_trip(&defaults());
        round_trip(GLOBAL_KEYS);
        round_trip(Dialog::DIALOG_KEYS);
        round_trip(Prompt::PROMPT_KEYS);
        round_trip(Finder::FIND_KEYS);
        round_trip(Filter::FILTER_KEYS);
        round_trip(help::HELP_KEYS);
    }

    #[test]
    fn a_capital_letter_carries_shift_whether_the_name_says_so_or_not() {
        let shifted = KeyBinding::plain(KeyCode::Char('G')).shift();

        assert_eq!("G".parse::<KeyBinding>().unwrap(), shifted);
        assert_eq!("Shift+G".parse::<KeyBinding>().unwrap(), shifted);
    }

    /// `+` is a key of its own, so the modifiers are stripped one prefix at a
    /// time rather than by splitting the whole name.
    #[test]
    fn plus_is_a_key_and_not_only_a_separator() {
        assert_eq!(
            "Ctrl++".parse::<KeyBinding>().unwrap(),
            KeyBinding::ctrl(KeyCode::Char('+'))
        );
        assert_eq!(
            "+".parse::<KeyBinding>().unwrap(),
            KeyBinding::plain(KeyCode::Char('+'))
        );
    }

    #[test]
    fn a_key_name_ignores_case_and_inner_spaces() {
        let expected = KeyBinding::ctrl(KeyCode::PageUp);

        for name in ["Ctrl+Page Up", "ctrl+pageup", "CTRL+PAGEUP"] {
            assert_eq!(name.parse::<KeyBinding>().unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn an_unknown_key_names_itself_in_the_error() {
        let error = "Ctrl+Wat".parse::<KeyBinding>().unwrap_err();

        assert!(
            matches!(&error, BindingError::UnknownKey { name } if name == "Ctrl+Wat"),
            "the error was {error}"
        );
    }

    #[test]
    fn the_globals_reach_the_help_overlay() {
        assert!(find(GLOBAL_KEYS, |m| matches!(m, GlobalMsg::ShowHelp)).is_some());
    }

    /// The invariant a global needs and `validate` cannot give: `validate`
    /// compares entries inside one table, and a global is shadowed from
    /// another one. A mode binding the same key would never see it, since the
    /// globals resolve first.
    #[test]
    fn no_mode_binds_a_global_key() {
        fn bound<T>(bindings: &[Binding<T>]) -> Vec<KeyBinding> {
            bindings.iter().map(|b| b.key).collect()
        }

        for (name, keys) in [
            ("browse", bound(&defaults())),
            ("dialog", bound(Dialog::DIALOG_KEYS)),
            ("prompt", bound(Prompt::PROMPT_KEYS)),
            ("finder", bound(Finder::FIND_KEYS)),
            ("filter", bound(Filter::FILTER_KEYS)),
            ("help", bound(help::HELP_KEYS)),
        ] {
            for global in GLOBAL_KEYS {
                assert!(
                    !keys.contains(&global.key),
                    "the {name} table takes {}, which is global",
                    global.key
                );
            }
        }
    }

    #[test]
    fn resolve_takes_the_first_match() {
        let table = [
            Binding {
                key: KeyBinding::plain(KeyCode::Char('x')),
                msg: Action::Quit,
                bar: None,
                help: "first",
            },
            Binding {
                key: KeyBinding::plain(KeyCode::Char('x')),
                msg: Action::ToggleSide,
                bar: None,
                help: "shadowed",
            },
        ];

        let event = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(resolve(&table, &event), Some(Action::Quit)));
    }

    #[test]
    fn modifiers_the_binding_does_not_name_are_ignored() {
        // `matches` masks the event down to CONTROL, ALT and SHIFT; every other
        // bit a terminal sets is dropped before the comparison.
        let binding = KeyBinding::plain(KeyCode::F(5));
        let event = KeyEvent::new(KeyCode::F(5), KeyModifiers::SUPER | KeyModifiers::HYPER);

        assert!(binding.matches(&event));
    }

    #[test]
    fn a_named_modifier_is_required() {
        let binding = KeyBinding::ctrl(KeyCode::Char('t'));

        assert!(binding.matches(&KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)));
        assert!(!binding.matches(&KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)));
    }
}
