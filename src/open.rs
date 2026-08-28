//! Handing the terminal to another program and taking it back.
//!
//! The screen, the raw mode and the pictures the terminal holds are all given
//! up together and all restored together, so the program that runs in between
//! finds a terminal indistinguishable from the one Mula was started in.

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt, io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        process::CommandExt,
    },
    path::Path,
    process::{Command, ExitStatus},
};

use ratatui::{
    DefaultTerminal,
    crossterm::{
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
};

use crate::ui::graphics::surface::Surface;

/// The signals the terminal driver raises from what the user typed and sends
/// to every process in the foreground group. Set aside in Mula for as long as
/// another program owns the terminal, and put back to their default in that
/// program itself.
const FROM_THE_TERMINAL: [i32; 2] = [libc::SIGINT, libc::SIGQUIT];

/// Which program a file is handed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    /// `$VISUAL`, then `$EDITOR`, then `vi`.
    Edit,
    /// `$PAGER`, then `less`.
    View,
}

impl Opener {
    /// The variables consulted, in order, and the program used when none of
    /// them names one.
    const fn spec(self) -> (&'static [&'static str], &'static str) {
        match self {
            Opener::Edit => (&["VISUAL", "EDITOR"], "vi"),
            Opener::View => (&["PAGER"], "less"),
        }
    }

    /// The program to run, read out of the environment at every call. A
    /// variable holding nothing but whitespace names no program and is passed
    /// over like an unset one.
    pub fn program(self) -> Program {
        let (names, fallback) = self.spec();
        names
            .iter()
            .filter_map(|name| env::var_os(name))
            .find_map(|spec| Program::parse(&spec))
            .unwrap_or_else(|| Program::of(fallback))
    }
}

/// A program to run, already split into what `Command` needs. Nothing is ever
/// handed to a shell, and the path travels as one element of `argv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    command: OsString,
    args: Vec<OsString>,
}

impl Program {
    /// Splits `spec` on ASCII whitespace: the first word is the program and
    /// the rest are arguments that go before the path. `None` when `spec` holds
    /// no word at all.
    ///
    /// Split over bytes rather than characters, so a program path that is not
    /// UTF-8 survives it.
    fn parse(spec: &OsStr) -> Option<Self> {
        let mut words = spec
            .as_bytes()
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|word| !word.is_empty())
            .map(|word| OsString::from_vec(word.to_vec()));

        Some(Self {
            command: words.next()?,
            args: words.collect(),
        })
    }

    /// A program run with no arguments of its own.
    fn of(command: &str) -> Self {
        Self {
            command: OsString::from(command),
            args: Vec::new(),
        }
    }
}

/// Writes the program without its arguments, which is what a message about it
/// names.
impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.command.to_string_lossy())
    }
}

/// What became of the program the terminal was handed to.
#[derive(Debug)]
pub enum Handover {
    /// It ran to the end. What it made of the file is its status.
    Exited(ExitStatus),
    /// It never started: nothing on `PATH` goes by that name, or what does
    /// could not be executed.
    NotStarted(io::Error),
}

/// Hands the terminal to `program` with `path` as its last argument and takes
/// it back once the program is done.
///
/// `path` must be absolute. It is the last argument and nothing quotes it, so
/// a relative one starting with `-` would reach the program as an option.
///
/// The returned error is the terminal's own and leaves nothing to draw on; a
/// program that would not start comes back as [`Handover::NotStarted`].
pub fn hand_over(
    terminal: &mut DefaultTerminal,
    surface: &mut Surface,
    program: &Program,
    path: &Path,
) -> io::Result<Handover> {
    let handover = leave(terminal, surface).map(|()| run(program, path));
    // Whatever came of the program, and whatever came of giving the screen up
    // in the first place, the screen has to come back.
    take_back(terminal)?;
    handover
}

/// Gives the screen up: the pictures first, then the cursor, the raw mode and
/// the alternate screen.
///
/// A placement belongs to the terminal rather than to the screen it was made
/// on, so a picture left behind would hang over the program that runs next.
fn leave(terminal: &mut DefaultTerminal, surface: &mut Surface) -> io::Result<()> {
    surface.clear(&mut io::stdout())?;
    // Ratatui hides the cursor on every frame that places none. A program that
    // does not turn it on itself would otherwise run blind.
    terminal.show_cursor()?;
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)
}

/// Takes the screen back and marks every cell as needing to be drawn again, so
/// the next frame repaints what the program wrote over.
///
/// The size is read from the terminal device rather than asked of the terminal
/// itself, so a window resized while the program ran is picked up without a
/// round trip over the same input the keys arrive on.
fn take_back(terminal: &mut DefaultTerminal) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let area = terminal.size()?.into();
    terminal.resize(area)
}

/// Runs `program` on `path` and waits for it, with the terminal already given
/// up.
///
/// The program starts in the directory holding `path`, so a relative name it is
/// given afterwards resolves against the panel rather than against wherever
/// Mula was started.
fn run(program: &Program, path: &Path) -> Handover {
    let mut command = Command::new(&program.command);
    command.args(&program.args).arg(path);
    if let Some(parent) = path.parent() {
        command.current_dir(parent);
    }
    hand_signals_back(&mut command);

    // Held for as long as the program runs and dropped on the way out.
    let _ignored = Ignored::of(&FROM_THE_TERMINAL);

    match command.status() {
        Ok(status) => Handover::Exited(status),
        Err(e) => Handover::NotStarted(e),
    }
}

/// Puts the signals of [`FROM_THE_TERMINAL`] back to their default in the
/// program that is about to run.
///
/// A disposition of "ignore" is inherited across `exec`, unlike a handler,
/// which is reset. Without this the program would inherit Mula's deafness to
/// Ctrl-C and could not be interrupted at all.
fn hand_signals_back(command: &mut Command) {
    // SAFETY: the closure runs in the forked child between `fork` and `exec`,
    // where only async-signal-safe calls are allowed. `signal` is one of them,
    // and the closure neither allocates nor takes a lock.
    unsafe {
        command.pre_exec(|| {
            for signal in FROM_THE_TERMINAL {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// Signals set aside for as long as another program owns the terminal, put
/// back as they were when this is dropped.
///
/// A signal that could not be set aside is left out, so it is not put back to
/// something it never was.
struct Ignored {
    previous: Vec<(i32, libc::sighandler_t)>,
}

impl Ignored {
    fn of(signals: &[i32]) -> Self {
        let previous = signals
            .iter()
            .filter_map(|&signal| {
                // SAFETY: `signal` is given a disposition rather than a handler
                // function, so nothing of this process is made to run in a
                // signal context.
                let previous = unsafe { libc::signal(signal, libc::SIG_IGN) };
                if previous == libc::SIG_ERR {
                    tracing::warn!(signal, "the signal could not be set aside");
                    return None;
                }
                Some((signal, previous))
            })
            .collect();

        Self { previous }
    }
}

impl Drop for Ignored {
    fn drop(&mut self) {
        for &(signal, previous) in &self.previous {
            // SAFETY: `previous` is what `signal` handed back for this same
            // signal, so it is a disposition this process was already running
            // under.
            unsafe { libc::signal(signal, previous) };
        }
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    fn parse(spec: &str) -> Option<Program> {
        Program::parse(OsStr::new(spec))
    }

    #[test]
    fn a_bare_name_is_the_program_and_no_arguments() {
        let program = parse("nvim").unwrap();

        assert_eq!(program, Program::of("nvim"));
    }

    #[test]
    fn the_words_after_the_program_are_its_arguments() {
        let program = parse("code --wait").unwrap();

        assert_eq!(program.command, OsString::from("code"));
        assert_eq!(program.args, vec![OsString::from("--wait")]);
    }

    /// A variable that was exported empty, or padded, names no program and has
    /// to fall through to the next one rather than run "".
    #[test]
    fn a_spec_holding_no_word_names_no_program() {
        assert!(parse("").is_none());
        assert!(parse("   \t ").is_none());
    }

    #[test]
    fn runs_of_whitespace_do_not_become_empty_arguments() {
        let program = parse("  emacs   -nw  ").unwrap();

        assert_eq!(program.command, OsString::from("emacs"));
        assert_eq!(program.args, vec![OsString::from("-nw")]);
    }

    /// Nothing is handed to a shell, so what looks like shell syntax is an
    /// argument like any other rather than something that runs.
    #[test]
    fn shell_syntax_is_kept_as_arguments() {
        let program = parse("vi; rm -rf /").unwrap();

        assert_eq!(program.command, OsString::from("vi;"));
        assert_eq!(
            program.args,
            vec![
                OsString::from("rm"),
                OsString::from("-rf"),
                OsString::from("/")
            ]
        );
    }

    #[test]
    fn a_program_is_named_without_its_arguments() {
        assert_eq!(parse("code --wait").unwrap().to_string(), "code");
    }
}
