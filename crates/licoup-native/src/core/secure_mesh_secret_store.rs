//! Platform-neutral secret-custody contract for Secure Client Mesh.
//!
//! Cryptographic and persistence code depends on this port. Platform keychains,
//! biometric contexts, and in-memory test stores implement it in the outer layer.

// Linux exports this contract for cross-platform callers while native presence
// authorization remains unavailable and fail-closed.
#[cfg_attr(target_os = "linux", allow(dead_code))]
mod authorization;
mod handle;
mod port;

pub use authorization::{
    MAX_SECRET_STORE_PRESENCE_GRANT_TTL, PresenceDecision, SecretStoreApprovedPresenceBatch,
    SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession, SecretStoreCallerChannel,
    SecretStoreConsumedPresence, SecretStoreKeyClass, SecretStoreOperation,
    SecretStorePresenceBatchRequest, SecretStorePresenceError, SecretStorePresenceGrant,
    SecretStorePresenceNonce, SecretStorePresenceProvider, SecretStorePresencePurpose,
    SecretStorePresenceScope,
};
pub use handle::SecretStoreHandle;
pub use port::SecureMeshSecretStore;
// The zeroizing secret buffer lives in `licoup-foundation` so the Gateway
// model and the custody backends share one owner.
#[cfg(test)]
pub use licoup_foundation::core::secret_bytes::SecretZeroizeProbe;
pub use licoup_foundation::core::secret_bytes::{MAX_SECRET_BYTES, SecretBytes, SecretBytesError};

#[cfg(target_os = "macos")]
pub(crate) use authorization::{derive_presence_binding_digest, digest_matches};

#[cfg(test)]
mod tests;
