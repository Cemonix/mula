use std::{env, io, panic, path::PathBuf};

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
mod fs;
mod keys;
mod open;
mod ui;

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

fn main() -> Result<(), AppError> {
    let _guard = init_logging();

    tracing::info!("App starting...");
    ratatui::run(|terminal| {
        // Asked before the loop starts: the terminal answers on standard
        // input, and `handle_events` would read the answer as keystrokes.
        let capabilities = Capabilities::detect();
        tracing::info!(?capabilities, "terminal graphics");

        if let Some(graphics) = capabilities.graphics() {
            forget_pictures_on_panic(graphics.protocol);
        }

        App::new(capabilities)?.run(terminal)
    })
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
