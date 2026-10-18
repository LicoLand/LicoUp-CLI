mod action_catalog;
mod activation;
mod dispatch_context;
mod dispatch_router;
mod feature_status;
mod fixture_envelope;
mod fixture_file;
mod fixture_lifecycle;
mod fixture_payload;
mod fixture_trust;
mod protected_operation;
mod redacted_error;
mod request_validation;

pub use action_catalog::MOBILE_RELAY_NATIVE_ACTIONS;
pub use activation::{DeviceTrustLifecycle, apply_device_trust_lifecycle};
pub use dispatch_context::{
    dispatch_json_with_files_dir, dispatch_json_with_files_dir_and_pairwise_secret_store,
};
pub use dispatch_router::{dispatch_json, dispatch_request};
pub use feature_status::{
    EXPECTED_FEATURES, FEATURE_COMMAND_POLICY, FEATURE_CONTENT_CRYPTO, FEATURE_DEVICE_TRUST,
    FEATURE_ENVELOPE_VALIDATION, FEATURE_LIFECYCLE_SERVICE_ACTIONS, FEATURE_MLS_RUNTIME,
    FEATURE_PAIRWISE_RUNTIME, FEATURE_PROTOCOL_STATUS, runtime_feature_flags,
    runtime_protocol_hash, runtime_self_test,
};

#[cfg(test)]
mod tests;
