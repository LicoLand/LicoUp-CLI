//! Telegram Communication Channel: Bot API transport, update loop, lane
//! bridge, binding store, bot-token custody and control commands.

pub mod binding;
pub mod bridge;
pub mod control;
pub mod credentials;
pub mod inbound;
pub mod runtime;
pub mod transport;

pub use binding::{
    BindingStore, ChatBinding, PairingRecord, approve_pairing, list_pairings, revoke_pairing,
};
pub use control::{ControlCommand, ControlOutcome, parse_control_command};
pub use credentials::{
    clear_bot_token, credentials_status, load_bot_token, set_bot_token, token_configured,
};
pub use runtime::{RuntimeConfig, run_channel_loop};
pub use transport::{
    BotIdentity, BotTransport, LiveBotTransport, MockBotTransport, TelegramApiError, Update,
    bot_commands,
};

use anyhow::Result;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
    remove_private_state_marker,
};
use licoup_foundation::platform::paths;
use serde_json::{Value, json};
use std::path::PathBuf;

const CHANNEL_STATE: &str = "telegram-gateway";
const READY_FILE: &str = "channel.ready";

fn ready_path() -> Result<PathBuf> {
    let root = paths::portable_data_dir()?.join(CHANNEL_STATE);
    ensure_private_dir(&root)?;
    Ok(root.join(READY_FILE))
}

pub fn mark_ready(bot_username: &str) -> Result<()> {
    let username = bot_username.trim_start_matches('@');
    atomic_write_private_text(
        &ready_path()?,
        &serde_json::to_string(&json!({
            "channelId": "telegram",
            "state": "running",
            "botUsername": username,
        }))?,
    )
}

pub fn clear_ready() -> Result<()> {
    let _ = remove_private_state_marker(&ready_path()?)?;
    Ok(())
}

pub fn channel_status() -> Result<Value> {
    let credentials = credentials_status()?;
    let configured = credentials
        .get("configured")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let ready = read_private_text_bounded(&ready_path()?, 1024)?
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let state = if let Some(ready) = ready.as_ref() {
        ready
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("running")
    } else if configured {
        "configured"
    } else {
        "unconfigured"
    };
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.gateway-channel-telegram.v1",
        "channelId": "telegram",
        "layer": "communication-channel",
        "state": state,
        "configured": configured,
        "token": credentials.get("token").cloned().unwrap_or(json!("missing")),
        "tokenSource": credentials.get("tokenSource").cloned().unwrap_or(json!("none")),
        "botUsername": ready.as_ref().and_then(|value| value.get("botUsername").cloned()),
    }))
}

/// Channel-layer status envelope: today the only channel is Telegram.
pub fn channel_layer_status() -> Result<Value> {
    let telegram = channel_status()?;
    Ok(json!({
        "ok": true,
        "schemaVersion": "licoup.gateway-channels.v1",
        "layer": "communication-channel",
        "channels": {
            "telegram": telegram,
        }
    }))
}
