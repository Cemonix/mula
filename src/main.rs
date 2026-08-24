use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    app::{App, AppError},
    ui::graphics::capabilities::Capabilities,
};

mod action;
mod app;
mod fs;
mod keys;
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

        App::new(capabilities)?.run(terminal)
    })
}
