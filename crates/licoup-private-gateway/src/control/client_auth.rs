//! Private client authentication for the loopback LLM Gateway.

use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use licoup_foundation::core::secret_bytes::SecretBytes;
use licoup_foundation::platform::file_security;
use rand_core::{OsRng, RngCore};
use std::path::{Path, PathBuf};

const TOKEN_BYTES: usize = 32;
const TOKEN_TEXT_BYTES: usize = 43;
const MAX_TOKEN_FILE_BYTES: usize = 128;

/// The token path of the runtime's own state layout.
///
/// The layout is declared once ([`crate::state`]) and read here, so the program
/// that needs a token and the program that reads it from the login item cannot
/// disagree about where it lives.
pub fn default_token_path() -> Result<PathBuf> {
    Ok(crate::state::GatewayStatePaths::for_current_home()?.client_token)
}

/// Return the stable, private token used by local API clients. Creation uses
/// create-new semantics so concurrent helpers cannot observe different tokens.
pub fn ensure_default_token() -> Result<SecretBytes> {
    let path = default_token_path()?;
    ensure_token_at(&path)
}

/// The same stable token of one explicit path.
///
/// The runtime process is started by the login item with nothing but a port, so
/// it is the first program to need a token for the layout it was started for
/// rather than for this process's own default path. The creation semantics are
/// the ones above: create-new, so two helpers cannot observe different tokens.
pub fn ensure_token_at(path: &Path) -> Result<SecretBytes> {
    ensure_token(path)
}

fn ensure_token(path: &Path) -> Result<SecretBytes> {
    if let Some(token) = read_token_if_present(&path)? {
        return Ok(token);
    }

    let mut random = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut random);
    let encoded = URL_SAFE_NO_PAD.encode(random);
    random.fill(0);
    validate_token_text(&encoded)?;

    match file_security::create_private_state_marker(&path, encoded.as_bytes()) {
        Ok(()) => SecretBytes::try_from_string(encoded)
            .map_err(|_| anyhow!("gateway_client_token_invalid")),
        Err(_) => {
            read_token_if_present(&path)?.ok_or_else(|| anyhow!("gateway_client_token_unavailable"))
        }
    }
}

pub fn read_token(path: &Path) -> Result<SecretBytes> {
    read_token_if_present(path)?.ok_or_else(|| anyhow!("gateway_client_token_unavailable"))
}

fn read_token_if_present(path: &Path) -> Result<Option<SecretBytes>> {
    let Some(text) = file_security::read_private_text_bounded(path, MAX_TOKEN_FILE_BYTES)? else {
        return Ok(None);
    };
    validate_token_text(&text)?;
    SecretBytes::try_from_string(text)
        .map(Some)
        .map_err(|_| anyhow!("gateway_client_token_invalid"))
}

fn validate_token_text(token: &str) -> Result<()> {
    ensure!(
        token.len() == TOKEN_TEXT_BYTES
            && token
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' }),
        "gateway_client_token_invalid"
    );
    let decoded = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| anyhow!("gateway_client_token_invalid"))?;
    ensure!(decoded.len() == TOKEN_BYTES, "gateway_client_token_invalid");
    Ok(())
}

/// Compare fixed-length authentication material without early exit.
pub fn token_matches(expected: &SecretBytes, presented: &str) -> bool {
    let expected = expected.expose_bytes();
    let presented = presented.as_bytes();
    let mut difference = expected.len() ^ presented.len();
    for index in 0..expected.len().max(presented.len()) {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = presented.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn token_comparison_is_exact() {
        let token = SecretBytes::try_from_string("a".repeat(TOKEN_TEXT_BYTES)).unwrap();
        assert!(token_matches(&token, &"a".repeat(TOKEN_TEXT_BYTES)));
        assert!(!token_matches(&token, &"b".repeat(TOKEN_TEXT_BYTES)));
        assert!(!token_matches(&token, "short"));
    }

    #[test]
    fn generated_shape_is_a_32_byte_base64url_token() {
        let encoded = URL_SAFE_NO_PAD.encode([7u8; TOKEN_BYTES]);
        validate_token_text(&encoded).unwrap();
        assert!(validate_token_text("licoup-local").is_err());
        assert!(validate_token_text(&format!("{}=", "a".repeat(TOKEN_TEXT_BYTES))).is_err());
    }

    #[test]
    fn private_token_is_created_once_and_reused() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("licoup-gateway-auth-{nonce}"));
        file_security::ensure_private_dir(&root).unwrap();
        let path = root.join("token");
        let first = ensure_token(&path).unwrap();
        let second = ensure_token(&path).unwrap();
        assert!(token_matches(
            &first,
            second.expose_utf8().expect("generated token stays UTF-8")
        ));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
