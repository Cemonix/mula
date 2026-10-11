use crate::ui::{columns::Columns, icon::Icons, pane::DotFiles, sort::SortKey};

/// One value out of a fixed set, spelled the way the config file writes it.
pub trait Choice: Copy + Eq + 'static {
    /// Every value, in the order the settings overlay steps through them.
    const ALL: &'static [Self];

    fn name(self) -> &'static str;
}

impl Choice for Columns {
    const ALL: &'static [Self] = &[Columns::SizeAndTime, Columns::Size, Columns::Name];

    fn name(self) -> &'static str {
        match self {
            Columns::SizeAndTime => "size-and-time",
            Columns::Size => "size",
            Columns::Name => "name",
        }
    }
}

impl Choice for SortKey {
    const ALL: &'static [Self] = &[
        SortKey::Name,
        SortKey::Modified,
        SortKey::Size,
        SortKey::Extension,
    ];

    fn name(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Modified => "time",
            SortKey::Size => "size",
            SortKey::Extension => "extension",
        }
    }
}

impl Choice for DotFiles {
    const ALL: &'static [Self] = &[DotFiles::Hidden, DotFiles::Shown];

    fn name(self) -> &'static str {
        match self {
            DotFiles::Hidden => "hidden",
            DotFiles::Shown => "shown",
        }
    }
}

impl Choice for Icons {
    const ALL: &'static [Self] = &[Icons::NerdFont, Icons::Emoji, Icons::Off];

    fn name(self) -> &'static str {
        match self {
            Icons::NerdFont => "nerd-font",
            Icons::Emoji => "emoji",
            Icons::Off => "off",
        }
    }
}

/// Which way a setting is stepped through its choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
}

/// The value of `name` among `T`'s choices, or `None` when it is not one.
fn named<T: Choice>(name: &str) -> Option<T> {
    T::ALL.iter().copied().find(|choice| choice.name() == name)
}

/// The choice beside `current`, wrapping at either end.
fn stepped<T: Choice>(current: T, step: Step) -> T {
    let count = T::ALL.len();
    let at = T::ALL
        .iter()
        .position(|choice| *choice == current)
        .expect("every value of a choice is in its ALL");
    let next = match step {
        Step::Previous => (at + count - 1) % count,
        Step::Next => (at + 1) % count,
    };
    T::ALL[next]
}

fn names<T: Choice>() -> Vec<&'static str> {
    T::ALL.iter().map(|choice| choice.name()).collect()
}

/// Everything the config file sets apart from the keys. A value left at its
/// default is the `Default` of its field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub columns: Columns,
    pub sort: SortKey,
    pub dot_files: DotFiles,
    pub icons: Icons,
}

/// A table of the config file, and a heading of the settings overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Listing,
    Appearance,
}

impl Section {
    /// Every section, in the order the file and the overlay list them.
    pub const ALL: &[Section] = &[Section::Listing, Section::Appearance];

    /// The table the section is written under.
    pub fn name(self) -> &'static str {
        match self {
            Section::Listing => "listing",
            Section::Appearance => "appearance",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Section::Listing => "Listing",
            Section::Appearance => "Appearance",
        }
    }
}

/// One field of [`Settings`], and everything the file and the overlay say
/// about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Columns,
    Sort,
    DotFiles,
    Icons,
}

impl Setting {
    /// Every setting, in the order the file and the overlay list them.
    pub const ALL: &[Setting] = &[
        Setting::Columns,
        Setting::Sort,
        Setting::DotFiles,
        Setting::Icons,
    ];

    pub fn section(self) -> Section {
        match self {
            Setting::Columns | Setting::Sort | Setting::DotFiles => Section::Listing,
            Setting::Icons => Section::Appearance,
        }
    }

    /// The key the setting is written under, inside its section's table.
    pub fn name(self) -> &'static str {
        match self {
            Setting::Columns => "columns",
            Setting::Sort => "sort",
            Setting::DotFiles => "dotfiles",
            Setting::Icons => "icons",
        }
    }

    /// The setting's name in the overlay.
    pub fn title(self) -> &'static str {
        match self {
            Setting::Columns => "Columns",
            Setting::Sort => "Sort by",
            Setting::DotFiles => "Dotfiles",
            Setting::Icons => "Icons",
        }
    }

    /// One line saying what the setting decides, written above it in the file
    /// and under it in the overlay.
    pub fn help(self) -> &'static str {
        match self {
            Setting::Columns => "What each row shows besides its name",
            Setting::Sort => "What a panel is sorted by when it opens",
            Setting::DotFiles => "Whether a panel lists names beginning with a dot when it opens",
            Setting::Icons => "The glyph in front of each name; nerd-font takes a Nerd Font",
        }
    }

    /// Every value the setting can take, by name.
    pub fn choices(self) -> Vec<&'static str> {
        match self {
            Setting::Columns => names::<Columns>(),
            Setting::Sort => names::<SortKey>(),
            Setting::DotFiles => names::<DotFiles>(),
            Setting::Icons => names::<Icons>(),
        }
    }

    /// The name of the value `settings` holds for this setting.
    pub fn value(self, settings: &Settings) -> &'static str {
        match self {
            Setting::Columns => settings.columns.name(),
            Setting::Sort => settings.sort.name(),
            Setting::DotFiles => settings.dot_files.name(),
            Setting::Icons => settings.icons.name(),
        }
    }

    /// Sets this setting to the value called `name`, and answers whether it
    /// was one of its choices. `settings` is left alone when it was not.
    pub fn select(self, settings: &mut Settings, name: &str) -> bool {
        fn put<T: Choice>(field: &mut T, name: &str) -> bool {
            named(name).map(|value| *field = value).is_some()
        }

        match self {
            Setting::Columns => put(&mut settings.columns, name),
            Setting::Sort => put(&mut settings.sort, name),
            Setting::DotFiles => put(&mut settings.dot_files, name),
            Setting::Icons => put(&mut settings.icons, name),
        }
    }

    /// Puts this setting back to its default.
    pub fn reset(self, settings: &mut Settings) {
        let defaults = Settings::default();
        match self {
            Setting::Columns => settings.columns = defaults.columns,
            Setting::Sort => settings.sort = defaults.sort,
            Setting::DotFiles => settings.dot_files = defaults.dot_files,
            Setting::Icons => settings.icons = defaults.icons,
        }
    }

    /// Moves this setting one choice along.
    pub fn step(self, settings: &mut Settings, step: Step) {
        match self {
            Setting::Columns => settings.columns = stepped(settings.columns, step),
            Setting::Sort => settings.sort = stepped(settings.sort, step),
            Setting::DotFiles => settings.dot_files = stepped(settings.dot_files, step),
            Setting::Icons => settings.icons = stepped(settings.icons, step),
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    /// A value the file can write has to be one it can read back, or a setting
    /// changed in the overlay would close the file at the next start.
    #[test]
    fn every_value_of_every_setting_reads_back_as_itself() {
        for setting in Setting::ALL {
            for name in setting.choices() {
                let mut settings = Settings::default();

                assert!(setting.select(&mut settings, name), "{name}");
                assert_eq!(setting.value(&settings), name);
            }
        }
    }

    #[test]
    fn no_setting_offers_one_name_twice() {
        for setting in Setting::ALL {
            let mut choices = setting.choices();
            choices.sort_unstable();
            choices.dedup();

            assert_eq!(choices.len(), setting.choices().len(), "{setting:?}");
        }
    }

    #[test]
    fn a_name_that_is_not_a_choice_changes_nothing() {
        let mut settings = Settings::default();

        assert!(!Setting::Sort.select(&mut settings, "colour"));
        assert_eq!(settings, Settings::default());
    }

    /// Stepping visits every choice and comes back to where it started, in
    /// both directions.
    #[test]
    fn stepping_goes_round_every_choice() {
        for setting in Setting::ALL {
            for step in [Step::Next, Step::Previous] {
                let mut settings = Settings::default();
                let mut seen = Vec::new();
                for _ in setting.choices() {
                    seen.push(setting.value(&settings));
                    setting.step(&mut settings, step);
                }

                assert_eq!(settings, Settings::default(), "{setting:?}");
                seen.sort_unstable();
                let mut choices = setting.choices();
                choices.sort_unstable();
                assert_eq!(seen, choices, "{setting:?}");
            }
        }
    }

    /// A setting missing from `ALL` would be one the file, the parser and the
    /// overlay all leave out. The match is what makes a new variant come
    /// through here.
    #[test]
    fn every_setting_is_listed() {
        let listed = |setting: Setting| match setting {
            Setting::Columns | Setting::Sort | Setting::DotFiles | Setting::Icons => {
                Setting::ALL.contains(&setting)
            }
        };

        assert!(listed(Setting::Columns));
        assert!(listed(Setting::Sort));
        assert!(listed(Setting::DotFiles));
        assert!(listed(Setting::Icons));
    }

    #[test]
    fn every_section_a_setting_is_in_is_listed() {
        for setting in Setting::ALL {
            assert!(Section::ALL.contains(&setting.section()), "{setting:?}");
        }
    }

    /// Resetting one setting leaves every other where it was.
    #[test]
    fn a_reset_puts_back_that_setting_alone() {
        for setting in Setting::ALL {
            let mut moved = Settings::default();
            for other in Setting::ALL {
                other.step(&mut moved, Step::Next);
            }
            let mut expected = moved;
            setting.select(&mut expected, setting.value(&Settings::default()));

            setting.reset(&mut moved);

            assert_eq!(moved, expected, "{setting:?}");
        }
    }

    /// Two settings sharing a name in one section would be one line of the
    /// file read into both.
    #[test]
    fn no_two_settings_of_a_section_share_a_name() {
        for (i, a) in Setting::ALL.iter().enumerate() {
            for b in &Setting::ALL[i + 1..] {
                assert!(
                    a.section() != b.section() || a.name() != b.name(),
                    "{a:?} and {b:?}"
                );
            }
        }
    }
}
