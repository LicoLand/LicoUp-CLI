mod capability_binding;
mod handshake_machine;
mod input_validation;
mod key_schedule;
mod transcript_codec;

pub use capability_binding::secure_mesh_pairwise_build_protocol_digest;
#[cfg(test)]
pub(crate) use capability_binding::secure_mesh_pairwise_test_capability_evaluation;
#[cfg(test)]
pub(super) use capability_binding::{
    capability_proof_request, capability_verification_context,
    secure_mesh_pairwise_build_protocol_digest_for_revision,
};
#[cfg(test)]
pub(super) use input_validation::{ensure_intro, ensure_local_identity_key_material};
pub(super) use key_schedule::{collect_pqxdh_classical_secret, derive_initial_keys};
#[cfg(test)]
pub(super) use key_schedule::{
    derive_pqxdh_classical_initiator_secret, derive_pqxdh_classical_responder_secret,
};
pub(super) use transcript_codec::decode_fixed_base64url;
pub use transcript_codec::{
    SecureMeshPairwiseSessionAccepted, SecureMeshPairwiseSessionFinished,
    SecureMeshPairwiseSessionIntro,
};
#[cfg(test)]
pub(super) use transcript_codec::{
    accept_signature_payload, derive_session_id, intro_signature_payload,
    pairwise_key_confirmation, sign_pairwise_transcript,
};
