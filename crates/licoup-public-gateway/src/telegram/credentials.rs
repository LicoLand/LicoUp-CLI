//! Bot token custody under the portable private data root.
//!
//! Inventory and status never return the token. Development may fall back to
//! `TELEGRAM_BOT_TOKEN` when no stored credential exists.

use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::{
    atomic_write_private_text, ensure_private_dir, read_private_text_bounded,
    remove_private_state_marker,
};
use licoup_foundation::platform::paths;
use serde_json::{Value, json};
use std::env;
use std::path::PathBuf;

const STATE_DIRECTORY: &str = "telegram-gateway";
const TOKEN_FILE: &str = "bot.token";
const MAX_TOKEN_BYTES: usize = 512;
pub const CREDENTIALS_SCHEMA: &str = "licoup.telegram-gateway-credentials.v1";

fn root_dir() -> Result<PathBuf> {
    let root = paths::portable_data_dir()?.join(STATE_DIRECTORY);
    ensure_private_dir(&root)?;
    Ok(root)
}

fn token_path() -> Result<PathBuf> {
    Ok(root_dir()?.join(TOKEN_FILE))
}

fn validate_token(token: &str) -> Result<()> {
    let trimmed = token.trim();
    ensure!(!trimmed.is_empty(), "telegram_gateway_token_invalid");
    ensure!(
        trimmed.len() <= MAX_TOKEN_BYTES,
        "telegram_gateway_token_invalid"
    );
    ensure!(
        !trimmed.chars().any(|ch| ch.is_whitespace()),
        "telegram_gateway_token_invalid"
    );
    ensure!(
        trimmed.contains(':') && trimmed.is_ascii(),
        "telegram_gateway_token_invalid"
    );
    Ok(())
}

/// Persist a BotFather token into the private data root.
pub fn set_bot_token(token: &str) -> Result<Value> {
    validate_token(token)?;
    let path = token_path()?;
    atomic_write_private_text(&path, token.trim())?;
    credentials_status()
}

/// Remove the stored token. Env fallback remains available for development.
pub fn clear_bot_token() -> Result<Value> {
    let path = token_path()?;
    let _ = remove_private_state_marker(&path)?;
    credentials_status()
}

/// Whether a stored or env token is available (never returns the secret).
pub fn token_configured() -> Result<bool> {
    Ok(load_bot_token()?.is_some())
}

/// Load the bot token for gateway runtime use. Prefer the private store, then
/// `TELEGRAM_BOT_TOKEN` for the default account only.
pub fn load_bot_token() -> Result<Option<String>> {
    if let Some(stored) = read_private_text_bounded(&token_path()?, MAX_TOKEN_BYTES)? {
        let trimmed = stored.trim().to_owned();
        if !trimmed.is_empty() {
            validate_token(&trimmed)?;
            return Ok(Some(trimmed));
        }
    }
    match env::var("TELEGRAM_BOT_TOKEN") {
        Ok(value) => {
            let trimmed = value.trim().to_owned();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                validate_token(&trimmed)?;
                Ok(Some(trimmed))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(anyhow!("telegram_gateway_token_env_invalid")),
    }
}

pub fn credentials_status() -> Result<Value> {
    let stored = read_private_text_bounded(&token_path()?, MAX_TOKEN_BYTES)?
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let env_present = env::var_os("TELEGRAM_BOT_TOKEN")
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    Ok(json!({
        "ok": true,
        "schemaVersion": CREDENTIALS_SCHEMA,
        "configured": stored || env_present,
        "tokenSource": if stored {
            "store"
        } else if env_present {
            "env"
        } else {
            "none"
        },
        "token": if stored || env_present { "configured" } else { "missing" },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_foundation::platform::paths::set_portable_data_dir_override;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_root<F: FnOnce(PathBuf)>(body: F) {
        let _guard = LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("licoup-tg-cred-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&root);
        let previous = set_portable_data_dir_override(Some(root.clone()));
        // Clear env interference for deterministic store tests.
        let previous_env = env::var("TELEGRAM_BOT_TOKEN").ok();
        unsafe { env::remove_var("TELEGRAM_BOT_TOKEN") };
        body(root.clone());
        if let Some(value) = previous_env {
            unsafe { env::set_var("TELEGRAM_BOT_TOKEN", value) };
        } else {
            unsafe { env::remove_var("TELEGRAM_BOT_TOKEN") };
        }
        set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn set_status_and_clear_never_echo_token() {
        with_temp_root(|_| {
            let status = set_bot_token("123456:ABC-DEF_test_token").unwrap();
            assert_eq!(status["configured"], true);
            assert_eq!(status["token"], "configured");
            assert_eq!(status["tokenSource"], "store");
            let serialized = status.to_string();
            assert!(!serialized.contains("ABC-DEF_test_token"));
            assert_eq!(
                load_bot_token().unwrap().as_deref(),
                Some("123456:ABC-DEF_test_token")
            );
            let cleared = clear_bot_token().unwrap();
            assert_eq!(cleared["configured"], false);
            assert_eq!(cleared["token"], "missing");
        });
    }

    #[test]
    fn rejects_invalid_token_shapes() {
        with_temp_root(|_| {
            assert!(set_bot_token("").is_err());
            assert!(set_bot_token("no-colon").is_err());
            assert!(set_bot_token("123:has space").is_err());
        });
    }
}
