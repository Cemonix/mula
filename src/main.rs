use std::{env, io, panic, path::PathBuf, process::ExitCode};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    app::{App, AppError},
    ui::graphics::{
        capabilities::{Capabilities, Protocol},
        surface,
    },
};

mod action;
mod app;
mod config;
mod fs;
mod keys;
mod open;
mod ui;

const USAGE: &str = "\
mula — a dual-pane terminal file manager

Usage:
    mula [DIRECTORY]

Both panels open in DIRECTORY, or in the directory mula was started from when
none is given.

Options:
    -h, --help       Print this and exit
    -V, --version    Print the version and exit

Press F1 while it runs for the keys that work where you are standing.
";

/// What the command line asked for, when it did not ask for the app to run.
enum Answered {
    /// A question the command line answers on its own: `--help`, `--version`.
    Question(String),
    /// Nothing that can be run: an unknown option, or a directory that cannot
    /// be entered.
    Refusal(String),
}

/// Reads the command line and leaves the process in the directory the panels
/// should open in.
///
/// A directory argument is spent here rather than carried any further: the
/// listings grow from `env::current_dir()`, so `mula /tmp` and `cd /tmp &&
/// mula` are the same run, and the paths handed to other programs stay
/// absolute without anything else having to know an argument was given.
fn read_command_line(args: impl Iterator<Item = String>) -> Result<(), Answered> {
    let mut directory: Option<PathBuf> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Err(Answered::Question(USAGE.to_string())),
            "-V" | "--version" => {
                let version = format!("{} {}\n", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return Err(Answered::Question(version));
            }
            _ if arg.starts_with('-') => {
                return Err(Answered::Refusal(format!(
                    "unknown option: {arg}\nTry `mula --help`."
                )));
            }
            _ if directory.is_some() => {
                return Err(Answered::Refusal(
                    "mula opens one directory, and the other panel follows you.".to_string(),
                ));
            }
            _ => directory = Some(PathBuf::from(arg)),
        }
    }

    let Some(directory) = directory else {
        return Ok(());
    };

    // `set_current_dir` is the whole check: a path that is not a directory
    // comes back as `Not a directory`, which is the complaint we would have
    // written ourselves.
    env::set_current_dir(&directory)
        .map_err(|e| Answered::Refusal(format!("{}: {e}", directory.display())))
}

/// Where a log goes: `$XDG_STATE_HOME/mula`, or the directory the platform
/// keeps state in. `None` when there is no home to put it under.
fn log_dir() -> Option<PathBuf> {
    if let Some(state) = env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(state).join("mula"));
    }

    let home = PathBuf::from(env::var_os("HOME").filter(|dir| !dir.is_empty())?);
    Some(match cfg!(target_os = "macos") {
        true => home.join("Library/Logs/mula"),
        false => home.join(".local/state/mula"),
    })
}

/// Starts logging, and only when `RUST_LOG` asks for it: a file manager is
/// run from every directory there is, and one that logs by default leaves a
/// trail of them behind.
///
/// `None` when nothing was asked for, and also when the directory could not be
/// opened. Not being able to write a log is not a reason to refuse to run.
fn init_logging() -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().ok()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_prefix("mula.log")
        .build(log_dir()?)
        .ok()?;
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
        .init();

    Some(guard)
}

fn main() -> ExitCode {
    match read_command_line(env::args().skip(1)) {
        Ok(()) => (),
        Err(Answered::Question(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(Answered::Refusal(reason)) => {
            eprintln!("mula: {reason}");
            return ExitCode::FAILURE;
        }
    }

    let _guard = init_logging();

    tracing::info!("App starting...");
    let outcome: Result<(), AppError> = ratatui::run(|terminal| {
        // Asked before the loop starts: the terminal answers on standard
        // input, and `handle_events` would read the answer as keystrokes.
        let capabilities = Capabilities::detect();
        tracing::info!(?capabilities, "terminal graphics");

        if let Some(graphics) = capabilities.graphics() {
            forget_pictures_on_panic(graphics.protocol);
        }

        App::new(capabilities)?.run(terminal)
    });

    // Reported here rather than returned: `main` prints a `Result` through
    // `Debug`, and the terminal is its own again by the time this runs.
    if let Err(e) = outcome {
        eprintln!("mula: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Takes every picture off the screen if the app panics.
///
/// Installed after `ratatui::run` has hooked the panic itself, and a hook runs
/// ahead of the one it replaced: this goes first, while the alternate screen
/// the pictures were placed on is still up, and ratatui restores the terminal
/// after it. A placement belongs to the terminal rather than to a screen, so
/// one left behind would sit over the shell.
///
/// Only for a terminal that answered with a protocol; nothing else is written
/// a sequence it never claimed to understand.
fn forget_pictures_on_panic(protocol: Protocol) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = surface::forget_all(protocol, &mut io::stdout());
        previous(info);
    }));
}

#[cfg(test)]
mod command_line_tests {
    use super::*;

    fn read(args: &[&str]) -> Result<(), Answered> {
        read_command_line(args.iter().map(|arg| arg.to_string()))
    }

    /// The directory branch is left out on purpose: `set_current_dir` moves
    /// the whole process, and the tests around this one share it.
    #[test]
    fn nothing_on_the_command_line_runs_from_where_it_stands() {
        assert!(read(&[]).is_ok());
    }

    #[test]
    fn the_version_printed_is_the_one_the_package_carries() {
        let Err(Answered::Question(text)) = read(&["--version"]) else {
            panic!("--version is a question, not a refusal");
        };

        assert_eq!(
            text.trim_end(),
            format!("mula {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn both_spellings_of_help_answer_with_the_usage() {
        for spelling in ["-h", "--help"] {
            let Err(Answered::Question(text)) = read(&[spelling]) else {
                panic!("{spelling} is a question, not a refusal");
            };

            assert_eq!(text, USAGE, "{spelling}");
        }
    }

    /// An unknown option is not a directory named `--colour`: a leading dash
    /// is refused rather than taken as a path.
    #[test]
    fn an_unknown_option_is_refused_rather_than_entered() {
        assert!(matches!(read(&["--colour"]), Err(Answered::Refusal(_))));
    }

    #[test]
    fn a_second_directory_is_refused() {
        assert!(matches!(read(&["/tmp", "/var"]), Err(Answered::Refusal(_))));
    }
}
