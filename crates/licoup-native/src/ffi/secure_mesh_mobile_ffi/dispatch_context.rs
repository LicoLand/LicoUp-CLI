use std::{path::PathBuf, sync::Arc};

use serde_json::Value;

use super::{dispatch_router::dispatch_json, request_validation::validate_ffi_request_bytes};
use crate::platform::secure_mesh_secret_store::SecureMeshSecretStore;

pub fn dispatch_json_with_files_dir(
    request_json: &str,
    files_dir: &str,
    unsupported_code: &'static str,
) -> anyhow::Result<Value> {
    validate_ffi_request_bytes(request_json)?;
    let request = serde_json::from_str::<Value>(request_json)?;
    let portable_dir = PathBuf::from(files_dir).join("portable-data");
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable_dir));
    let result = dispatch_json(&request, unsupported_code);
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    result
}

pub fn dispatch_json_with_files_dir_and_pairwise_secret_store(
    request_json: &str,
    files_dir: &str,
    unsupported_code: &'static str,
    pairwise_secret_store: Arc<dyn SecureMeshSecretStore>,
) -> anyhow::Result<Value> {
    validate_ffi_request_bytes(request_json)?;
    let request = serde_json::from_str::<Value>(request_json)?;
    let portable_dir = PathBuf::from(files_dir).join("portable-data");
    let previous =
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(portable_dir));
    let mobile_relay_secret_store = Arc::clone(&pairwise_secret_store);
    let result = crate::domain::mobile_relay::with_pairwise_secret_store_override(
        pairwise_secret_store,
        || {
            crate::domain::mobile_relay::with_mobile_relay_secret_store_override(
                mobile_relay_secret_store,
                || dispatch_json(&request, unsupported_code),
            )
        },
    );
    licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    result
}
