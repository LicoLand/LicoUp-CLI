use super::*;
use serde_json::{Map, Value};

mod transfer;
mod wire;

fn object(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}
