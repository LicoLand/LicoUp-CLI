use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn advance_event_chain(chain_hash: &mut String, domain: &[u8], value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hash_bytes(&mut hasher, b'p', chain_hash.as_bytes());
    hash_rollout_item(&mut hasher, value);
    let next = format!("{:x}", hasher.finalize());
    chain_hash.clone_from(&next);
    next
}

fn hash_rollout_item(hasher: &mut Sha256, value: &Value) {
    let Some(object) = value.as_object() else {
        hash_canonical_json(hasher, value);
        return;
    };
    let response_item = object.get("type").and_then(Value::as_str) == Some("response_item");
    let mut keys = object
        .keys()
        .filter(|key| key.as_str() != "timestamp")
        .collect::<Vec<_>>();
    keys.sort();
    hasher.update(b"o");
    hasher.update((keys.len() as u64).to_be_bytes());
    for key in keys {
        hash_bytes(hasher, b'k', key.as_bytes());
        let child = object.get(key).unwrap_or(&Value::Null);
        if response_item && key == "payload" {
            hash_response_payload(hasher, child);
        } else {
            hash_canonical_json(hasher, child);
        }
    }
}

fn hash_response_payload(hasher: &mut Sha256, value: &Value) {
    let Some(object) = value.as_object() else {
        hash_canonical_json(hasher, value);
        return;
    };
    let mut keys = object
        .keys()
        .filter(|key| key.as_str() != "id")
        .collect::<Vec<_>>();
    keys.sort();
    hasher.update(b"o");
    hasher.update((keys.len() as u64).to_be_bytes());
    for key in keys {
        hash_bytes(hasher, b'k', key.as_bytes());
        hash_canonical_json(hasher, object.get(key).unwrap_or(&Value::Null));
    }
}

fn hash_canonical_json(hasher: &mut Sha256, value: &Value) {
    match value {
        Value::Null => hasher.update(b"n"),
        Value::Bool(value) => hasher.update(if *value { b"t" } else { b"f" }),
        Value::Number(value) => hash_bytes(hasher, b'#', value.to_string().as_bytes()),
        Value::String(value) => hash_bytes(hasher, b's', value.as_bytes()),
        Value::Array(values) => {
            hasher.update(b"a");
            hasher.update((values.len() as u64).to_be_bytes());
            for value in values {
                hash_canonical_json(hasher, value);
            }
        }
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            hasher.update(b"o");
            hasher.update((keys.len() as u64).to_be_bytes());
            for key in keys {
                hash_bytes(hasher, b'k', key.as_bytes());
                hash_canonical_json(hasher, object.get(key).unwrap_or(&Value::Null));
            }
        }
    }
}

fn hash_bytes(hasher: &mut Sha256, tag: u8, value: &[u8]) {
    hasher.update([tag]);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
