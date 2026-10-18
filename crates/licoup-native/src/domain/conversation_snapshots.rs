//! Stable public facade for native conversation snapshot and archive operations.

use anyhow::Result;
use serde_json::Value;

pub fn root_get(params: &Value) -> Result<Value> {
    super::conversation::snapshots::root_get(params)
}

pub fn root_set(params: &Value) -> Result<Value> {
    super::conversation::snapshots::root_set(params)
}

pub fn collections_list(params: &Value) -> Result<Value> {
    super::conversation::snapshots::collections_list(params)
}

pub fn profiles_list(params: &Value) -> Result<Value> {
    super::conversation::snapshots::profiles_list(params)
}

pub fn profile_get(params: &Value) -> Result<Value> {
    super::conversation::snapshots::profile_get(params)
}

pub fn profile_import(params: &Value) -> Result<Value> {
    super::conversation::snapshots::profile_import(params)
}

pub fn archive_run(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_run(params)
}

pub fn archive_verify(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_verify(params)
}

pub fn archive_report(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_report(params)
}

pub fn archive_collect(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_collect(params)
}

pub(crate) fn archive_selection_preview(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_selection_preview(params)
}

pub(crate) fn archive_selection_collect(params: &Value) -> Result<Value> {
    super::conversation::snapshots::archive_selection_collect(params)
}

pub fn collect(params: &Value) -> Result<Value> {
    super::conversation::snapshots::collect(params)
}
