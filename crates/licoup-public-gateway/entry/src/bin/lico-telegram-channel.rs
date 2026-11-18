//! Telegram channel process: the `org.licoland.channel.telegram` package entry.
//!
//! The process owns exactly one job: run the channel's long-polling loop. The
//! bot token resolves through the custody port and every Agent call travels
//! the lane port — both installed here from the host composition, so the
//! channel never links custody or conversation internals directly.
//!
//! Lifetime is deliberately simple: the supervisor stops the process with
//! SIGTERM, and the channel reports readiness through its state files the same
//! way it did when it ran inside the gateway runtime.

use licoup::platform::gateway_composition;
use licoup_public_gateway::telegram::{self, RuntimeConfig, clear_ready, run_channel_loop};
use std::env;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run() -> Result<(), ()> {
    if env::args().any(|arg| arg == "--check") {
        println!(
            r#"{{"ok":true,"schemaVersion":"licoup.telegram-channel-check.v1","channels":["telegram"]}}"#
        );
        return Ok(());
    }
    let api_root = env::var("TELEGRAM_API_ROOT").ok();
    // Custody and lane ports are installed by the host composition; the
    // dialect then resolves its token through custody, never a direct read.
    gateway_composition::install_channel().map_err(|_| ())?;
    let transport = telegram::live_transport(api_root.as_deref()).map_err(|_| ())?;
    let store = telegram::open_bindings().map_err(|_| ())?;
    let result = run_channel_loop(
        transport,
        store,
        RuntimeConfig {
            stop: Arc::new(AtomicBool::new(false)),
            ..RuntimeConfig::default()
        },
    );
    let _ = clear_ready();
    result.map(|_| ()).map_err(|_| ())
}
