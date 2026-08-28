use std::{io, panic};

use tracing_appender::non_blocking::WorkerGuard;
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

fn init_logging() -> WorkerGuard {
    let file_appender = tracing_appender::rolling::never("logs", "mula.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
        .init();

    _guard
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
