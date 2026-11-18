use serde_json::Value;

mod antigravity;
mod generated_context;
mod json_extract;
mod projection;
mod semantic;
mod structured_privacy;

fn drop_json_iteratively(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(items) => pending.extend(items),
            Value::Object(object) => pending.extend(object.into_iter().map(|(_, value)| value)),
            _ => {}
        }
    }
}
