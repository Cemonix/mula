use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    process,
};

use thiserror::Error;
use toml::Value;

use crate::{
    action::Action,
    keys::{self, BROWSE_ACTIONS, Binding, BindingError, GLOBAL_KEYS, KeyBinding},
    settings::{Section, Setting, Settings},
};

/// Where the config lives: `$XDG_CONFIG_HOME/mula`, else `~/.config/mula`, on
/// every platform.
///
/// `None` when there is no home to look under.
fn config_file() -> Option<PathBuf> {
    let dir = match env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(config) => PathBuf::from(config),
        None => PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?).join(".config"),
    };
    Some(dir.join("mula").join("config.toml"))
}

/// Symbolic links followed from the config file before giving up on it, which
/// is what the kernel allows in a path on Linux.
const MAX_LINKS: usize = 40;

/// Written at the top of the file.
const HEADER: &str = "\
# Mula's settings. Mula writes this file itself, whole, every time a setting
# changes in the settings overlay: the values here are kept, anything else
# written by hand is not.
#
# A commented line is the default. A line under it is what replaces it, and
# taking that line out puts the default back.
";

/// Written under `[keys.browse]`.
const KEYS_HEADER: &str = "\
# Each action takes one key, a list of keys, or [] to leave it unreachable.
# Key names are the ones F1 prints. Ctrl+, Alt+ and Shift+ go in front, in that
# order; a capital letter carries Shift whether the name says so or not.
#
# Two actions cannot share a key, and none may take F1. A file that breaks
# either rule is not used at all: Mula starts on its defaults and says why.
";

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{} could not be read: {source}", path.display())]
    Unreadable { path: PathBuf, source: io::Error },
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{name} is not a table")]
    NotATable { name: String },
    #[error("{name} is not a setting Mula knows")]
    UnknownSetting { name: String },
    #[error("{name} takes one of: {choices}")]
    BadChoice { name: String, choices: String },
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
    #[error("{} could not be written: {source}", path.display())]
    Unwritable { path: PathBuf, source: io::Error },
    #[error("there is no home directory to keep the settings in")]
    NoHome,
    #[error("the config file could not be read, so it is left as it is until Mula restarts")]
    ReadOnly,
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
/// ["y"]` into one key; flattening makes both name the same action.
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

/// Reads the table under `[keys.browse]`, checking every name against the
/// catalogue and every key against the globals.
///
/// A global is resolved before the mode's table, so a key that one takes would
/// never reach Browse at all. `validate` cannot see that: it compares entries
/// inside one table.
fn browse_bindings(table: &toml::Table) -> Result<BTreeMap<String, Vec<KeyBinding>>, ConfigError> {
    let mut bound = BTreeMap::new();
    collect("", table, &mut bound)?;

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

/// `value` as a table, or the error naming it as something else.
fn table_at<'v>(name: &str, value: &'v Value) -> Result<&'v toml::Table, ConfigError> {
    value.as_table().ok_or_else(|| ConfigError::NotATable {
        name: name.to_string(),
    })
}

/// Reads the settings of `section` out of its table onto `settings`.
fn read_section(
    section: Section,
    table: &toml::Table,
    settings: &mut Settings,
) -> Result<(), ConfigError> {
    for (key, value) in table {
        let name = format!("{}.{key}", section.name());
        let setting = Setting::ALL
            .iter()
            .find(|setting| setting.section() == section && setting.name() == key)
            .ok_or_else(|| ConfigError::UnknownSetting { name: name.clone() })?;

        let chosen = value
            .as_str()
            .is_some_and(|value| setting.select(settings, value));
        if !chosen {
            return Err(ConfigError::BadChoice {
                name,
                choices: setting.choices().join(", "),
            });
        }
    }
    Ok(())
}

/// What a config file says, read whole: the settings, and the actions it gives
/// keys of their own.
#[derive(Debug, Default, PartialEq)]
struct Contents {
    settings: Settings,
    keys: BTreeMap<String, Vec<KeyBinding>>,
}

/// Reads `text`, the contents of `path`. Anything in it Mula does not know is
/// an error: it would not survive the next write.
fn parse(text: &str, path: &Path) -> Result<Contents, ConfigError> {
    let document: toml::Table = text.parse().map_err(|source| ConfigError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;

    let mut contents = Contents::default();
    for (key, value) in &document {
        if key == "keys" {
            for (mode, value) in table_at(key, value)? {
                let name = format!("keys.{mode}");
                match mode.as_str() {
                    "browse" => contents.keys = browse_bindings(table_at(&name, value)?)?,
                    _ => return Err(ConfigError::UnknownSetting { name }),
                }
            }
            continue;
        }

        let section = Section::ALL
            .iter()
            .find(|section| section.name() == key)
            .ok_or_else(|| ConfigError::UnknownSetting { name: key.clone() })?;
        read_section(*section, table_at(key, value)?, &mut contents.settings)?;
    }

    let table = browse_table(&contents.keys);
    keys::validate(&table).map_err(|source| ConfigError::Clash { source })?;
    Ok(contents)
}

/// The Browse table with the keys `bound` gives the actions it names, and the
/// catalogue's own for the rest.
fn browse_table(bound: &BTreeMap<String, Vec<KeyBinding>>) -> Vec<Binding<Action>> {
    keys::table(BROWSE_ACTIONS, |name| bound.get(name).map(Vec::as_slice))
}

/// `keys` the way the file writes them: one key as a string, any other number
/// as a list.
fn keys_value(keys: &[KeyBinding]) -> Value {
    match keys {
        [one] => Value::String(one.to_string()),
        many => Value::Array(
            many.iter()
                .map(|key| Value::String(key.to_string()))
                .collect(),
        ),
    }
}

/// Writes one entry: `help` as a comment, the default commented out under it,
/// and `value` under that when it is not the default.
fn entry(out: &mut String, help: &str, name: &str, default: &Value, value: &Value) {
    let _ = writeln!(out, "\n# {help}\n# {name} = {default}");
    if value != default {
        let _ = writeln!(out, "{name} = {value}");
    }
}

/// The whole file for `contents`: every setting and every action, each under a
/// comment saying what it does and a commented line holding its default.
fn render(contents: &Contents) -> String {
    let defaults = Settings::default();
    let mut out = String::from(HEADER);

    for section in Section::ALL {
        let _ = write!(out, "\n[{}]\n", section.name());
        for setting in Setting::ALL.iter().filter(|s| s.section() == *section) {
            let help = format!("{}: {}", setting.help(), setting.choices().join(", "));
            entry(
                &mut out,
                &help,
                setting.name(),
                &Value::String(setting.value(&defaults).to_string()),
                &Value::String(setting.value(&contents.settings).to_string()),
            );
        }
    }

    let _ = write!(out, "\n[keys.browse]\n\n{KEYS_HEADER}");
    for action in BROWSE_ACTIONS {
        let default = keys_value(action.keys);
        let value = contents
            .keys
            .get(action.name)
            .map_or_else(|| default.clone(), |keys| keys_value(keys));
        entry(&mut out, action.help, action.name, &default, &value);
    }
    out
}

/// The path a write to `path` has to land on: the file at the end of whatever
/// symbolic links it is, or would be once created.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&path)?;
                path = match path.parent() {
                    Some(dir) => dir.join(target),
                    None => target,
                };
            }
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from_raw_os_error(libc::ELOOP))
}

/// Replaces the file `path` resolves to with `text`, all of it or none of it.
///
/// The text goes into a file beside the target, which takes the target's
/// permissions and is renamed over it once on disk. The directories above it
/// are created; a symbolic link on the way stays a link.
fn write_whole(path: &Path, text: &str) -> Result<(), ConfigError> {
    let unwritable = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ConfigError::Unwritable { path, source }
    };

    let target = resolve(path).map_err(unwritable(path))?;
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(ConfigError::Unwritable {
            path: target.clone(),
            source: io::Error::from(io::ErrorKind::InvalidInput),
        });
    };
    fs::create_dir_all(dir).map_err(unwritable(dir))?;

    let temp = dir.join(format!(".{}.{}.tmp", name.to_string_lossy(), process::id()));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)?;
        if let Ok(meta) = fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, &target)
    })();

    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.map_err(unwritable(&target))
}

/// Why a config is written nowhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Closed {
    /// There is no home to put the file in.
    NoHome,
    /// The file is there and could not be used. Writing it would replace what
    /// the user has with the defaults.
    Unread,
}

/// The settings and keys Mula runs with, and the file they are kept in.
///
/// The two agree: every change is written out at once, and a write that fails
/// leaves the settings as they were.
#[derive(Debug)]
pub struct Config {
    contents: Contents,
    file: Result<PathBuf, Closed>,
}

impl Config {
    /// The config as it stands on disk, and what went wrong reading it.
    ///
    /// A file that is not there is the defaults, and so is a missing home;
    /// neither is a complaint.
    pub fn load() -> (Self, Option<ConfigError>) {
        match config_file() {
            Some(file) => Self::read(file),
            None => (
                Self {
                    contents: Contents::default(),
                    file: Err(Closed::NoHome),
                },
                None,
            ),
        }
    }

    /// The config `file` holds, written back to `file`, or the defaults
    /// written nowhere when it cannot be used.
    fn read(file: PathBuf) -> (Self, Option<ConfigError>) {
        let parsed = match fs::read_to_string(&file) {
            Ok(text) => parse(&text, &file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Contents::default()),
            Err(source) => Err(ConfigError::Unreadable {
                path: file.clone(),
                source,
            }),
        };

        match parsed {
            Ok(contents) => (
                Self {
                    contents,
                    file: Ok(file),
                },
                None,
            ),
            Err(e) => (
                Self {
                    contents: Contents::default(),
                    file: Err(Closed::Unread),
                },
                Some(e),
            ),
        }
    }

    pub fn settings(&self) -> Settings {
        self.contents.settings
    }

    /// The Browse key table the config asks for.
    pub fn browse_table(&self) -> Vec<Binding<Action>> {
        browse_table(&self.contents.keys)
    }

    /// Takes `settings` and writes the file out. When the write fails the
    /// settings stay as they were.
    pub fn set(&mut self, settings: Settings) -> Result<(), ConfigError> {
        let previous = self.contents.settings;
        self.contents.settings = settings;
        if let Err(e) = self.save() {
            self.contents.settings = previous;
            return Err(e);
        }
        Ok(())
    }

    fn save(&self) -> Result<(), ConfigError> {
        match &self.file {
            Ok(file) => write_whole(file, &render(&self.contents)),
            Err(Closed::NoHome) => Err(ConfigError::NoHome),
            Err(Closed::Unread) => Err(ConfigError::ReadOnly),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::{
        fs::{ops::TransferOp, temp_tree::TempTree},
        ui::sort::SortKey,
    };

    fn built(text: &str) -> Result<Vec<Binding<Action>>, ConfigError> {
        parse(text, Path::new("config.toml")).map(|contents| browse_table(&contents.keys))
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

    /// The key copying answers to when no config has moved it, read out of the
    /// catalogue rather than written down here: a test about the config file
    /// has nothing to say about which key that is.
    fn default_copy_key() -> KeyCode {
        BROWSE_ACTIONS
            .iter()
            .find(|entry| entry.name == "transfer.copy")
            .and_then(|entry| entry.keys.first())
            .expect("the catalogue gives copying a key")
            .code
    }

    /// A setting and a key moved off their defaults, which every test of the
    /// written file wants something of.
    fn changed() -> Contents {
        let mut contents = Contents::default();
        contents.settings.sort = SortKey::Size;
        contents.keys.insert(
            "transfer.copy".to_string(),
            vec![KeyBinding::plain(KeyCode::Char('y'))],
        );
        contents
    }

    /// A config kept in `tree`, holding `contents`.
    fn config_in(tree: &TempTree, contents: Contents) -> Config {
        Config {
            contents,
            file: Ok(tree.at("mula/config.toml")),
        }
    }

    #[test]
    fn an_empty_config_is_the_default_table() {
        let table = built("").unwrap();

        assert!(copies(&table, default_copy_key()));
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
        assert!(!copies(&table, default_copy_key()));
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
            assert!(!copies(&table, default_copy_key()));
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

        assert!(!copies(&table, default_copy_key()));
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

    #[test]
    fn a_setting_takes_the_value_the_file_gives_it() {
        let contents = parse("[listing]\nsort = \"size\"", Path::new("config.toml")).unwrap();

        assert_eq!(contents.settings.sort, SortKey::Size);
    }

    #[test]
    fn a_value_a_setting_does_not_offer_lists_the_ones_it_does() {
        let error = parse("[listing]\nsort = \"colour\"", Path::new("config.toml")).unwrap_err();

        assert!(
            matches!(&error, ConfigError::BadChoice { name, choices }
                if name == "listing.sort" && choices.contains("size")),
            "the error was {error}"
        );
    }

    /// Mula rewrites the file whole, so what it cannot read it would drop at
    /// the next change. Refusing it keeps the file closed instead.
    #[test]
    fn anything_mula_does_not_know_is_refused() {
        for (text, unknown) in [
            ("[listing]\ncolour = \"red\"", "listing.colour"),
            ("[theme]\nname = \"dark\"", "theme"),
            ("[keys.finder]\nclose = \"q\"", "keys.finder"),
        ] {
            let error = parse(text, Path::new("config.toml")).unwrap_err();

            assert!(
                matches!(&error, ConfigError::UnknownSetting { name } if name == unknown),
                "{text}: the error was {error}"
            );
        }
    }

    #[test]
    fn a_section_that_is_not_a_table_names_itself() {
        let error = parse("listing = 3", Path::new("config.toml")).unwrap_err();

        assert!(
            matches!(&error, ConfigError::NotATable { name } if name == "listing"),
            "the error was {error}"
        );
    }

    /// What Mula writes is what it reads back, with every setting and key
    /// moved off its default kept.
    #[test]
    fn the_written_file_reads_back_as_what_was_written() {
        let contents = changed();

        let read = parse(&render(&contents), Path::new("config.toml")).unwrap();

        assert_eq!(read, contents);
    }

    /// The defaults written out read back as nothing set at all: every line of
    /// them is a comment.
    #[test]
    fn a_file_of_defaults_sets_nothing() {
        let text = render(&Contents::default());

        assert_eq!(
            parse(&text, Path::new("config.toml")).unwrap(),
            Contents::default()
        );
        assert!(
            text.lines()
                .all(|line| line.is_empty() || line.starts_with('#') || line.starts_with('[')),
            "{text}"
        );
    }

    /// Everything a user could set is in the file, at its default, so the file
    /// is the list of what there is.
    #[test]
    fn the_written_file_names_every_setting_and_every_action() {
        let text = render(&Contents::default());

        for setting in Setting::ALL {
            assert!(
                text.contains(&format!("\n# {} = ", setting.name())),
                "{setting:?}"
            );
        }
        for action in BROWSE_ACTIONS {
            assert!(
                text.contains(&format!("\n# {} = ", action.name)),
                "{}",
                action.name
            );
        }
    }

    /// Every key of the catalogue survives being written and read back,
    /// including the ones TOML has to escape.
    #[test]
    fn every_default_key_reads_back_from_the_file() {
        let mut text = String::from("[keys.browse]\n");
        for action in BROWSE_ACTIONS {
            text.push_str(&format!("{} = {}\n", action.name, keys_value(action.keys)));
        }

        let read = parse(&text, Path::new("config.toml")).unwrap();

        for action in BROWSE_ACTIONS {
            assert_eq!(read.keys[action.name], action.keys, "{}", action.name);
        }
    }

    #[test]
    fn a_change_is_written_out() {
        let tree = TempTree::new();
        let mut config = config_in(&tree, Contents::default());
        let mut settings = config.settings();
        settings.sort = SortKey::Size;

        config.set(settings).unwrap();

        let (read, complaint) = Config::read(tree.at("mula/config.toml"));
        assert!(complaint.is_none());
        assert_eq!(read.settings().sort, SortKey::Size);
    }

    /// A rename over the link would replace it with a file and quietly cut it
    /// off from wherever it pointed, a dotfiles repository most likely.
    #[cfg(unix)]
    #[test]
    fn a_write_goes_through_a_symlink_and_leaves_it_one() {
        let tree = TempTree::new();
        let real = tree.make_file("dotfiles/mula.toml", "");
        tree.make_dir("mula");
        std::os::unix::fs::symlink("../dotfiles/mula.toml", tree.at("mula/config.toml")).unwrap();

        config_in(&tree, Contents::default())
            .set(changed().settings)
            .unwrap();

        assert!(
            fs::symlink_metadata(tree.at("mula/config.toml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(real)
                .unwrap()
                .contains("\nsort = \"size\"")
        );
    }

    /// A link to a file the dotfiles do not hold yet is followed too, and the
    /// file is created where it points.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_is_written_through() {
        let tree = TempTree::new();
        tree.make_dir("dotfiles");
        tree.make_dir("mula");
        std::os::unix::fs::symlink(tree.at("dotfiles/mula.toml"), tree.at("mula/config.toml"))
            .unwrap();

        config_in(&tree, Contents::default())
            .set(changed().settings)
            .unwrap();

        assert!(
            fs::symlink_metadata(tree.at("mula/config.toml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(tree.at("dotfiles/mula.toml").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_write_keeps_the_permissions_the_file_had() {
        use std::os::unix::fs::PermissionsExt;

        let tree = TempTree::new();
        let file = tree.make_file("mula/config.toml", "");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

        config_in(&tree, Contents::default())
            .set(changed().settings)
            .unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// A write that fails leaves nothing on screen claiming it happened, and
    /// no half-written file beside the real one.
    #[test]
    fn a_failed_write_keeps_the_settings_it_had() {
        let tree = TempTree::new();
        tree.make_file("mula", "a file where the directory should be");
        let mut config = config_in(&tree, Contents::default());

        assert!(config.set(changed().settings).is_err());
        assert_eq!(config.settings(), Settings::default());
    }

    #[test]
    fn a_file_that_cannot_be_used_is_never_written() {
        let tree = TempTree::new();
        let file = tree.make_file("mula/config.toml", "[listing\n");

        let (mut config, complaint) = Config::read(file.clone());

        assert!(matches!(complaint, Some(ConfigError::Malformed { .. })));
        assert!(matches!(
            config.set(changed().settings),
            Err(ConfigError::ReadOnly)
        ));
        assert_eq!(config.settings(), Settings::default());
        assert_eq!(fs::read_to_string(file).unwrap(), "[listing\n");
    }

    #[test]
    fn a_file_that_is_not_there_is_the_defaults_and_is_written_later() {
        let tree = TempTree::new();

        let (mut config, complaint) = Config::read(tree.at("mula/config.toml"));

        assert!(complaint.is_none());
        assert_eq!(config.settings(), Settings::default());
        config.set(changed().settings).unwrap();
        assert!(tree.at("mula/config.toml").is_file());
    }
}
