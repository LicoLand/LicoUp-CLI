mod activation;
mod authorization;
mod codec;
mod decision;
mod identity;
mod input;
mod model;
mod policy;
mod record;
mod signature;
mod verification;

pub const SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION: &str = "licomesh.secure-mesh.device-trust.v2";
pub const SECURE_MESH_DEVICE_TRUST_STATUS: &str = "fingerprint_60_digit_safety_number_qr_policy_cli_gui_available_cross_signing_diagnostic_only_requires_durable_epoch_and_revocation_validation";

const DEVICE_IDENTITY_MAGIC: &[u8] = b"LCOSM-DID-v1";
const CROSS_SIGNATURE_MAGIC: &[u8] = b"LCOSM-XSG-v1";
const TRUST_RECORD_MAGIC: &[u8] = b"LCOSM-TRR-v1";
const SAS_MAGIC: &[u8] = b"LCOSM-SAFETY-NUMBER-v2";
const QR_MAGIC: &[u8] = b"LCOSM-QR-v2";
const PUBLIC_KEY_LEN: usize = 32;
const SAFETY_NUMBER_CHUNK_COUNT: usize = 12;
const SAFETY_NUMBER_DIGITS_PER_CHUNK: usize = 5;
const SAFETY_NUMBER_CHUNK_MODULUS: u32 = 100_000;

pub use activation::{
    AuthorityPosition, DeviceActivationLookup, DeviceActivationRefusal, DeviceActivationRequest,
    DeviceActivationState, DeviceCleanupIntent, DurableDeviceActivation,
    MAX_SECURE_MESH_AUTHORITY_STATE_BYTES, MAX_SECURE_MESH_DEVICE_ACTIVATIONS,
    SECURE_MESH_DEVICE_ACTIVATION_RECORD, SECURE_MESH_DEVICE_ACTIVATION_SCHEMA_VERSION,
    SecureMeshDeviceActivationStore, activation_authority_digest, device_activation_ledger_path,
    device_activation_request_from_json,
};
pub use authorization::{
    ProtectedSendAuthorization, ProtectedSendPayloadKind, authorize_protected_send,
    authorize_protected_send_from_trust_record,
};
pub use identity::{DeviceTrustPublicIdentity, detect_identity_key_change};
pub use model::{DeviceCrossSignature, DeviceTrustRecord, DeviceTrustState};
pub use policy::{
    evaluate_device_trust_lifecycle_json, evaluate_device_trust_policy_json,
    evaluate_device_trust_verification_json,
};
pub use record::{
    device_trust_record_from_json, device_trust_record_to_json, verify_device_trust_record_json,
};
pub use signature::{
    sign_device_cross_signature, sign_device_trust_record, verify_device_cross_signature,
    verify_device_trust_record,
};
pub use verification::{qr_verification_payload, sas_decimal_chunks};

#[cfg(test)]
mod tests;
