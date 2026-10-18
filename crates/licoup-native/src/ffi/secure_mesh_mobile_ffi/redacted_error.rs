use serde_json::{Value, json};

pub(super) fn unsupported_action_response(action: &str, unsupported_code: &'static str) -> Value {
    json!({
        "ok": false,
        "code": unsupported_code,
        "action": action,
    })
}
