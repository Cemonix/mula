use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;
use toml::Value;

use crate::{
    action::Action,
    keys::{self, BROWSE_ACTIONS, Binding, BindingError, GLOBAL_KEYS, KeyBinding},
};

/// Where the config lives: `$XDG_CONFIG_HOME/mula`, else `~/.config/mula`, on
/// every platform.
///
/// Not the platform's own directory, which is what `log_dir` picks. A log is
/// state the program writes and nobody opens; this is a file the user edits and
/// carries between machines, and `~/.config` is where a terminal program's is
/// looked for even on macOS.
///
/// `None` when there is no home to look under.
fn config_file() -> Option<PathBuf> {
    let dir = match env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(config) => PathBuf::from(config),
        None => PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?).join(".config"),
    };
    Some(dir.join("mula").join("config.toml"))
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{} could not be read: {source}", path.display())]
    Unreadable { path: PathBuf, source: io::Error },
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("keys.browse is not a table")]
    NotATable,
    #[error("keys.browse names {name}, which is not an action")]
    UnknownAction { name: String },
    #[error("keys.browse.{name} is not a key or a list of keys")]
    NotKeys { name: String },
    #[error("keys.browse.{name}: {source}")]
    BadKey { name: String, source: BindingError },
    #[error("keys.browse: {source}")]
    Clash { source: BindingError },
    #[error("keys.browse.{name} takes {key}, which works in every mode and never reaches Browse")]
    ShadowsGlobal { name: String, key: KeyBinding },
}

/// The keys one action is bound to: a key name, or a list of them. An empty
/// list leaves the action reachable by no key at all.
fn keys_of(name: &str, value: &Value) -> Result<Vec<KeyBinding>, ConfigError> {
    let named = match value {
        Value::String(one) => vec![one.as_str()],
        Value::Array(many) => many
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<&str>>>()
            .ok_or_else(|| ConfigError::NotKeys {
                name: name.to_string(),
            })?,
        _ => {
            return Err(ConfigError::NotKeys {
                name: name.to_string(),
            });
        }
    };

    named
        .iter()
        .map(|key| {
            key.parse().map_err(|source| ConfigError::BadKey {
                name: name.to_string(),
                source,
            })
        })
        .collect()
}

/// Walks the table under `[keys.browse]`, joining nested tables with `.`.
///
/// TOML turns `transfer.copy = ["y"]` into a table and `"transfer.copy" =
/// ["y"]` into one key, and a config file should not turn on where the quotes
/// went — flattening makes both name the same action.
fn collect(
    prefix: &str,
    table: &toml::Table,
    bound: &mut BTreeMap<String, Vec<KeyBinding>>,
) -> Result<(), ConfigError> {
    for (key, value) in table {
        let name = match prefix.is_empty() {
            true => key.clone(),
            false => format!("{prefix}.{key}"),
        };
        match value {
            Value::Table(nested) => collect(&name, nested, bound)?,
            value => {
                let keys = keys_of(&name, value)?;
                bound.insert(name, keys);
            }
        }
    }
    Ok(())
}

/// Reads `[keys.browse]` out of a parsed document, checking every name against
/// the catalogue and every key against the globals.
///
/// A global is resolved before the mode's table, so a key that one takes would
/// never reach Browse at all. `validate` cannot see that: it compares entries
/// inside one table.
fn browse_bindings(
    document: &toml::Table,
) -> Result<BTreeMap<String, Vec<KeyBinding>>, ConfigError> {
    let browse = document
        .get("keys")
        .and_then(Value::as_table)
        .and_then(|keys| keys.get("browse"));
    let mut bound = BTreeMap::new();
    match browse {
        None => return Ok(bound),
        Some(Value::Table(table)) => collect("", table, &mut bound)?,
        Some(_) => return Err(ConfigError::NotATable),
    }

    for (name, keys) in &bound {
        if !BROWSE_ACTIONS.iter().any(|entry| entry.name == name) {
            return Err(ConfigError::UnknownAction { name: name.clone() });
        }
        if let Some(key) = keys
            .iter()
            .find(|key| GLOBAL_KEYS.iter().any(|global| global.key == **key))
        {
            return Err(ConfigError::ShadowsGlobal {
                name: name.clone(),
                key: *key,
            });
        }
    }

    Ok(bound)
}

/// Builds the Browse table from `text`, which is the contents of `path`.
fn table_from(text: &str, path: &Path) -> Result<Vec<Binding<Action>>, ConfigError> {
    let document: toml::Table = text.parse().map_err(|source| ConfigError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    let bound = browse_bindings(&document)?;

    let table = keys::table(BROWSE_ACTIONS, |name| bound.get(name).map(Vec::as_slice));
    keys::validate(&table).map_err(|source| ConfigError::Clash { source })?;
    Ok(table)
}

/// The Browse key table the config asks for, or the error that stopped it.
///
/// A missing file and a missing home are not errors: they are the defaults,
/// which is what every caller falls back to anyway.
pub fn browse_table() -> Result<Vec<Binding<Action>>, ConfigError> {
    let defaults = || keys::table(BROWSE_ACTIONS, |_| None);

    let Some(path) = config_file() else {
        return Ok(defaults());
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(defaults()),
        Err(source) => return Err(ConfigError::Unreadable { path, source }),
    };

    table_from(&text, &path)
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::fs::ops::TransferOp;

    fn built(text: &str) -> Result<Vec<Binding<Action>>, ConfigError> {
        table_from(text, Path::new("config.toml"))
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn copies(table: &[Binding<Action>], code: KeyCode) -> bool {
        matches!(
            keys::resolve(table, &press(code)),
            Some(Action::Transfer {
                op: TransferOp::Copy
            })
        )
    }

    const EXAMPLE: &str = include_str!("../config.example.toml");

    /// The example is what a user starts from, so it has to name everything
    /// they could want to move. Equality alone would not catch an action left
    /// out of it: an unmentioned action keeps its default key, which is what
    /// the example claims to be showing.
    #[test]
    fn the_example_config_names_every_action() {
        for entry in BROWSE_ACTIONS {
            assert!(
                EXAMPLE.contains(&format!("\n{} = ", entry.name)),
                "{} is not in config.example.toml",
                entry.name
            );
        }
    }

    /// What the example says the defaults are is what they are. This is also
    /// the one test that runs the parser over the file a user is handed, rather
    /// than over a string written to suit it.
    #[test]
    fn the_example_config_is_the_defaults_written_out() {
        let listed = |table: &[Binding<Action>]| {
            table
                .iter()
                .map(|b| (b.key.to_string(), b.bar, b.help))
                .collect::<Vec<_>>()
        };

        let example = built(EXAMPLE).unwrap();
        let defaults = keys::table(BROWSE_ACTIONS, |_| None);

        assert_eq!(listed(&example), listed(&defaults));
    }

    #[test]
    fn an_empty_config_is_the_default_table() {
        let table = built("").unwrap();

        assert!(copies(&table, KeyCode::F(5)));
    }

    /// The whole point: an action moves to the key the config names and stops
    /// answering to its own.
    #[test]
    fn an_action_answers_to_the_keys_the_config_gives_it() {
        let table = built(
            r#"
            [keys.browse]
            "transfer.copy" = ["y"]
            "#,
        )
        .unwrap();

        assert!(copies(&table, KeyCode::Char('y')));
        assert!(!copies(&table, KeyCode::F(5)));
    }

    /// TOML reads the unquoted form as a nested table. Both spellings have to
    /// mean the same action, or the config turns on where the quotes went.
    #[test]
    fn a_dotted_name_and_a_quoted_one_are_one_action() {
        let quoted = built("[keys.browse]\n\"transfer.copy\" = [\"y\"]").unwrap();
        let dotted = built("[keys.browse]\ntransfer.copy = [\"y\"]").unwrap();
        let sectioned = built("[keys.browse.transfer]\ncopy = [\"y\"]").unwrap();

        for table in [quoted, dotted, sectioned] {
            assert!(copies(&table, KeyCode::Char('y')));
            assert!(!copies(&table, KeyCode::F(5)));
        }
    }

    #[test]
    fn one_key_may_be_given_without_a_list() {
        let table = built("[keys.browse]\n\"transfer.copy\" = \"y\"").unwrap();

        assert!(copies(&table, KeyCode::Char('y')));
    }

    #[test]
    fn an_empty_list_leaves_the_action_unbound() {
        let table = built("[keys.browse]\n\"transfer.copy\" = []").unwrap();

        assert!(!copies(&table, KeyCode::F(5)));
        assert!(
            keys::find(&table, |a| matches!(
                a,
                Action::Transfer {
                    op: TransferOp::Copy
                }
            ))
            .is_none()
        );
    }

    #[test]
    fn an_action_may_be_reachable_more_than_one_way() {
        let table = built("[keys.browse]\n\"transfer.copy\" = [\"F5\", \"y\"]").unwrap();

        assert!(copies(&table, KeyCode::F(5)));
        assert!(copies(&table, KeyCode::Char('y')));
    }

    #[test]
    fn a_misspelled_action_names_itself() {
        let error = built("[keys.browse]\n\"transfer.kopie\" = [\"y\"]").unwrap_err();

        assert!(
            matches!(&error, ConfigError::UnknownAction { name } if name == "transfer.kopie"),
            "the error was {error}"
        );
    }

    #[test]
    fn a_misspelled_key_names_the_action_it_was_given_to() {
        let error = built("[keys.browse]\n\"transfer.copy\" = [\"Ctrl+Wat\"]").unwrap_err();

        assert!(
            matches!(&error, ConfigError::BadKey { name, .. } if name == "transfer.copy"),
            "the error was {error}"
        );
    }

    /// The one thing `validate` is for, now that a second table can name keys.
    #[test]
    fn two_actions_cannot_take_one_key() {
        let error = built("[keys.browse]\n\"transfer.copy\" = [\"d\"]\n\"entry.delete\" = [\"d\"]")
            .unwrap_err();

        assert!(
            matches!(error, ConfigError::Clash { .. }),
            "the error was {error}"
        );
    }

    /// A config key colliding with a default one is a clash too: the default is
    /// still in the table under whatever action kept it.
    #[test]
    fn a_config_key_cannot_take_a_key_a_default_already_holds() {
        let error = built("[keys.browse]\n\"transfer.copy\" = [\"q\"]").unwrap_err();

        assert!(
            matches!(error, ConfigError::Clash { .. }),
            "the error was {error}"
        );
    }

    /// The globals resolve first, so a Browse binding on one would never fire.
    /// `validate` cannot see this: it compares entries inside one table.
    #[test]
    fn a_global_key_cannot_be_taken_by_an_action() {
        let global = GLOBAL_KEYS[0].key.to_string();
        let error = built(&format!(
            "[keys.browse]\n\"transfer.copy\" = [\"{global}\"]"
        ))
        .unwrap_err();

        assert!(
            matches!(&error, ConfigError::ShadowsGlobal { name, .. } if name == "transfer.copy"),
            "the error was {error}"
        );
    }

    #[test]
    fn a_value_that_is_not_a_key_names_the_action() {
        let error = built("[keys.browse]\n\"transfer.copy\" = 5").unwrap_err();

        assert!(
            matches!(&error, ConfigError::NotKeys { name } if name == "transfer.copy"),
            "the error was {error}"
        );
    }

    #[test]
    fn broken_toml_says_which_file() {
        let error = built("[keys.browse").unwrap_err();

        assert!(
            matches!(error, ConfigError::Malformed { .. }),
            "the error was {error}"
        );
    }
}
