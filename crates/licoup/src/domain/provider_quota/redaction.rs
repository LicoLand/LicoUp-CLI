//! Redaction guard applied before any snapshot, retained-state, log, or
//! diagnostic write. The quota contract carries no credential fields by
//! construction; this guard is the enforced backstop that strips
//! credential-shaped keys and bearer-shaped strings from anything the domain
//! emits.

use serde_json::Value;

const MAX_REDACTION_DEPTH: usize = 16;
const REDACTED: &str = "<redacted>";

/// Strip credential-shaped fields from one outgoing JSON value in place.
pub(super) fn redact_outgoing(value: &mut Value) {
    redact_value(value, 0);
}

/// True when the value still contains credential-shaped material. Tests and
/// debug assertions use this to prove the privacy boundary on emitted
/// artifacts.
#[cfg(test)]
pub(super) fn contains_credential_material(value: &Value) -> bool {
    let mut probe = value.clone();
    redact_value(&mut probe, 0);
    probe != *value
}

fn redact_value(value: &mut Value, depth: usize) {
    if depth > MAX_REDACTION_DEPTH {
        *value = Value::String(REDACTED.to_string());
        return;
    }
    match value {
        Value::Object(map) => {
            let keys = map.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                if is_credential_key(&key) {
                    map.insert(key, Value::String(REDACTED.to_string()));
                } else if let Some(child) = map.get_mut(&key) {
                    redact_value(child, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for child in items.iter_mut() {
                redact_value(child, depth + 1);
            }
        }
        Value::String(text) => {
            if let Some(redacted) = redact_bearer_text(text) {
                *text = redacted;
            }
        }
        _ => {}
    }
}

fn is_credential_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("authorization")
        || normalized.contains("authheader")
        || normalized.contains("privatekey")
        || normalized.contains("clientsecret")
        || normalized.contains("csrf")
        || normalized.contains("cookie")
}

fn redact_bearer_text(text: &str) -> Option<String> {
    let lowered = text.to_ascii_lowercase();
    let index = lowered.find("bearer ")?;
    let start = index + "bearer ".len();
    let tail = &text[start..];
    let tail_len = tail
        .find(|ch: char| {
            !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '~' | '+' | '/' | '=' | '-'))
        })
        .unwrap_or(tail.len());
    if tail_len == 0 {
        return None;
    }
    let mut redacted = text.to_string();
    redacted.replace_range(start..start + tail_len, REDACTED);
    Some(redacted)
}
