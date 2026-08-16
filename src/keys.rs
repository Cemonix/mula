use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

use crate::{
    action::{Action, NavDirection, Side, ToggleDirection},
    ops::TransferOp,
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

// TODO: It would be good idea to implement shortcut handler that would read shortcuts from file,
// user should be able to change shortcuts if they want, so save new etc...
//
// `resolve` takes the first match and the help overlay lists the entries in this
// order. `ShowHelp` carries no `bar` label; the keybar draws the help key itself.
pub const BROWSE_KEYS: &[Binding<Action>] = &[
    Binding {
        key: KeyBinding::plain(KeyCode::Up),
        msg: Action::MoveCursor(NavDirection::Up),
        bar: None,
        help: "Moves the cursor one item up",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Down),
        msg: Action::MoveCursor(NavDirection::Down),
        bar: None,
        help: "Moves the cursor one item down",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Left),
        msg: Action::FocusSide(Side::Left),
        bar: None,
        help: "Focuses the left panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Right),
        msg: Action::FocusSide(Side::Right),
        bar: None,
        help: "Focuses the right panel",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Enter),
        msg: Action::OpenSelected,
        bar: None,
        help: "Enters the directory under the cursor",
    },
    Binding {
        key: KeyBinding::plain(KeyCode::Char(' ')),
        msg: Action::ToggleMark,
        bar: Some("Mark"),
        help: "Marks or unmarks the item under the cursor",
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
        key: KeyBinding::plain(KeyCode::F(8)),
        msg: Action::Delete,
        bar: Some("Delete"),
        help: "Deletes marked items, asking first",
    },
    Binding {
        key: KeyBinding::ctrl(KeyCode::Char('t')),
        msg: Action::NewTab,
        bar: None,
        help: "Opens a new tab in the focused panel",
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
        key: KeyBinding::plain(KeyCode::Char('?')),
        msg: Action::ShowHelp,
        bar: None,
        help: "Lists every key available right now",
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
    fn browse_keys_reach_the_help_overlay() {
        assert!(find(BROWSE_KEYS, |a| matches!(a, Action::ShowHelp)).is_some());
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
                msg: Action::ShowHelp,
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
