use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use crate::ffi::generated::secure_mesh::{
    SECURE_MESH_MAX_COLLECTION_ENTRIES, SECURE_MESH_MAX_DEPTH, SECURE_MESH_MAX_NODES,
    SECURE_MESH_MAX_REQUEST_BYTES, SECURE_MESH_MAX_STRING_BYTES,
};

pub(super) const MAX_FFI_REQUEST_BYTES: usize = SECURE_MESH_MAX_REQUEST_BYTES;
pub(super) const MAX_FFI_JSON_DEPTH: usize = SECURE_MESH_MAX_DEPTH;
pub(super) const MAX_FFI_JSON_NODES: usize = SECURE_MESH_MAX_NODES;
pub(super) const MAX_FFI_OBJECT_FIELDS: usize = SECURE_MESH_MAX_COLLECTION_ENTRIES;
pub(super) const MAX_FFI_ARRAY_ITEMS: usize = SECURE_MESH_MAX_COLLECTION_ENTRIES;
pub(super) const MAX_FFI_STRING_BYTES: usize = SECURE_MESH_MAX_STRING_BYTES;

pub(super) fn validate_ffi_request_bytes(request_json: &str) -> Result<()> {
    ensure!(
        request_json.len() <= MAX_FFI_REQUEST_BYTES,
        "secure mesh native request exceeds the byte limit"
    );
    Ok(())
}

pub(super) fn validate_ffi_json_structure(request: &Value) -> Result<()> {
    let mut stack = vec![(request, 1usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        ensure!(
            depth <= MAX_FFI_JSON_DEPTH,
            "secure mesh native request exceeds the JSON depth limit"
        );
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| anyhow!("secure mesh native request node count overflow"))?;
        ensure!(
            nodes <= MAX_FFI_JSON_NODES,
            "secure mesh native request exceeds the JSON node limit"
        );
        match value {
            Value::String(value) => ensure!(
                value.len() <= MAX_FFI_STRING_BYTES,
                "secure mesh native request contains an oversized string"
            ),
            Value::Array(values) => {
                ensure!(
                    values.len() <= MAX_FFI_ARRAY_ITEMS,
                    "secure mesh native request contains an oversized array"
                );
                for value in values.iter().rev() {
                    stack.push((value, depth.saturating_add(1)));
                }
            }
            Value::Object(values) => {
                ensure!(
                    values.len() <= MAX_FFI_OBJECT_FIELDS,
                    "secure mesh native request contains an oversized object"
                );
                for (key, value) in values.iter().rev() {
                    ensure!(
                        key.len() <= MAX_FFI_STRING_BYTES,
                        "secure mesh native request contains an oversized object key"
                    );
                    stack.push((value, depth.saturating_add(1)));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(())
}
