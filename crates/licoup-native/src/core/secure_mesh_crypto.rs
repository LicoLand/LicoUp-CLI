mod aad_binding;
mod constants;
mod content_key;
mod frame_codec;
mod header_codec;
mod key_derivation;
mod length_codec;
mod model;
mod padding;
mod private_context;
mod public_payload;
mod validation;

pub use constants::{SECURE_MESH_CONTENT_CIPHER_SUITE, SECURE_MESH_CONTENT_CRYPTO_STATUS};
pub use content_key::ContentKey;
pub use model::{
    OpenedSecureMeshPayload, SealedSecureMeshPayload, SecureMeshContentContext,
    SecureMeshPayloadKind, SecureMeshPlaintext,
};
#[allow(unused_imports)]
pub(crate) use model::{
    OpenedSecureMeshPrivateContextPayload, SealedSecureMeshPrivateContextPayload,
};
pub(crate) use private_context::{open_private_context_payload, seal_private_context_payload};
pub use public_payload::{
    open_payload, open_payload_with_aad_binding, seal_payload, seal_payload_with_aad_binding,
};

#[cfg(test)]
mod tests;
