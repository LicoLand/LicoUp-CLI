//! Telegram Communication Channel: the product binding over the
//! `a4mc-telegram` dialect.
//!
//! The dialect crate owns the channel machinery — Bot API long polling,
//! update lowering, pairing and per-chat bindings, the slash-command
//! surface and normalized sends. What stays here is the product seam:
//!
//! * `credentials` — bot-token custody under the portable private data
//!   root and its env fallback, exposed to the dialect as the
//!   `a4mc::custody` port this binding installs;
//! * the portable state directory and the ready/status files the host's
//!   service lifecycle and `gateway channel status` read;
//! * the pairing CLI surface (`list` / `approve` / `revoke`) over the
//!   dialect's binding store;
//! * `run_channel_loop` — the dialect loop with the product readiness
//!   marker wired into its start hook.
//!
//! Conversation work reaches the host through `a4mc::lane` (re-exported
//! as [`crate::lane`]); the composing host installs both ports once per
//! process and without them every call fails closed.

pub mod credentials;

pub use a4mc_telegram::{
    BindingStore, BotIdentity, BotTransport, ChatBinding, ControlCommand, ControlOutcome,
    LiveBotTransport, MockBotTransport, PairingRecord, RuntimeConfig, TelegramApiError, Update,
    bot_commands, live_transport, parse_control_command, validate_bot_token,
};
pub use credentials::{
    clear_bot_token, credentials_status, install_custody, load_bot_token, set_bot_token,
    token_configured,
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
/// CLI output schema for the pairing commands — a product document
/// contract, independent of the dialect's on-disk store schema.
const PAIRING_SCHEMA: &str = "licoup.telegram-gateway-bindings.v1";

/// The channel's portable state directory: bindings, ready marker, and the
/// pid/log files the service lifecycle writes.
pub fn state_dir() -> Result<PathBuf> {
    let root = paths::portable_data_dir()?.join(CHANNEL_STATE);
    ensure_private_dir(&root)?;
    Ok(root)
}

/// Open the dialect's binding store at the product state directory.
pub fn open_bindings() -> Result<BindingStore> {
    BindingStore::open_in(&state_dir()?)
}

fn ready_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(READY_FILE))
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

/// Run the dialect channel loop with the product readiness marker wired:
/// once the Bot API handshake succeeds, the ready file records the bot
/// username exactly as the in-process runtime did.
pub fn run_channel_loop<T: BotTransport + 'static>(
    transport: T,
    store: BindingStore,
    mut config: RuntimeConfig,
) -> Result<BotIdentity> {
    if config.on_started.is_none() {
        config.on_started = Some(Box::new(|identity: &BotIdentity| {
            let _ = mark_ready(&identity.username);
        }));
    }
    a4mc_telegram::run_channel_loop(transport, store, config)
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

/// List pending pairings and approved chats for the pairing CLI.
pub fn list_pairings() -> Result<Value> {
    let mut store = open_bindings()?;
    store.refresh()?;
    store.prune_expired()?;
    Ok(json!({
        "ok": true,
        "schemaVersion": PAIRING_SCHEMA,
        "pairings": store.pending_pairings(),
        "chats": store.paired_chats(),
    }))
}

/// Approve one pending pairing code — the host-side admission decision the
/// dialect's store records.
pub fn approve_pairing(code: &str) -> Result<Value> {
    let mut store = open_bindings()?;
    let binding = store.approve(code)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": PAIRING_SCHEMA,
        "approved": true,
        "binding": binding,
    }))
}

/// Revoke a chat's pairing and any pending pairing for it.
pub fn revoke_pairing(chat_id: i64) -> Result<Value> {
    let mut store = open_bindings()?;
    let removed = store.revoke(chat_id)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": PAIRING_SCHEMA,
        "revoked": removed,
        "chatId": chat_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn with_state_dir<F: FnOnce()>(body: F) {
        let _guard = LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("licoup-tg-mod-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&root);
        let previous = set_portable_data_dir_override(Some(root.clone()));
        body();
        set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pairing_cli_round_trips_over_the_dialect_store() {
        with_state_dir(|| {
            let mut store = open_bindings().unwrap();
            let pending = store.request_pairing(55, 66, Some("carol".into())).unwrap();
            drop(store);

            let listed = list_pairings().unwrap();
            assert_eq!(listed["schemaVersion"], json!(PAIRING_SCHEMA));
            assert_eq!(listed["pairings"].as_array().unwrap().len(), 1);

            let approved = approve_pairing(&pending.code).unwrap();
            assert_eq!(approved["approved"], json!(true));
            assert_eq!(approved["binding"]["chatId"], json!(55));

            let listed = list_pairings().unwrap();
            assert!(listed["pairings"].as_array().unwrap().is_empty());
            assert_eq!(listed["chats"].as_array().unwrap().len(), 1);

            let revoked = revoke_pairing(55).unwrap();
            assert_eq!(revoked["revoked"], json!(true));
        });
    }

    #[test]
    fn ready_marker_round_trips_into_channel_status() {
        with_state_dir(|| {
            mark_ready("@licoup_bot").unwrap();
            let status = channel_status().unwrap();
            assert_eq!(status["state"], json!("running"));
            assert_eq!(status["botUsername"], json!("licoup_bot"));
            clear_ready().unwrap();
            let status = channel_status().unwrap();
            assert_eq!(status["state"], json!("unconfigured"));
            assert_eq!(status["botUsername"], Value::Null);
        });
    }
}
