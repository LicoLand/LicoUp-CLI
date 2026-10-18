//! Local-only skill catalog facade.
//!
//! LicoUp can discover, display, hide, and move skills that already exist in
//! an agent's local skill root to the system Trash. It does not download,
//! install, update, synchronize, or roll back skill packages.

use crate::platform::client_state::ClientStateStore;
use anyhow::{Result, anyhow, ensure};
use licoup_foundation::platform::file_security::validate_no_symlink_ancestors;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod catalog;
mod delete;
mod discovery;
mod package;
mod pairing;
mod state;
mod usage;

use catalog::{get as skill_get_in, list as skill_list_in, visibility as skill_visibility_in};
use package::{inspect_skill_dir, sanitize_skill_id};
use pairing::{
    approve as pair_approve_in, list as pair_list_in, request as pair_request_in,
    revoke as pair_revoke_in,
};
use state::*;

const STATUS_APPROVED: &str = "approved";
const STATUS_REVOKED: &str = "revoked";

pub fn pair_request(params: &Value) -> Result<Value> {
    pair_request_in(&ClientStateStore::portable()?, params)
}

pub fn pair_approve(params: &Value) -> Result<Value> {
    pair_approve_in(&ClientStateStore::portable()?, params)
}

pub fn pair_revoke(params: &Value) -> Result<Value> {
    pair_revoke_in(&ClientStateStore::portable()?, params)
}

pub fn pair_list(params: &Value) -> Result<Value> {
    pair_list_in(&ClientStateStore::portable()?, params)
}

pub fn skill_list(params: &Value) -> Result<Value> {
    skill_list_in(&ClientStateStore::portable()?, params)
}

pub fn skill_get(params: &Value) -> Result<Value> {
    skill_get_in(&ClientStateStore::portable()?, params)
}

pub fn skill_visibility(params: &Value) -> Result<Value> {
    skill_visibility_in(&ClientStateStore::portable()?, params)
}

pub fn observe_agent_skill_invocations(agent_id: &str, result: &Value) -> Result<Value> {
    usage::observe_conversation_result(&ClientStateStore::portable()?, agent_id, result)
}

pub fn skill_usage_report(params: &Value) -> Result<Value> {
    usage::report(&ClientStateStore::portable()?, params)
}

pub fn skill_usage_scan(params: &Value) -> Result<Value> {
    usage::scan(&ClientStateStore::portable()?, params)
}

pub fn skill_delete_plan(params: &Value) -> Result<Value> {
    delete::plan(&ClientStateStore::portable()?, params)
}

pub fn skill_delete_apply(params: &Value) -> Result<Value> {
    delete::apply(&ClientStateStore::portable()?, params)
}

#[cfg(test)]
mod tests;
