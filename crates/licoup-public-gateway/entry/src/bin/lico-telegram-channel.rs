//! Telegram channel process: the `org.licoland.channel.telegram` package entry.
//!
//! The process owns exactly one job: run the channel's long-polling loop
//! against the bot token in the channel's custody store. Every Agent call the
//! channel makes travels the lane port this binary installs from the host, so
//! the channel never links conversation internals directly.
//!
//! Lifetime is deliberately simple: the supervisor stops the process with
//! SIGTERM, and the channel reports readiness through its state files the same
//! way it did when it ran inside the gateway runtime.

use licoup_native::platform::gateway_composition;
use licoup_public_gateway::telegram::{
    BindingStore, LiveBotTransport, RuntimeConfig, clear_ready, load_bot_token, run_channel_loop,
};
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
    gateway_composition::install_channel().map_err(|_| ())?;
    let token = load_bot_token().map_err(|_| ())?.ok_or(())?;
    let transport = LiveBotTransport::new(token, api_root.as_deref());
    let store = BindingStore::open_default().map_err(|_| ())?;
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
