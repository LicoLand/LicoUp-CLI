//! Stable public facade for read-only native conversation history.

use anyhow::Result;
use serde_json::Value;

pub fn conversation_list(params: &Value) -> Result<Value> {
    super::conversation::history::conversation_list(params)
}

pub fn model_catalog(params: &Value) -> Result<Value> {
    super::conversation::history::model_catalog(params)
}

pub fn conversation_stream(params: &Value) -> Result<()> {
    super::conversation::history::conversation_stream(params)
}

pub fn conversation_append(params: &Value) -> Result<Value> {
    super::conversation::history::conversation_append(params)
}

pub fn conversation_delete(params: &Value) -> Result<Value> {
    super::conversation::history::conversation_delete(params)
}
