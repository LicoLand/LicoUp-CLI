//! Frozen continuous-collaboration domain ports and admission.
//!
//! Public DTOs are generated from `schemas/client_bridge/conversation.json`
//! (`continuousAssistant`). This module does not own Event history or a second
//! database. M1 durable-state owns SQL and reducers behind the frozen ports.

pub mod admission;
pub mod commit;
pub mod error;
pub mod generated;
pub mod hooks;
pub mod lifecycle;
pub mod migrate;
pub mod persist;
pub mod ports;
pub mod store_ports;
pub mod turn_response;
pub mod unavailable;
pub mod workflow_notices;

pub use crate::store::ContinuityUnitOfWork;
pub use admission::{
    ContinuityCommitmentAdmission, OWNER_HANDOFF_REVIEW_POLICY, admit_card_anchor,
    admit_card_identity_stable, admit_commitment_admission, admit_completion_transition,
    admit_composition_request, admit_goal_progress, admit_idempotency, admit_page_limit,
    admit_parent_context_grant, admit_sibling_card_order, admit_source_ref,
    admit_task_child_admission, admit_task_relation, admit_utf8_span, admit_versions,
    retain_current_criterion_evidence, wake_repeats_recorded_sources,
};
pub use commit::{
    CHILD_WORK_ACCEPTED_DESIGNATION, CHILD_WORK_INTENT_DESIGNATION, CHILD_WORK_LIVE_DESIGNATION,
    CHILD_WORK_PENDING_DESIGNATION, CHILD_WORK_STARTED_DESIGNATION, ChildWorkIdentity,
    CompletionNoticeView, INGRESS_USER_POSTED_DESIGNATION, PENDING_OBLIGATION_PAGE_SIZE,
    SETTLEMENT_APPLIED_DESIGNATION, SETTLEMENT_PENDING_DESIGNATION, accept_completion,
    ack_completion_notices, admit_evaluation_corpus, admit_evaluation_session,
    append_criterion_evidence, apply_goal_control, begin_collection_invocation,
    bump_host_generation, bump_revocation, child_work_accepted, child_work_identity_from_payload,
    child_work_identity_payload, child_work_named_key, child_work_operation_id, child_work_started,
    claim_collection_operation, clear_child_work_live, commit_collected_qualification,
    commit_user_posted_proposal, consume_logical_wake, count_cancel_effects,
    current_host_generation, derived_live_count, enqueue_review_wake, find_admitted_parent_grant,
    hand_off_conversation_goals, hand_off_goal_owner, ingress_execution_recorded,
    list_all_parent_grants, list_all_pending_wakes, list_child_work_live,
    list_completion_notification_ids, list_due_goals, list_pending_completion_notices,
    list_qualification_evidence, list_unacked_child_work, list_unacked_child_work_page,
    list_unapplied_settlements, list_unapplied_settlements_page, list_unknown_effect_ids,
    load_adoption_policy_values, load_effect_status, load_evaluation_corpus,
    load_evaluation_session, load_qualification_evidence_for, pending_obligation_scan_evidence,
    persist_adoption_enabled, persist_adoption_stage, persist_evaluation_session, put_agreement,
    put_derived, put_effect, put_grant, put_qualification_evidence, read_agreements,
    read_child_links, read_child_work_accepted, read_child_work_intent, read_child_work_live,
    read_goal, read_goal_bundle, read_oldest_pending_child_work, read_pending_outbox,
    read_pending_wakes, read_qualification_invalidations, read_relation_for_child,
    read_settlement_applied, read_settlement_pending, record_child_work_accepted,
    record_child_work_intent, record_child_work_live, record_child_work_started,
    record_ingress_execution, record_qualification_invalidation, record_settlement_applied,
    record_settlement_pending, release_collection_operation, replay_effect,
    resolve_completion_notice, resolve_stored_owner_authority, revoke_source, schedule_goal_due,
    settlement_applied, source_is_revoked_now, update_wake_host_generation,
};
pub use generated::*;
pub use hooks::{
    ContinuityEffectStatus, ContinuityInterrupt, continuity_now_ms, set_continuity_clock,
    set_continuity_interrupt,
};
pub use lifecycle::ContinuityGoalEvent;
pub use persist::{
    EvaluationCasePolarity, EvaluationExpectedAction, StoredEvaluationCase, StoredEvaluationCorpus,
    StoredEvaluationSession, StoredOwnerAuthority, evaluation_corpus_version_digest,
};
pub use ports::*;
pub use store_ports::*;
pub use turn_response::{
    ASSISTANT_TURN_INVALID_ERROR, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN,
    apply_admitted_validation_failure_facts, decode_assistant_turn_response,
    is_assistant_turn_response_mode, project_admitted_known_text_fields,
    public_admitted_failure_payload, public_admitted_output, published_envelope,
    published_terminal_envelope, redact_live_runtime_event,
    redact_live_runtime_event_with_assembly, trusted_response_mode_from_metadata,
    trusted_response_mode_metadata, usable_reply_text,
};
pub use unavailable::*;
