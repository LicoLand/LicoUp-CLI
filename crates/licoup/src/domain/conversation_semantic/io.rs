use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::domain::conversation::event_semantics::hash_text;

use super::markdown::render_semantic_markdown;
use super::model::{SEMANTIC_JSON, SEMANTIC_MD};
use super::validation::validate_semantic_conversation;

pub fn materialize_semantic_documents(
    conversation_dir: &Path,
    semantic: &Value,
) -> Result<(PathBuf, PathBuf, String)> {
    let json_path = conversation_dir.join(SEMANTIC_JSON);
    let md_path = conversation_dir.join(SEMANTIC_MD);
    let json_text = serde_json::to_string_pretty(semantic)?;
    fs::write(&json_path, format!("{}\n", json_text))?;
    fs::write(&md_path, render_semantic_markdown(semantic))?;
    Ok((json_path, md_path, hash_text(&json_text)))
}

pub fn load_and_validate_fixture(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)?;
    validate_semantic_conversation(&value)?;
    Ok(value)
}
