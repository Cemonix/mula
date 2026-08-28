use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

use crate::{
    action::{Action, ListEnd, VerticalDir},
    fs::ops::TransferOp,
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
        write!(f, "{}", self.code)
    }
}

#[derive(Error, Debug)]
pub enum BindingError {
    #[error("{key} is bound more than once")]
    Duplicate { key: KeyBinding },
}

pub struct Binding<T> {
    pub key: KeyBinding,
    pub msg: T,
    pub bar: Option<&'static str>,
    pub help: &'static str,
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

// `resolve` takes the first match and the help overlay lists the entries in this
// order.
pub const BROWSE_KEYS: &[Binding<Action>] = &[
    Binding {
        key: KeyBinding::plain(KeyCode::Up),
        msg: Action::MoveCursor(VerticalDir::Up),
        bar: None,
        help: "Moves the cursor one item up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Down),
        msg: Action::MoveCursor(VerticalDir::Down),
        bar: None,
        help: "Moves the cursor one item down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('g')),
        msg: Action::MoveCursorTo(ListEnd::First),
        bar: None,
        help: "Moves the cursor to the first item",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('G')).shift(),
        msg: Action::MoveCursorTo(ListEnd::Last),
        bar: None,
        help: "Moves the cursor to the last item",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Tab),
        msg: Action::ToggleSide,
        bar: None,
        help: "Focuses the other panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Enter),
        msg: Action::OpenSelected,
        bar: None,
        help: "Enters a directory, or opens a file with what belongs to it",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char(' ')),
        msg: Action::ToggleMark,
        bar: None,
        help: "Marks or unmarks the item under the cursor",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Up).shift(),
        msg: Action::MarkAndMove {
            op: MarkOp::Mark,
            nav_dir: VerticalDir::Up,
        },
        bar: None,
        help: "Marks the item under the cursor and moves up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Down).shift(),
        msg: Action::MarkAndMove {
            op: MarkOp::Mark,
            nav_dir: VerticalDir::Down,
        },
        bar: None,
        help: "Marks the item under the cursor and moves down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Up).alt(),
        msg: Action::MarkAndMove {
            op: MarkOp::Unmark,
            nav_dir: VerticalDir::Up,
        },
        bar: None,
        help: "Unmarks the item under the cursor and moves up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Down).alt(),
        msg: Action::MarkAndMove {
            op: MarkOp::Unmark,
            nav_dir: VerticalDir::Down,
        },
        bar: None,
        help: "Unmarks the item under the cursor and moves down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Esc),
        msg: Action::ClearMarks,
        bar: None,
        help: "Unmarks every item in the focused panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(2)),
        msg: Action::Rename,
        bar: Some("Rename"),
        help: "Renames the item under the cursor",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(3)),
        msg: Action::Open(Opener::View),
        bar: Some("View"),
        help: "Opens the file under the cursor in $PAGER",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(4)),
        msg: Action::Open(Opener::Edit),
        bar: Some("Edit"),
        help: "Opens the file under the cursor in $EDITOR",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(5)),
        msg: Action::Transfer {
            op: TransferOp::Copy,
        },
        bar: Some("Copy"),
        help: "Copies marked items into the other panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(6)),
        msg: Action::Transfer {
            op: TransferOp::Move,
        },
        bar: Some("Move"),
        help: "Moves marked items into the other panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(7)),
        msg: Action::CreateEntry,
        bar: Some("New"),
        help: "Creates a file, or a folder if the name ends with /",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(8)),
        msg: Action::Delete,
        bar: Some("Delete"),
        help: "Deletes marked items, asking first",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::F(9)),
        msg: Action::CancelJob,
        bar: None,
        help: "Stops the running operation",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('v')),
        msg: Action::ToggleQuickView,
        bar: None,
        help: "Shows what is under the cursor in the other panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('c')),
        msg: Action::CycleColumns,
        bar: None,
        help: "Cycles the listing between name, size, and size with date",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('/')),
        msg: Action::Find,
        bar: None,
        help: "Searches the tree below this panel for a name",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('t')),
        msg: Action::NewTab,
        bar: None,
        help: "Opens a new tab in the focused panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('w')),
        msg: Action::CloseTab,
        bar: None,
        help: "Closes the active tab, unless it is the only one",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('r')),
        msg: Action::RenameTab,
        bar: None,
        help: "Renames the active tab",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('[')),
        msg: Action::ToggleTab(ToggleDirection::Previous),
        bar: None,
        help: "Switches to the previous tab, wrapping to the last one",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char(']')),
        msg: Action::ToggleTab(ToggleDirection::Next),
        bar: None,
        help: "Switches to the next tab, wrapping to the first one",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char('q')),
        msg: Action::Quit,
        bar: Some("Quit"),
        help: "Leaves Mula",
    },
];

#[cfg(test)]
mod keys_tests {
    use super::*;
    use crate::ui::{dialog::Dialog, finder::Finder, help, prompt::Prompt};

    #[test]
    fn browse_keys_bind_every_key_once() {
        validate(BROWSE_KEYS).unwrap();
    }

    #[test]
    fn validate_names_the_key_that_is_bound_twice() {
        let table = [
            Binding {
                key: KeyBinding::plain(KeyCode::F(8)),
                msg: Action::Delete,
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

        let BindingError::Duplicate { key } = validate(&table).unwrap_err();
        assert_eq!(key, KeyBinding::plain(KeyCode::F(8)));
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
            ("browse", bound(BROWSE_KEYS)),
            ("dialog", bound(Dialog::DIALOG_KEYS)),
            ("prompt", bound(Prompt::PROMPT_KEYS)),
            ("finder", bound(Finder::FIND_KEYS)),
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
