use std::{
    env, io,
    os::unix::ffi::OsStrExt,
    panic,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

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
mod favorites;
mod fs;
mod keys;
mod open;
mod ui;

const USAGE: &str = "\
mula — a dual-pane terminal file manager

Usage:
    mula [OPTIONS] [DIRECTORY]

Both panels open in DIRECTORY, or in the directory mula was started from when
none is given.

Options:
    --init SHELL     Print the shell function that leaves your shell in the
                     directory mula was quit in; SHELL is bash, zsh or fish.
                     Put `eval \"$(mula --init zsh)\"` in your rc file
    --cwd-file FILE  On quitting, write the directory of the focused panel
                     to FILE; the function above uses this
    -h, --help       Print this and exit
    -V, --version    Print the version and exit

Press F1 while it runs for the keys that work where you are standing.
";

/// What the command line asked for, when it did not ask for the app to run.
enum Answered {
    /// A question the command line answers on its own: `--help`, `--version`,
    /// `--init`.
    Question(String),
    /// Nothing that can be run: an unknown option, or a directory that cannot
    /// be entered.
    Refusal(String),
}

/// What the command line asked of a run.
#[derive(Default)]
struct Options {
    /// Where the directory the focused panel ends in is written.
    cwd_file: Option<PathBuf>,
}

/// Reads the command line and leaves the process in the directory the panels
/// should open in.
///
/// A directory argument is spent here rather than carried any further: the
/// listings grow from `env::current_dir()`, so `mula /tmp` and `cd /tmp &&
/// mula` are the same run, and the paths handed to other programs stay
/// absolute without anything else having to know an argument was given.
fn read_command_line(mut args: impl Iterator<Item = String>) -> Result<Options, Answered> {
    let mut directory: Option<PathBuf> = None;
    let mut options = Options::default();

    while let Some(arg) = args.next() {
        if let Some(file) = arg.strip_prefix("--cwd-file=") {
            options.cwd_file = Some(PathBuf::from(file));
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => return Err(Answered::Question(USAGE.to_string())),
            "-V" | "--version" => {
                let version = format!("{} {}\n", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return Err(Answered::Question(version));
            }
            "--init" => {
                let function = match args.next().as_deref() {
                    Some("bash" | "zsh") => include_str!("shell/posix.sh"),
                    Some("fish") => include_str!("shell/fish.fish"),
                    Some(shell) => {
                        return Err(Answered::Refusal(format!(
                            "no function for {shell}; there is one for bash, zsh and fish."
                        )));
                    }
                    None => {
                        return Err(Answered::Refusal(
                            "--init needs a shell: bash, zsh or fish.".to_string(),
                        ));
                    }
                };
                return Err(Answered::Question(function.to_string()));
            }
            "--cwd-file" => match args.next() {
                Some(file) => options.cwd_file = Some(PathBuf::from(file)),
                None => {
                    return Err(Answered::Refusal(
                        "--cwd-file needs the file to write to.".to_string(),
                    ));
                }
            },
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
        return Ok(options);
    };

    // `set_current_dir` is the whole check: a path that is not a directory
    // comes back as `Not a directory`, which is the complaint we would have
    // written ourselves.
    env::set_current_dir(&directory)
        .map_err(|e| Answered::Refusal(format!("{}: {e}", directory.display())))?;
    Ok(options)
}

/// Writes `directory` to `file` as raw bytes with nothing after it, so a name
/// that is not UTF-8 reaches the shell as it is on disk.
fn write_cwd(file: &Path, directory: &Path) -> io::Result<()> {
    std::fs::write(file, directory.as_os_str().as_bytes())
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
    let options = match read_command_line(env::args().skip(1)) {
        Ok(options) => options,
        Err(Answered::Question(text)) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(Answered::Refusal(reason)) => {
            eprintln!("mula: {reason}");
            return ExitCode::FAILURE;
        }
    };

    let _guard = init_logging();

    tracing::info!("App starting...");
    let outcome: Result<Arc<Path>, AppError> = ratatui::run(|terminal| {
        // Asked before the loop starts: the terminal answers on standard
        // input, and `handle_events` would read the answer as keystrokes.
        let capabilities = Capabilities::detect();
        tracing::info!(?capabilities, "terminal graphics");

        if let Some(graphics) = capabilities.graphics() {
            forget_pictures_on_panic(graphics.protocol);
        }

        let mut app = App::new(capabilities)?;
        app.run(terminal)?;
        Ok(Arc::clone(app.get_focused_pane().get_current_dir()))
    });

    // Reported here rather than returned: `main` prints a `Result` through
    // `Debug`, and the terminal is its own again by the time this runs.
    let directory = match outcome {
        Ok(directory) => directory,
        Err(e) => {
            eprintln!("mula: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(file) = options.cwd_file
        && let Err(e) = write_cwd(&file, &directory)
    {
        eprintln!("mula: {}: {e}", file.display());
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

    fn read(args: &[&str]) -> Result<Options, Answered> {
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

    #[test]
    fn both_spellings_of_the_cwd_file_name_it() {
        for args in [&["--cwd-file", "/tmp/cwd"][..], &["--cwd-file=/tmp/cwd"]] {
            let Ok(options) = read(args) else {
                panic!("{args:?} is a run, not an answer");
            };

            assert_eq!(options.cwd_file.as_deref(), Some(Path::new("/tmp/cwd")));
        }
    }

    /// The function has to call the binary past itself, or `mula` inside a
    /// function named `mula` would call the function again.
    #[test]
    fn every_shell_is_given_a_function_that_calls_the_binary() {
        for shell in ["bash", "zsh", "fish"] {
            let Err(Answered::Question(text)) = read(&["--init", shell]) else {
                panic!("--init {shell} is a question, not a refusal");
            };

            assert!(text.contains("command mula --cwd-file="), "{shell}");
        }
    }

    #[test]
    fn a_shell_without_a_function_is_refused() {
        assert!(matches!(
            read(&["--init", "tcsh"]),
            Err(Answered::Refusal(_))
        ));
        assert!(matches!(read(&["--init"]), Err(Answered::Refusal(_))));
    }

    #[test]
    fn a_cwd_file_without_a_name_is_refused() {
        assert!(matches!(read(&["--cwd-file"]), Err(Answered::Refusal(_))));
    }
}
