//! Stable composition root for semantic message projection and privacy-preserving redaction.

mod antigravity;
mod generated_context;
mod json_extract;
mod projection;
mod semantic;
mod structured_privacy;

pub(in crate::domain::conversation::history) use antigravity::{
    extract_user_request as extract_antigravity_user_request,
    strip_artifact_noise as strip_antigravity_artifact_noise,
};
pub(in crate::domain::conversation::history) use generated_context::{
    background_context_prompt_text, extract_user_image_attachments, generated_control_text,
    normalize_generated_metadata_message, strip_generated_context_blocks,
};
pub(in crate::domain::conversation::history) use json_extract::{
    extract_native_model, extract_native_session_id, extract_role, extract_text, extract_timestamp,
    find_string,
};
pub(in crate::domain::conversation::history) use projection::{
    clean_native_message_text, native_history_message_id, native_message_timestamp,
    plain_history_message, structured_history_message,
};
pub(in crate::domain::conversation::history) use semantic::{
    HistoryMessageKind, history_message_kind_from_semantic, normalize_history_message_semantic,
};

#[cfg(test)]
mod tests;
