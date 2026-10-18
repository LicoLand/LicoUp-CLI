use super::admission::{admit_page_limit, admit_parent_context_grant, admit_task_relation};
use super::error::{continuity_failure, sql_failure};
use super::generated::{
    ContinuityAgreement, ContinuityCandidateIdentity, ContinuityEffectClass, ContinuityEvidenceRef,
    ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage, ContinuityFollowThroughKind,
    ContinuityGoalCompletionTransition, ContinuityGoalContract, ContinuityGoalControl,
    ContinuityGoalLifecycle, ContinuityGoalProgress, ContinuityMatter, ContinuityMatterAssociation,
    ContinuityMatterStatus, ContinuityNextAttention, ContinuityParentContextGrant,
    ContinuityParentGrantBasis, ContinuityParentGrantStatus, ContinuitySourceRef,
    ContinuityTaskConversationRelation, ContinuityTaskListingKind, ContinuityWake,
    ContinuityWriteEnvelope,
};
use super::hooks::{ContinuityEffectStatus, continuity_now_ms};
use crate::continuity::ports::ContinuityCommitReceipt;
use crate::store::ContinuityUnitOfWork;
use rusqlite::OptionalExtension;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PersistReceipt {
    conversation_id: String,
    revision: i64,
    effect_class: ContinuityEffectClass,
}

impl From<&ContinuityCommitReceipt> for PersistReceipt {
    fn from(receipt: &ContinuityCommitReceipt) -> Self {
        Self {
            conversation_id: receipt.conversation_id.clone(),
            revision: receipt.revision,
            effect_class: receipt.effect_class,
        }
    }
}

impl From<PersistReceipt> for ContinuityCommitReceipt {
    fn from(receipt: PersistReceipt) -> Self {
        ContinuityCommitReceipt {
            conversation_id: receipt.conversation_id,
            revision: receipt.revision,
            effect_class: receipt.effect_class,
        }
    }
}

pub fn new_continuity_id(prefix: &str) -> String {
    format!("{prefix}:{}", Uuid::new_v4())
}

pub fn encode_json<T: Serialize>(value: &T) -> Result<String, ContinuityFailure> {
    serde_json::to_string(value).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        )
    })
}

pub fn decode_json<T: DeserializeOwned>(raw: &str) -> Result<T, ContinuityFailure> {
    serde_json::from_str(raw).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        )
    })
}

pub fn ensure_scope(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<(i64, i64), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_scope(conversation_id, revocation_generation, acl_generation)
         VALUES (?1, 0, 0)
         ON CONFLICT(conversation_id) DO NOTHING",
        [conversation_id],
    )
    .map_err(sql_failure)?;
    unit.query_row(
        "SELECT revocation_generation, acl_generation FROM continuity_scope WHERE conversation_id=?1",
        [conversation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(sql_failure)
}

pub fn load_idempotency(
    unit: &ContinuityUnitOfWork<'_>,
    request_id: &str,
) -> Result<Option<(Value, ContinuityCommitReceipt)>, ContinuityFailure> {
    let row: Option<(String, String)> = unit
        .query_row(
            "SELECT payload, receipt FROM continuity_idempotency WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_failure)?;
    match row {
        Some((payload, receipt)) => {
            let stored: PersistReceipt = decode_json(&receipt)?;
            Ok(Some((decode_json(&payload)?, stored.into())))
        }
        None => Ok(None),
    }
}

pub fn store_idempotency(
    unit: &ContinuityUnitOfWork<'_>,
    request_id: &str,
    conversation_id: &str,
    payload: &Value,
    receipt: &ContinuityCommitReceipt,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_idempotency(request_id, conversation_id, payload, receipt)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            request_id,
            conversation_id,
            encode_json(payload)?,
            encode_json(&PersistReceipt::from(receipt))?
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub const INGRESS_USER_POSTED_DESIGNATION: &str = "user-posted";
pub const SETTLEMENT_PENDING_DESIGNATION: &str = "settlement-pending";
pub const SETTLEMENT_APPLIED_DESIGNATION: &str = "settlement-applied";
pub const CHILD_WORK_STARTED_DESIGNATION: &str = "child-work-started";
pub const CHILD_WORK_INTENT_DESIGNATION: &str = "child-work-intent";
pub const CHILD_WORK_ACCEPTED_DESIGNATION: &str = "child-work-accepted";
pub const CHILD_WORK_PENDING_DESIGNATION: &str = "child-work-pending";
pub const CHILD_WORK_LIVE_DESIGNATION: &str = "child-work-live";
pub const PENDING_OBLIGATION_PAGE_SIZE: u64 = 50;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildWorkIdentity {
    pub parent_conversation_id: String,
    pub child_conversation_id: String,
    pub goal_id: String,
    pub membership_id: String,
    pub admitted_revision: i64,
    pub work_generation: i64,
    pub operation_id: String,
    pub dispatch_id: Option<String>,
    pub native_turn_id: Option<String>,
}

pub fn ingress_interpretation_key(recipient_membership_id: &str, designation: &str) -> String {
    format!("ingress:{recipient_membership_id}:{designation}")
}

pub fn settlement_interpretation_key(settlement_id: &str, designation: &str) -> String {
    format!("settlement:{settlement_id}:{designation}")
}

pub fn child_work_interpretation_key(goal_id: &str, revision: i64) -> String {
    format!("work:{goal_id}:{revision}")
}

pub fn child_work_named_key(goal_id: &str, revision: i64, designation: &str) -> String {
    format!("work:{goal_id}:{revision}:{designation}")
}

pub fn child_work_operation_id(goal_id: &str, revision: i64) -> String {
    format!("dispatch:child-work:{goal_id}:{revision}")
}

pub fn child_work_live_key(goal_id: &str) -> String {
    format!("work:{goal_id}:{CHILD_WORK_LIVE_DESIGNATION}")
}

pub fn child_work_identity_from_payload(
    payload: &Value,
    parent_conversation_id: &str,
    goal_id: &str,
) -> Option<ChildWorkIdentity> {
    let operation_id = payload
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let child_conversation_id = payload
        .get("childConversationId")
        .or_else(|| payload.get("conversationId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let membership_id = payload
        .get("membershipId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_owned();
    let admitted_revision = payload
        .get("admittedRevision")
        .or_else(|| payload.get("revision"))
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)?;
    let work_generation = payload
        .get("workGeneration")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 1)
        .unwrap_or_else(|| admitted_revision.max(1));
    let parent = payload
        .get("parentConversationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(parent_conversation_id)
        .to_owned();
    let dispatch_id = payload
        .get("dispatchId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "turn:accepted")
        .map(str::to_owned);
    let native_turn_id = payload
        .get("nativeTurnId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(ChildWorkIdentity {
        parent_conversation_id: parent,
        child_conversation_id,
        goal_id: goal_id.to_owned(),
        membership_id,
        admitted_revision,
        work_generation,
        operation_id,
        dispatch_id,
        native_turn_id,
    })
}

pub fn child_work_identity_payload(identity: &ChildWorkIdentity, kind: &str) -> Value {
    let mut payload = json!({
        "kind": kind,
        "goalId": identity.goal_id,
        "revision": identity.admitted_revision,
        "admittedRevision": identity.admitted_revision,
        "workGeneration": identity.work_generation,
        "childConversationId": identity.child_conversation_id,
        "conversationId": identity.child_conversation_id,
        "membershipId": identity.membership_id,
        "parentConversationId": identity.parent_conversation_id,
        "operationId": identity.operation_id,
    });
    if let Some(dispatch_id) = &identity.dispatch_id {
        payload["dispatchId"] = json!(dispatch_id);
    }
    if let Some(native_turn_id) = &identity.native_turn_id {
        payload["nativeTurnId"] = json!(native_turn_id);
    }
    payload
}

pub fn child_recipient_membership(
    unit: &ContinuityUnitOfWork<'_>,
    child_conversation_id: &str,
) -> Result<Option<String>, ContinuityFailure> {
    let assistant: Option<String> = unit
        .query_row(
            "SELECT assistant_membership_id FROM conversations WHERE id=?1",
            [child_conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(sql_failure)?
        .flatten();
    if assistant
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(assistant);
    }
    unit.query_row(
        "SELECT m.id FROM memberships m
         JOIN principals p ON p.id=m.principal_id
         WHERE m.conversation_id=?1 AND m.status='active' AND p.kind='agent'
         ORDER BY m.joined_at ASC, m.id ASC LIMIT 1",
        [child_conversation_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_failure)
}

pub fn write_source_cursor(
    unit: &ContinuityUnitOfWork<'_>,
    envelope: &ContinuityWriteEnvelope,
) -> Result<(), ContinuityFailure> {
    let source_event = envelope
        .source_event_refs
        .first()
        .map(|source| source.opaque_id.as_str())
        .unwrap_or("none");
    unit.execute(
        "INSERT INTO continuity_source_cursors(
           conversation_id, source_event_id, interpretation_key, payload
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(conversation_id, source_event_id, interpretation_key)
         DO UPDATE SET payload=excluded.payload",
        rusqlite::params![
            envelope.conversation_id,
            source_event,
            envelope.request_id,
            encode_json(envelope)?
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

/// Durable ingress identity: event + recipient + designation.
/// This is not the model-generated proposal `requestId`.
pub fn write_ingress_execution(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    recipient_membership_id: &str,
    designation: &str,
) -> Result<(), ContinuityFailure> {
    let key = ingress_interpretation_key(recipient_membership_id, designation);
    unit.execute(
        "INSERT INTO continuity_source_cursors(
           conversation_id, source_event_id, interpretation_key, payload
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(conversation_id, source_event_id, interpretation_key)
         DO UPDATE SET payload=excluded.payload",
        rusqlite::params![
            conversation_id,
            source_event_id,
            key,
            encode_json(&json!({
                "kind": "ingress",
                "recipientMembershipId": recipient_membership_id,
                "designation": designation,
            }))?
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn load_ingress_execution(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    recipient_membership_id: &str,
    designation: &str,
) -> Result<bool, ContinuityFailure> {
    let key = ingress_interpretation_key(recipient_membership_id, designation);
    let found: i64 = unit
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM continuity_source_cursors
               WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3
             )",
            rusqlite::params![conversation_id, source_event_id, key],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    Ok(found != 0)
}

pub fn upsert_matter(
    unit: &ContinuityUnitOfWork<'_>,
    matter: &ContinuityMatter,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_matters(
           id, conversation_id, revision, label, association_refs, created_event, status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
           revision=excluded.revision,
           label=excluded.label,
           association_refs=excluded.association_refs,
           status=excluded.status
         WHERE continuity_matters.deleted_at IS NULL",
        rusqlite::params![
            matter.id,
            matter.conversation_id,
            matter.revision,
            matter.label,
            encode_json(&matter.association_refs)?,
            encode_json(&matter.created_event)?,
            match matter.status {
                ContinuityMatterStatus::Open => "open",
                ContinuityMatterStatus::Archived => "archived",
            }
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn upsert_association(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    interpretation_key: &str,
    association: &ContinuityMatterAssociation,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_matter_associations(
           conversation_id, source_event_id, interpretation_key, matter_id,
           association_revision, proposed_by, reason_code, source_ref, supersedes
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(conversation_id, source_event_id, interpretation_key)
         DO UPDATE SET
           matter_id=excluded.matter_id,
           association_revision=excluded.association_revision,
           source_ref=excluded.source_ref,
           supersedes=excluded.supersedes",
        rusqlite::params![
            conversation_id,
            association.source_ref.opaque_id,
            interpretation_key,
            association.matter_id,
            association.association_revision,
            association.proposed_by,
            association.reason_code,
            encode_json(&association.source_ref)?,
            association.supersedes
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn insert_agreement(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    agreement: &ContinuityAgreement,
) -> Result<(), ContinuityFailure> {
    if let Some(previous) = agreement.supersedes {
        unit.execute(
            "UPDATE continuity_agreements
             SET superseded_by=?2
             WHERE conversation_id=?1 AND effective_revision=?3 AND superseded_by IS NULL",
            rusqlite::params![conversation_id, agreement.id, previous],
        )
        .map_err(sql_failure)?;
    }
    unit.execute(
        "INSERT INTO continuity_agreements(
           id, conversation_id, scope, statement_ref, origin, effective_revision,
           supersedes, valid_from, valid_until, revocation_generation
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            agreement.id,
            conversation_id,
            match agreement.scope {
                super::ContinuityAgreementScope::Conversation => "conversation",
                super::ContinuityAgreementScope::Matter => "matter",
                super::ContinuityAgreementScope::Goal => "goal",
            },
            encode_json(&agreement.statement_ref)?,
            match agreement.origin {
                super::ContinuityAgreementOrigin::UserExplicit => "user-explicit",
                super::ContinuityAgreementOrigin::SourceFact => "source-fact",
                super::ContinuityAgreementOrigin::AgentInference => "agent-inference",
            },
            agreement.effective_revision,
            agreement.supersedes,
            agreement.valid_from,
            agreement.valid_until,
            agreement.revocation_generation
        ],
    )
    .map_err(sql_failure)?;
    invalidate_derived(unit, conversation_id)?;
    Ok(())
}

pub fn goal_conversation_id(
    unit: &ContinuityUnitOfWork<'_>,
    goal_id: &str,
) -> Result<Option<String>, ContinuityFailure> {
    unit.query_row(
        "SELECT conversation_id FROM continuity_goals
         WHERE goal_id=?1 AND deleted_at IS NULL",
        [goal_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_failure)
}

pub fn load_goal(
    unit: &ContinuityUnitOfWork<'_>,
    goal_id: &str,
) -> Result<Option<(ContinuityGoalContract, ContinuityGoalProgress)>, ContinuityFailure> {
    let row: Option<(String, String)> = unit
        .query_row(
            "SELECT contract, progress FROM continuity_goals
             WHERE goal_id=?1 AND deleted_at IS NULL",
            [goal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_failure)?;
    match row {
        Some((contract, progress)) => Ok(Some((decode_json(&contract)?, decode_json(&progress)?))),
        None => Ok(None),
    }
}

pub fn upsert_goal(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    contract: &ContinuityGoalContract,
    progress: &ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    let next_due = due_from_attention(&progress.next_attention);
    unit.execute(
        "INSERT INTO continuity_goals(
           goal_id, conversation_id, matter_id, contract, progress, lifecycle, control,
           revision, next_due
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(goal_id) DO UPDATE SET
           contract=excluded.contract,
           progress=excluded.progress,
           lifecycle=excluded.lifecycle,
           control=excluded.control,
           revision=excluded.revision,
           next_due=excluded.next_due
         WHERE continuity_goals.deleted_at IS NULL",
        rusqlite::params![
            progress.goal_id,
            conversation_id,
            contract.matter_id,
            encode_json(contract)?,
            encode_json(progress)?,
            lifecycle_wire(progress.lifecycle),
            control_wire(progress.control),
            progress.revision,
            next_due
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn enqueue_wake(
    unit: &ContinuityUnitOfWork<'_>,
    wake: &ContinuityWake,
    conversation_id: &str,
) -> Result<bool, ContinuityFailure> {
    // Distinct logical wakes stay distinct rows: original events are never
    // merged away at enqueue. Consumption coalesces same-goal wakes into one
    // logical advancement; a duplicate notification (same logical wake id) is
    // idempotent and does not create a second logical delivery.
    let changed = unit
        .execute(
            "INSERT OR IGNORE INTO continuity_outbox(
               logical_wake_id, conversation_id, goal_id, payload, created_at, settlement
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            rusqlite::params![
                wake.logical_wake_id,
                conversation_id,
                wake.goal_id,
                encode_json(wake)?,
                continuity_now_ms()
            ],
        )
        .map_err(sql_failure)?;
    Ok(changed > 0)
}

pub fn consume_wake(
    unit: &ContinuityUnitOfWork<'_>,
    logical_wake_id: &str,
) -> Result<bool, ContinuityFailure> {
    let changed = unit
        .execute(
            "UPDATE continuity_outbox
             SET consumed_at=?2, settlement='handed_off'
             WHERE logical_wake_id=?1 AND consumed_at IS NULL",
            rusqlite::params![logical_wake_id, continuity_now_ms()],
        )
        .map_err(sql_failure)?;
    Ok(changed > 0)
}

pub fn list_pending_wakes(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<Vec<ContinuityWake>, ContinuityFailure> {
    let payloads: Vec<String> = unit
        .query_vec(
            "SELECT payload FROM continuity_outbox
             WHERE conversation_id=?1 AND consumed_at IS NULL
             ORDER BY logical_wake_id ASC",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    payloads
        .iter()
        .map(|payload| decode_json(payload))
        .collect()
}

pub fn pending_outbox_count(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    unit.query_row(
        "SELECT COUNT(*) FROM continuity_outbox
         WHERE conversation_id=?1 AND consumed_at IS NULL",
        [conversation_id],
        |row| row.get(0),
    )
    .map_err(sql_failure)
}

pub fn record_effect(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: Option<&str>,
    logical_effect_id: &str,
    status: ContinuityEffectStatus,
) -> Result<ContinuityEffectStatus, ContinuityFailure> {
    let existing: Option<String> = unit
        .query_row(
            "SELECT status FROM continuity_effects WHERE logical_effect_id=?1",
            [logical_effect_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    if let Some(existing) = existing {
        let current = ContinuityEffectStatus::parse(&existing).ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityEffects,
            )
        })?;
        if current == ContinuityEffectStatus::Unknown && status != ContinuityEffectStatus::Unknown {
            // Reconcile may resolve unknown. Replay of a known-executed id is refused above.
        } else if current == ContinuityEffectStatus::Executed
            && status != ContinuityEffectStatus::Executed
        {
            return Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityEffects,
            ));
        } else if current == status {
            return Ok(current);
        }
    }
    unit.execute(
        "INSERT INTO continuity_effects(logical_effect_id, conversation_id, goal_id, status, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(logical_effect_id) DO UPDATE SET
           status=excluded.status,
           updated_at=excluded.updated_at",
        rusqlite::params![
            logical_effect_id,
            conversation_id,
            goal_id,
            status.as_str(),
            continuity_now_ms()
        ],
    )
    .map_err(sql_failure)?;
    Ok(status)
}

pub fn delete_effect_if_status(
    unit: &ContinuityUnitOfWork<'_>,
    logical_effect_id: &str,
    status: ContinuityEffectStatus,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "DELETE FROM continuity_effects WHERE logical_effect_id=?1 AND status=?2",
        rusqlite::params![logical_effect_id, status.as_str()],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn load_effect(
    unit: &ContinuityUnitOfWork<'_>,
    logical_effect_id: &str,
) -> Result<Option<ContinuityEffectStatus>, ContinuityFailure> {
    let row: Option<String> = unit
        .query_row(
            "SELECT status FROM continuity_effects WHERE logical_effect_id=?1",
            [logical_effect_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    row.map(|value| {
        ContinuityEffectStatus::parse(&value).ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityEffects,
            )
        })
    })
    .transpose()
}

pub fn has_unknown_effects(
    unit: &ContinuityUnitOfWork<'_>,
    goal_id: &str,
) -> Result<bool, ContinuityFailure> {
    let count: i64 = unit
        .query_row(
            "SELECT COUNT(*) FROM continuity_effects
             WHERE goal_id=?1 AND status='unknown'",
            [goal_id],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    Ok(count > 0)
}

pub fn increment_revocation(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    opaque_id: Option<&str>,
    deleted: bool,
) -> Result<i64, ContinuityFailure> {
    ensure_scope(unit, conversation_id)?;
    unit.execute(
        "UPDATE continuity_scope
         SET revocation_generation = revocation_generation + 1
         WHERE conversation_id=?1",
        [conversation_id],
    )
    .map_err(sql_failure)?;
    let generation: i64 = unit
        .query_row(
            "SELECT revocation_generation FROM continuity_scope WHERE conversation_id=?1",
            [conversation_id],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    if let Some(opaque_id) = opaque_id {
        unit.execute(
            "INSERT INTO continuity_source_revocations(
               conversation_id, opaque_id, revocation_generation, deleted
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(conversation_id, opaque_id) DO UPDATE SET
               revocation_generation=excluded.revocation_generation,
               deleted=excluded.deleted",
            rusqlite::params![conversation_id, opaque_id, generation, deleted as i64],
        )
        .map_err(sql_failure)?;
        revoke_parent_grants_covering_opaque(unit, conversation_id, opaque_id)?;
    }
    invalidate_derived(unit, conversation_id)?;
    Ok(generation)
}

fn revoke_parent_grants_covering_opaque(
    unit: &ContinuityUnitOfWork<'_>,
    source_conversation_id: &str,
    opaque_id: &str,
) -> Result<(), ContinuityFailure> {
    let mut after: Option<String> = None;
    loop {
        let page = list_child_relations(
            unit,
            source_conversation_id,
            after.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE,
        )?;
        let page_len = page.len();
        after = page
            .last()
            .map(|relation| relation.card_anchor.sequence.to_string());
        for relation in page {
            let Some(recipient) =
                child_recipient_membership(unit, &relation.child_conversation_id)?
            else {
                continue;
            };
            revoke_admitted_recipient_grants(
                unit,
                source_conversation_id,
                &relation.child_conversation_id,
                &recipient,
                |source| source.opaque_id == opaque_id,
            )?;
        }
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(())
}

pub fn revoke_admitted_recipient_grants(
    unit: &ContinuityUnitOfWork<'_>,
    source_conversation_id: &str,
    recipient_conversation_id: &str,
    recipient_membership_id: &str,
    mut covers: impl FnMut(&ContinuitySourceRef) -> bool,
) -> Result<(), ContinuityFailure> {
    let mut after: Option<String> = None;
    loop {
        let page = list_parent_grants(
            unit,
            recipient_conversation_id,
            recipient_membership_id,
            after.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE,
        )?;
        let page_len = page.len();
        after = page.last().map(|grant| grant.grant_id.clone());
        for grant in page {
            if grant.source_conversation_id != source_conversation_id
                || grant.status != ContinuityParentGrantStatus::Admitted
            {
                continue;
            }
            if !grant.source_refs.iter().any(&mut covers) {
                continue;
            }
            let mut revoked = grant;
            revoked.status = ContinuityParentGrantStatus::Revoked;
            upsert_grant(unit, &revoked)?;
        }
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(())
}

pub fn source_is_revoked(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source: &ContinuitySourceRef,
) -> Result<bool, ContinuityFailure> {
    if source.validity == super::ContinuitySourceValidity::Revoked {
        return Ok(true);
    }
    let row: Option<i64> = unit
        .query_row(
            "SELECT deleted FROM continuity_source_revocations
             WHERE conversation_id=?1 AND opaque_id=?2",
            rusqlite::params![conversation_id, source.opaque_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    Ok(row.is_some())
}

pub fn insert_derived(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    kind: &str,
    source_opaque_id: &str,
    body: &str,
    revocation_generation: i64,
) -> Result<String, ContinuityFailure> {
    let id = new_continuity_id("derived");
    unit.execute(
        "INSERT INTO continuity_derived(
           id, conversation_id, kind, source_opaque_id, body, revocation_generation
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            id,
            conversation_id,
            kind,
            source_opaque_id,
            body,
            revocation_generation
        ],
    )
    .map_err(sql_failure)?;
    Ok(id)
}

pub fn live_derived_count(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    unit.query_row(
        "SELECT COUNT(*) FROM continuity_derived
         WHERE conversation_id=?1 AND invalidated=0 AND deleted_at IS NULL",
        [conversation_id],
        |row| row.get(0),
    )
    .map_err(sql_failure)
}

fn invalidate_derived(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "UPDATE continuity_derived
         SET invalidated=1
         WHERE conversation_id=?1 AND deleted_at IS NULL",
        [conversation_id],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn current_agreements(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<Vec<ContinuityAgreement>, ContinuityFailure> {
    let rows = unit
        .query_vec(
            "SELECT id, scope, statement_ref, origin, effective_revision, supersedes,
                    valid_from, valid_until, revocation_generation
             FROM continuity_agreements
             WHERE conversation_id=?1
               AND superseded_by IS NULL
               AND deleted_at IS NULL
             ORDER BY effective_revision DESC, id DESC",
            [conversation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .map_err(sql_failure)?;
    let mut agreements = Vec::with_capacity(rows.len());
    for row in rows {
        agreements.push(ContinuityAgreement {
            id: row.0,
            scope: decode_scope(&row.1)?,
            statement_ref: decode_json(&row.2)?,
            origin: decode_origin(&row.3)?,
            effective_revision: row.4,
            supersedes: row.5,
            valid_from: row.6,
            valid_until: row.7,
            revocation_generation: row.8,
        });
    }
    Ok(agreements)
}

pub fn goals_for_matter(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    matter_id: &str,
) -> Result<Vec<String>, ContinuityFailure> {
    unit.query_vec(
        "SELECT goal_id FROM continuity_goals
         WHERE conversation_id=?1 AND matter_id=?2 AND deleted_at IS NULL
         ORDER BY goal_id ASC",
        rusqlite::params![conversation_id, matter_id],
        |row| row.get(0),
    )
    .map_err(sql_failure)
}

pub fn list_child_links(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, String, String)>, ContinuityFailure> {
    unit.query_vec(
        "SELECT child_conversation_id, parent_conversation_id, goal_id
         FROM continuity_task_relations
         ORDER BY card_sequence ASC, goal_id ASC",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(sql_failure)
}

pub fn list_matters(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<ContinuityMatter>, ContinuityFailure> {
    let rows = unit
        .query_vec(
            "SELECT id, conversation_id, revision, label, association_refs, created_event, status
             FROM continuity_matters
             WHERE conversation_id=?1
               AND deleted_at IS NULL
               AND (?2 IS NULL OR id > ?2)
             ORDER BY id ASC
             LIMIT ?3",
            rusqlite::params![conversation_id, after, limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|row| {
            Ok(ContinuityMatter {
                id: row.0,
                conversation_id: row.1,
                revision: row.2,
                label: row.3,
                association_refs: decode_json(&row.4)?,
                created_event: decode_json(&row.5)?,
                status: if row.6 == "archived" {
                    ContinuityMatterStatus::Archived
                } else {
                    ContinuityMatterStatus::Open
                },
            })
        })
        .collect()
}

pub fn load_relation(
    unit: &ContinuityUnitOfWork<'_>,
    goal_id: &str,
) -> Result<Option<ContinuityTaskConversationRelation>, ContinuityFailure> {
    let row = unit
        .query_row(
            "SELECT goal_id, parent_conversation_id, child_conversation_id, card_event_id,
                    card_sequence, card_part_id, listing_kind, follow_through_kind,
                    created_event, completion_transition, revision
             FROM continuity_task_relations WHERE goal_id=?1",
            [goal_id],
            relation_from_row,
        )
        .optional()
        .map_err(sql_failure)?;
    row.map(relation_from_parts).transpose()
}

pub fn load_relation_for_child(
    unit: &ContinuityUnitOfWork<'_>,
    child_conversation_id: &str,
) -> Result<Option<ContinuityTaskConversationRelation>, ContinuityFailure> {
    let row = unit
        .query_row(
            "SELECT goal_id, parent_conversation_id, child_conversation_id, card_event_id,
                    card_sequence, card_part_id, listing_kind, follow_through_kind,
                    created_event, completion_transition, revision
             FROM continuity_task_relations WHERE child_conversation_id=?1",
            [child_conversation_id],
            relation_from_row,
        )
        .optional()
        .map_err(sql_failure)?;
    row.map(relation_from_parts).transpose()
}

pub fn list_due_goals(
    unit: &ContinuityUnitOfWork<'_>,
    now: i64,
) -> Result<Vec<(String, String, i64)>, ContinuityFailure> {
    unit.query_vec(
        "SELECT goal_id, conversation_id, next_due
         FROM continuity_goals
         WHERE deleted_at IS NULL
           AND next_due IS NOT NULL
           AND next_due <= ?1
           AND lifecycle NOT IN ('achieved', 'cancelled', 'superseded')
         ORDER BY next_due ASC, goal_id ASC",
        [now],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map_err(sql_failure)
}

pub fn list_child_relations(
    unit: &ContinuityUnitOfWork<'_>,
    parent_conversation_id: &str,
    after: Option<&str>,
    limit: u64,
) -> Result<Vec<ContinuityTaskConversationRelation>, ContinuityFailure> {
    let limit = admit_page_limit(limit)?;
    let after_sequence = after.and_then(|cursor| cursor.parse::<i64>().ok());
    let rows = unit
        .query_vec(
            "SELECT goal_id, parent_conversation_id, child_conversation_id, card_event_id,
                    card_sequence, card_part_id, listing_kind, follow_through_kind,
                    created_event, completion_transition, revision
             FROM continuity_task_relations
             WHERE parent_conversation_id=?1
               AND (?2 IS NULL OR card_sequence > ?2)
             ORDER BY card_sequence ASC, goal_id ASC
             LIMIT ?3",
            rusqlite::params![parent_conversation_id, after_sequence, limit as i64],
            relation_from_row,
        )
        .map_err(sql_failure)?;
    rows.into_iter().map(relation_from_parts).collect()
}

pub fn upsert_relation(
    unit: &ContinuityUnitOfWork<'_>,
    relation: &ContinuityTaskConversationRelation,
    previous: Option<&ContinuityTaskConversationRelation>,
) -> Result<(), ContinuityFailure> {
    admit_task_relation(relation, previous)?;
    unit.execute(
        "INSERT INTO continuity_task_relations(
           goal_id, parent_conversation_id, child_conversation_id, card_event_id,
           card_sequence, card_part_id, listing_kind, follow_through_kind,
           created_event, completion_transition, revision
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(goal_id) DO UPDATE SET
           completion_transition=excluded.completion_transition,
           revision=excluded.revision",
        rusqlite::params![
            relation.goal_id,
            relation.parent_conversation_id,
            relation.child_conversation_id,
            relation.card_anchor.event_id,
            relation.card_anchor.sequence,
            relation.card_anchor.part_id,
            "child-task",
            "durable",
            encode_json(&relation.created_event)?,
            relation
                .completion_transition
                .as_ref()
                .map(encode_json)
                .transpose()?,
            relation.revision
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn list_parent_grants(
    unit: &ContinuityUnitOfWork<'_>,
    recipient_conversation_id: &str,
    recipient_membership_id: &str,
    after: Option<&str>,
    limit: u64,
) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
    let limit = admit_page_limit(limit)?;
    let rows = unit
        .query_vec(
            "SELECT grant_id, source_conversation_id, recipient_conversation_id,
                    recipient_membership_id, source_refs, authorized_scopes, status,
                    request_id, revocation_generation
             FROM continuity_parent_grants
             WHERE recipient_conversation_id=?1
               AND recipient_membership_id=?2
               AND (?3 IS NULL OR grant_id > ?3)
             ORDER BY grant_id ASC
             LIMIT ?4",
            rusqlite::params![
                recipient_conversation_id,
                recipient_membership_id,
                after,
                limit as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|row| {
            Ok(ContinuityParentContextGrant {
                grant_id: row.0,
                source_conversation_id: row.1,
                recipient_conversation_id: row.2,
                recipient_membership_id: row.3,
                source_refs: decode_json(&row.4)?,
                authorized_scopes: decode_json(&row.5)?,
                status: decode_grant_status(&row.6)?,
                request_id: row.7,
                revocation_generation: row.8,
            })
        })
        .collect()
}

fn decode_grant_status(raw: &str) -> Result<super::ContinuityParentGrantStatus, ContinuityFailure> {
    match raw {
        "admitted" => Ok(super::ContinuityParentGrantStatus::Admitted),
        "revoked" => Ok(super::ContinuityParentGrantStatus::Revoked),
        "exhausted" => Ok(super::ContinuityParentGrantStatus::Exhausted),
        _ => Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        )),
    }
}

pub fn upsert_grant(
    unit: &ContinuityUnitOfWork<'_>,
    grant: &ContinuityParentContextGrant,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_parent_grants(
           grant_id, source_conversation_id, recipient_conversation_id,
           recipient_membership_id, source_refs, authorized_scopes, status,
           request_id, revocation_generation
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(grant_id) DO UPDATE SET
           source_refs=excluded.source_refs,
           authorized_scopes=excluded.authorized_scopes,
           status=excluded.status,
           revocation_generation=excluded.revocation_generation",
        rusqlite::params![
            grant.grant_id,
            grant.source_conversation_id,
            grant.recipient_conversation_id,
            grant.recipient_membership_id,
            encode_json(&grant.source_refs)?,
            encode_json(&grant.authorized_scopes)?,
            match grant.status {
                super::ContinuityParentGrantStatus::Admitted => "admitted",
                super::ContinuityParentGrantStatus::Revoked => "revoked",
                super::ContinuityParentGrantStatus::Exhausted => "exhausted",
            },
            grant.request_id,
            grant.revocation_generation
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn load_completion(
    unit: &ContinuityUnitOfWork<'_>,
    notification_id: &str,
) -> Result<Option<ContinuityGoalCompletionTransition>, ContinuityFailure> {
    let row: Option<String> = unit
        .query_row(
            "SELECT transition FROM continuity_completion_transitions WHERE notification_id=?1",
            [notification_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    row.map(|raw| decode_json(&raw)).transpose()
}

pub fn store_completion(
    unit: &ContinuityUnitOfWork<'_>,
    transition: &ContinuityGoalCompletionTransition,
) -> Result<bool, ContinuityFailure> {
    let changed = unit
        .execute(
            "INSERT OR IGNORE INTO continuity_completion_transitions(
               notification_id, goal_id, transition, consumed, created_at
             ) VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![
                transition.notification_id,
                transition.goal_id,
                encode_json(transition)?,
                continuity_now_ms()
            ],
        )
        .map_err(sql_failure)?;
    Ok(changed > 0)
}

pub fn default_receipt(
    conversation_id: String,
    revision: i64,
    effect: ContinuityEffectClass,
) -> ContinuityCommitReceipt {
    ContinuityCommitReceipt {
        conversation_id,
        revision,
        effect_class: effect,
    }
}

pub fn due_from_attention(attention: &Option<ContinuityNextAttention>) -> Option<i64> {
    match attention {
        Some(ContinuityNextAttention::Wait { trigger_ref, .. }) => due_from_trigger(trigger_ref),
        _ => None,
    }
}

pub fn due_from_trigger(trigger_ref: &str) -> Option<i64> {
    trigger_ref
        .strip_prefix("due:")
        .or_else(|| trigger_ref.strip_prefix("review_due:"))
        .and_then(|value| value.parse().ok())
}

pub const ADOPTION_ENABLED_KEY: &str = "adoption_enabled";
pub const ADOPTION_STAGE_KEY: &str = "adoption_stage";
pub const ADOPTION_ENABLED_DEFAULT: &str = "1";
pub const ADOPTION_STAGE_OFFLINE: &str = "offline";
pub const ADOPTION_STAGE_ADMITTED_SHADOW: &str = "admitted_shadow";
pub const ADOPTION_STAGE_QUALIFIED_LOW_RISK: &str = "qualified_low_risk";
pub const ADOPTION_STAGE_EXPANDED: &str = "expanded";

pub fn persist_schema_value(
    unit: &ContinuityUnitOfWork<'_>,
    key: &str,
    value: &str,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_schema(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn load_schema_value(
    unit: &ContinuityUnitOfWork<'_>,
    key: &str,
) -> Result<Option<String>, ContinuityFailure> {
    unit.query_row(
        "SELECT value FROM continuity_schema WHERE key=?1",
        [key],
        |row| row.get(0),
    )
    .optional()
    .map_err(sql_failure)
}

pub fn persist_qualification_evidence(
    unit: &ContinuityUnitOfWork<'_>,
    responsibility_id: &str,
    identity_key: &str,
    payload: &str,
    evidence_class: &str,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_qualification_evidence(
           responsibility_id, identity_key, payload, evidence_class, ingested_at
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(responsibility_id, identity_key) DO UPDATE SET
           payload=excluded.payload,
           evidence_class=excluded.evidence_class,
           ingested_at=excluded.ingested_at",
        rusqlite::params![
            responsibility_id,
            identity_key,
            payload,
            evidence_class,
            continuity_now_ms()
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn load_qualification_evidence_row(
    unit: &ContinuityUnitOfWork<'_>,
    responsibility_id: &str,
    identity_key: &str,
) -> Result<Option<(String, String, String, String)>, ContinuityFailure> {
    unit.query_row(
        "SELECT responsibility_id, identity_key, payload, evidence_class
         FROM continuity_qualification_evidence
         WHERE responsibility_id=?1 AND identity_key=?2",
        rusqlite::params![responsibility_id, identity_key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .optional()
    .map_err(sql_failure)
}

pub fn load_qualification_evidence(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, String, String, String)>, ContinuityFailure> {
    unit.query_vec(
        "SELECT responsibility_id, identity_key, payload, evidence_class
         FROM continuity_qualification_evidence
         ORDER BY responsibility_id ASC, identity_key ASC",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .map_err(sql_failure)
}

pub fn list_completion_ids(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<String>, ContinuityFailure> {
    unit.query_vec(
        "SELECT notification_id FROM continuity_completion_transitions
         ORDER BY created_at ASC, notification_id ASC",
        [],
        |row| row.get(0),
    )
    .map_err(sql_failure)
}

#[derive(Clone, Debug)]
pub struct StoredCompletionRow {
    pub notification_id: String,
    pub goal_id: String,
    pub transition: ContinuityGoalCompletionTransition,
    pub consumed: bool,
}

pub fn list_completion_rows(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<StoredCompletionRow>, ContinuityFailure> {
    let rows = unit
        .query_vec(
            "SELECT notification_id, goal_id, transition, consumed
             FROM continuity_completion_transitions
             ORDER BY created_at ASC, notification_id ASC",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|(notification_id, goal_id, raw, consumed)| {
            Ok(StoredCompletionRow {
                notification_id,
                goal_id,
                transition: decode_json(&raw)?,
                consumed: consumed != 0,
            })
        })
        .collect()
}

pub fn consume_completion_ids(
    unit: &ContinuityUnitOfWork<'_>,
    ids: &[String],
) -> Result<(), ContinuityFailure> {
    for id in ids {
        unit.execute(
            "UPDATE continuity_completion_transitions
             SET consumed=1
             WHERE notification_id=?1 AND consumed=0",
            [id],
        )
        .map_err(sql_failure)?;
    }
    Ok(())
}

pub fn admit_local_owner_principal(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    owner_membership_id: &str,
) -> Result<String, ContinuityFailure> {
    let principal: Option<String> = unit
        .query_row(
            "SELECT p.id FROM memberships m
             JOIN principals p ON p.id=m.principal_id
             WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'
               AND m.access='owner' AND p.kind='human'",
            rusqlite::params![owner_membership_id, conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    principal.ok_or_else(|| {
        continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredOwnerAuthority {
    conversation_id: String,
    owner_membership_id: String,
    owner_principal_id: String,
}

impl StoredOwnerAuthority {
    pub(crate) fn from_admitted(
        conversation_id: String,
        owner_membership_id: String,
        owner_principal_id: String,
    ) -> Self {
        Self {
            conversation_id,
            owner_membership_id,
            owner_principal_id,
        }
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub fn owner_membership_id(&self) -> &str {
        &self.owner_membership_id
    }

    pub fn owner_principal_id(&self) -> &str {
        &self.owner_principal_id
    }
}

pub const EVALUATION_SESSION_KEY_PREFIX: &str = "evaluation_session:";
pub const EVALUATION_CORPUS_KEY_PREFIX: &str = "evaluation_corpus:";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvaluationCasePolarity {
    Negative,
    Positive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvaluationExpectedAction {
    Abstain,
    Takeover,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvaluationCase {
    pub case_id: String,
    pub family: String,
    pub subgroup: String,
    pub polarity: EvaluationCasePolarity,
    pub expected_action: EvaluationExpectedAction,
    pub input: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvaluationCorpus {
    pub conversation_id: String,
    pub owner_membership_id: String,
    pub owner_principal_id: String,
    pub dataset_id: String,
    pub version_digest: String,
    pub cases: Vec<StoredEvaluationCase>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEvaluationSession {
    pub session_id: String,
    pub conversation_id: String,
    pub owner_membership_id: String,
    pub owner_principal_id: String,
    pub recipient_membership_id: String,
    pub responsibility_id: String,
    pub identity: ContinuityCandidateIdentity,
    pub policy_revision: String,
    #[serde(default)]
    pub dataset_id: String,
    #[serde(default)]
    pub corpus_version: String,
    #[serde(default)]
    pub consumed: bool,
}

impl StoredEvaluationSession {
    pub fn same_admitted_binding(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.conversation_id == other.conversation_id
            && self.owner_membership_id == other.owner_membership_id
            && self.owner_principal_id == other.owner_principal_id
            && self.recipient_membership_id == other.recipient_membership_id
            && self.responsibility_id == other.responsibility_id
            && self.identity == other.identity
            && self.policy_revision == other.policy_revision
            && self.dataset_id == other.dataset_id
            && self.corpus_version == other.corpus_version
    }
}

pub fn evaluation_session_key(session_id: &str) -> String {
    format!("{EVALUATION_SESSION_KEY_PREFIX}{session_id}")
}

pub fn evaluation_corpus_key(conversation_id: &str) -> String {
    format!("{EVALUATION_CORPUS_KEY_PREFIX}{conversation_id}")
}

pub fn collection_effect_id_for_session(session_id: &str) -> String {
    format!("effect:collection:{session_id}")
}

pub fn evaluation_corpus_version_digest(
    dataset_id: &str,
    cases: &[StoredEvaluationCase],
) -> Result<String, ContinuityFailure> {
    let mut cases = cases.to_vec();
    cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    let canonical = serde_json::to_string(&serde_json::json!({
        "datasetId": dataset_id,
        "cases": cases,
    }))
    .map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
}

pub fn load_evaluation_corpus_in_unit(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<Option<StoredEvaluationCorpus>, ContinuityFailure> {
    let Some(raw) = load_schema_value(unit, &evaluation_corpus_key(conversation_id))? else {
        return Ok(None);
    };
    serde_json::from_str(&raw).map(Some).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })
}

pub fn persist_evaluation_corpus_in_unit(
    unit: &ContinuityUnitOfWork<'_>,
    corpus: &StoredEvaluationCorpus,
) -> Result<(), ContinuityFailure> {
    persist_schema_value(
        unit,
        &evaluation_corpus_key(&corpus.conversation_id),
        &serde_json::to_string(corpus).map_err(|_| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityCommit,
            )
        })?,
    )
}

pub fn load_evaluation_session_in_unit(
    unit: &ContinuityUnitOfWork<'_>,
    session_id: &str,
) -> Result<Option<StoredEvaluationSession>, ContinuityFailure> {
    let Some(raw) = load_schema_value(unit, &evaluation_session_key(session_id))? else {
        return Ok(None);
    };
    serde_json::from_str(&raw).map(Some).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })
}

pub fn persist_evaluation_session_in_unit(
    unit: &ContinuityUnitOfWork<'_>,
    session: &StoredEvaluationSession,
) -> Result<(), ContinuityFailure> {
    persist_schema_value(
        unit,
        &evaluation_session_key(&session.session_id),
        &serde_json::to_string(session).map_err(|_| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityCommit,
            )
        })?,
    )
}

const QUALIFIED_PENDING_NOTICE_PREDICATE: &str = "
    JOIN continuity_task_relations r ON r.goal_id = n.goal_id
    JOIN conversations parent ON parent.id = r.parent_conversation_id AND parent.archived = 0
    JOIN conversations child ON child.id = r.child_conversation_id AND child.archived = 0
    JOIN memberships owner_m
      ON owner_m.conversation_id = r.parent_conversation_id
     AND owner_m.principal_id = ?1
     AND owner_m.status = 'active'
     AND owner_m.access = 'owner'
    JOIN principals owner_p
      ON owner_p.id = owner_m.principal_id
     AND owner_p.kind = 'human'
    JOIN memberships child_m
      ON child_m.conversation_id = r.child_conversation_id
     AND child_m.principal_id = ?1
     AND child_m.status = 'active'
    JOIN continuity_goals g
      ON g.goal_id = n.goal_id
     AND g.deleted_at IS NULL
    WHERE NOT EXISTS (
      SELECT 1 FROM continuity_source_revocations rev
      WHERE rev.conversation_id = r.parent_conversation_id
        AND rev.opaque_id = r.card_event_id
    )";

fn map_completion_row(
    notification_id: String,
    goal_id: String,
    raw: String,
    consumed: i64,
) -> Result<StoredCompletionRow, ContinuityFailure> {
    Ok(StoredCompletionRow {
        notification_id,
        goal_id,
        transition: decode_json(&raw)?,
        consumed: consumed != 0,
    })
}

pub fn list_qualified_pending_completion_rows(
    unit: &ContinuityUnitOfWork<'_>,
    principal_id: &str,
    limit: usize,
) -> Result<Vec<StoredCompletionRow>, ContinuityFailure> {
    let sql = format!(
        "SELECT n.notification_id, n.goal_id, n.transition, n.consumed
         FROM continuity_completion_transitions n
         {QUALIFIED_PENDING_NOTICE_PREDICATE}
           AND n.consumed = 0
         ORDER BY n.created_at ASC, n.notification_id ASC
         LIMIT ?2"
    );
    let rows = unit
        .query_vec(&sql, rusqlite::params![principal_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|(notification_id, goal_id, raw, consumed)| {
            map_completion_row(notification_id, goal_id, raw, consumed)
        })
        .collect()
}

pub fn list_qualified_completion_rows_for_ids(
    unit: &ContinuityUnitOfWork<'_>,
    principal_id: &str,
    notification_ids: &[String],
) -> Result<Vec<StoredCompletionRow>, ContinuityFailure> {
    if notification_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut placeholders = String::new();
    let mut params: Vec<rusqlite::types::Value> = vec![principal_id.to_owned().into()];
    for (index, id) in notification_ids.iter().enumerate() {
        if index > 0 {
            placeholders.push(',');
        }
        placeholders.push('?');
        placeholders.push_str(&(index + 2).to_string());
        params.push(id.clone().into());
    }
    let sql = format!(
        "SELECT n.notification_id, n.goal_id, n.transition, n.consumed
         FROM continuity_completion_transitions n
         {QUALIFIED_PENDING_NOTICE_PREDICATE}
           AND n.notification_id IN ({placeholders})"
    );
    let rows = unit
        .query_vec(&sql, rusqlite::params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|(notification_id, goal_id, raw, consumed)| {
            map_completion_row(notification_id, goal_id, raw, consumed)
        })
        .collect()
}

pub fn load_qualified_completion_row(
    unit: &ContinuityUnitOfWork<'_>,
    principal_id: &str,
    notification_id: &str,
) -> Result<Option<StoredCompletionRow>, ContinuityFailure> {
    let sql = format!(
        "SELECT n.notification_id, n.goal_id, n.transition, n.consumed
         FROM continuity_completion_transitions n
         {QUALIFIED_PENDING_NOTICE_PREDICATE}
           AND n.notification_id = ?2"
    );
    let row = unit
        .query_row(
            &sql,
            rusqlite::params![principal_id, notification_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sql_failure)?;
    row.map(|(notification_id, goal_id, raw, consumed)| {
        map_completion_row(notification_id, goal_id, raw, consumed)
    })
    .transpose()
}

pub fn list_all_parent_grants(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
    let rows = unit
        .query_vec(
            "SELECT grant_id, source_conversation_id, recipient_conversation_id,
                    recipient_membership_id, source_refs, authorized_scopes, status,
                    request_id, revocation_generation
             FROM continuity_parent_grants
             ORDER BY grant_id ASC",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .map_err(sql_failure)?;
    rows.into_iter()
        .map(|row| {
            Ok(ContinuityParentContextGrant {
                grant_id: row.0,
                source_conversation_id: row.1,
                recipient_conversation_id: row.2,
                recipient_membership_id: row.3,
                source_refs: decode_json(&row.4)?,
                authorized_scopes: decode_json(&row.5)?,
                status: decode_grant_status(&row.6)?,
                request_id: row.7,
                revocation_generation: row.8,
            })
        })
        .collect()
}

pub fn count_cancel_effects(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    unit.query_row(
        "SELECT COUNT(*) FROM continuity_effects
         WHERE conversation_id=?1
           AND (logical_effect_id LIKE '%cancel%' OR status LIKE '%cancel%')",
        [conversation_id],
        |row| row.get(0),
    )
    .map_err(sql_failure)
}

fn lifecycle_wire(lifecycle: ContinuityGoalLifecycle) -> &'static str {
    match lifecycle {
        ContinuityGoalLifecycle::Active => "active",
        ContinuityGoalLifecycle::Waiting => "waiting",
        ContinuityGoalLifecycle::Verifying => "verifying",
        ContinuityGoalLifecycle::Achieved => "achieved",
        ContinuityGoalLifecycle::Cancelled => "cancelled",
        ContinuityGoalLifecycle::Superseded => "superseded",
    }
}

fn control_wire(control: ContinuityGoalControl) -> &'static str {
    match control {
        ContinuityGoalControl::Enabled => "enabled",
        ContinuityGoalControl::Paused => "paused",
        ContinuityGoalControl::CancelRequested => "cancel-requested",
    }
}

fn decode_scope(raw: &str) -> Result<super::ContinuityAgreementScope, ContinuityFailure> {
    match raw {
        "conversation" => Ok(super::ContinuityAgreementScope::Conversation),
        "matter" => Ok(super::ContinuityAgreementScope::Matter),
        "goal" => Ok(super::ContinuityAgreementScope::Goal),
        _ => Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        )),
    }
}

fn decode_origin(raw: &str) -> Result<super::ContinuityAgreementOrigin, ContinuityFailure> {
    match raw {
        "user-explicit" => Ok(super::ContinuityAgreementOrigin::UserExplicit),
        "source-fact" => Ok(super::ContinuityAgreementOrigin::SourceFact),
        "agent-inference" => Ok(super::ContinuityAgreementOrigin::AgentInference),
        _ => Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        )),
    }
}

type RelationRow = (
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    i64,
);

fn relation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn relation_from_parts(
    row: RelationRow,
) -> Result<ContinuityTaskConversationRelation, ContinuityFailure> {
    let created_event: ContinuitySourceRef = decode_json(&row.8)?;
    Ok(ContinuityTaskConversationRelation {
        goal_id: row.0,
        parent_conversation_id: row.1.clone(),
        child_conversation_id: row.2,
        card_anchor: super::ContinuityParentCardAnchor {
            parent_conversation_id: row.1,
            event_id: row.3,
            sequence: row.4,
            part_id: row.5,
        },
        listing_kind: ContinuityTaskListingKind::ChildTask,
        follow_through_kind: ContinuityFollowThroughKind::Durable,
        completion_transition: row.9.as_deref().map(decode_json).transpose()?,
        revision: row.10,
        created_event,
    })
}

pub fn list_all_pending_wakes(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, ContinuityWake)>, ContinuityFailure> {
    let rows: Vec<(String, String)> = unit
        .query_vec(
            "SELECT conversation_id, payload FROM continuity_outbox
             WHERE consumed_at IS NULL
             ORDER BY logical_wake_id ASC",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_failure)?;
    let mut wakes = Vec::with_capacity(rows.len());
    for (conversation_id, payload) in rows {
        wakes.push((conversation_id, decode_json(&payload)?));
    }
    Ok(wakes)
}

pub fn update_wake_generation(
    unit: &ContinuityUnitOfWork<'_>,
    logical_wake_id: &str,
    generation: i64,
) -> Result<bool, ContinuityFailure> {
    let payload: Option<String> = unit
        .query_row(
            "SELECT payload FROM continuity_outbox
             WHERE logical_wake_id=?1 AND consumed_at IS NULL",
            [logical_wake_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    let Some(raw) = payload else {
        return Ok(false);
    };
    let mut wake: ContinuityWake = decode_json(&raw)?;
    wake.host_generation = generation;
    let changed = unit
        .execute(
            "UPDATE continuity_outbox SET payload=?2
             WHERE logical_wake_id=?1 AND consumed_at IS NULL",
            rusqlite::params![logical_wake_id, encode_json(&wake)?],
        )
        .map_err(sql_failure)?;
    Ok(changed > 0)
}

pub fn append_goal_evidence(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    evidence: ContinuityEvidenceRef,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    let Some((contract, mut progress)) = load_goal(unit, goal_id)? else {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceUnavailable,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    let owned = goal_conversation_id(unit, goal_id)?;
    if owned.as_deref() != Some(conversation_id) {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    progress.criterion_evidence_refs.push(evidence);
    progress.revision += 1;
    upsert_goal(unit, conversation_id, &contract, &progress)?;
    Ok(progress)
}

pub fn write_named_cursor(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    interpretation_key: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "INSERT INTO continuity_source_cursors(
           conversation_id, source_event_id, interpretation_key, payload
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(conversation_id, source_event_id, interpretation_key)
         DO UPDATE SET payload=excluded.payload",
        rusqlite::params![
            conversation_id,
            source_event_id,
            interpretation_key,
            encode_json(payload)?
        ],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn delete_named_cursor(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    interpretation_key: &str,
) -> Result<(), ContinuityFailure> {
    unit.execute(
        "DELETE FROM continuity_source_cursors
         WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3",
        rusqlite::params![conversation_id, source_event_id, interpretation_key],
    )
    .map_err(sql_failure)?;
    Ok(())
}

pub fn named_cursor_exists(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    interpretation_key: &str,
) -> Result<bool, ContinuityFailure> {
    let found: i64 = unit
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM continuity_source_cursors
               WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3
             )",
            rusqlite::params![conversation_id, source_event_id, interpretation_key],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    Ok(found != 0)
}

pub fn load_named_cursor_payload(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    source_event_id: &str,
    interpretation_key: &str,
) -> Result<Option<Value>, ContinuityFailure> {
    let raw: Option<String> = unit
        .query_row(
            "SELECT payload FROM continuity_source_cursors
             WHERE conversation_id=?1 AND source_event_id=?2 AND interpretation_key=?3",
            rusqlite::params![conversation_id, source_event_id, interpretation_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_failure)?;
    raw.map(|value| decode_json(&value)).transpose()
}

pub fn write_settlement_pending(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    settlement_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        settlement_id,
        &settlement_interpretation_key(settlement_id, SETTLEMENT_PENDING_DESIGNATION),
        payload,
    )
}

pub fn write_settlement_applied(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    settlement_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        settlement_id,
        &settlement_interpretation_key(settlement_id, SETTLEMENT_APPLIED_DESIGNATION),
        payload,
    )?;
    delete_named_cursor(
        unit,
        conversation_id,
        settlement_id,
        &settlement_interpretation_key(settlement_id, SETTLEMENT_PENDING_DESIGNATION),
    )
}

pub fn settlement_applied(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    settlement_id: &str,
) -> Result<bool, ContinuityFailure> {
    named_cursor_exists(
        unit,
        conversation_id,
        settlement_id,
        &settlement_interpretation_key(settlement_id, SETTLEMENT_APPLIED_DESIGNATION),
    )
}

pub fn write_child_work_started(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_interpretation_key(goal_id, revision),
        &json!({
            "kind": CHILD_WORK_STARTED_DESIGNATION,
            "goalId": goal_id,
            "revision": revision,
            "admittedRevision": revision,
        }),
    )?;
    delete_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_PENDING_DESIGNATION),
    )
}

pub fn child_work_started(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<bool, ContinuityFailure> {
    named_cursor_exists(
        unit,
        conversation_id,
        goal_id,
        &child_work_interpretation_key(goal_id, revision),
    )
}

pub fn write_child_work_intent(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_INTENT_DESIGNATION),
        payload,
    )?;
    if child_work_started(unit, conversation_id, goal_id, revision)? {
        return delete_named_cursor(
            unit,
            conversation_id,
            goal_id,
            &child_work_named_key(goal_id, revision, CHILD_WORK_PENDING_DESIGNATION),
        );
    }
    write_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_PENDING_DESIGNATION),
        payload,
    )
}

pub fn write_child_work_accepted(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_ACCEPTED_DESIGNATION),
        payload,
    )
}

pub fn child_work_accepted(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<bool, ContinuityFailure> {
    named_cursor_exists(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_ACCEPTED_DESIGNATION),
    )
}

pub fn load_child_work_accepted(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<Option<Value>, ContinuityFailure> {
    load_named_cursor_payload(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_ACCEPTED_DESIGNATION),
    )
}

pub fn load_child_work_intent(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<Option<Value>, ContinuityFailure> {
    load_named_cursor_payload(
        unit,
        conversation_id,
        goal_id,
        &child_work_named_key(goal_id, revision, CHILD_WORK_INTENT_DESIGNATION),
    )
}

pub fn write_child_work_live(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    write_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_live_key(goal_id),
        payload,
    )
}

pub fn load_child_work_live(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Option<Value>, ContinuityFailure> {
    load_named_cursor_payload(
        unit,
        conversation_id,
        goal_id,
        &child_work_live_key(goal_id),
    )
}

pub fn delete_child_work_live(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
) -> Result<(), ContinuityFailure> {
    delete_named_cursor(
        unit,
        conversation_id,
        goal_id,
        &child_work_live_key(goal_id),
    )
}

pub fn list_child_work_live(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    let rows: Vec<(String, String, String)> = unit
        .query_vec(
            "SELECT conversation_id, source_event_id, payload
             FROM continuity_source_cursors
             WHERE interpretation_key LIKE 'work:%:child-work-live'
             ORDER BY conversation_id ASC, source_event_id ASC",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql_failure)?;
    let mut live = Vec::with_capacity(rows.len());
    for (conversation_id, goal_id, raw) in rows {
        live.push((conversation_id, goal_id, decode_json(&raw)?));
    }
    Ok(live)
}

pub fn load_oldest_pending_child_work(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Option<(i64, Value)>, ContinuityFailure> {
    let rows: Vec<(String, String)> = unit
        .query_vec(
            "SELECT interpretation_key, payload
             FROM continuity_source_cursors
             WHERE conversation_id=?1
               AND source_event_id=?2
               AND interpretation_key LIKE 'work:' || ?2 || ':%:child-work-pending'
             ORDER BY interpretation_key ASC",
            rusqlite::params![conversation_id, goal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_failure)?;
    let mut oldest: Option<(i64, Value)> = None;
    for (_, raw) in rows {
        let payload: Value = decode_json(&raw)?;
        let revision = payload
            .get("admittedRevision")
            .or_else(|| payload.get("revision"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if oldest
            .as_ref()
            .is_none_or(|(current, _)| revision < *current)
        {
            oldest = Some((revision, payload));
        }
    }
    Ok(oldest)
}

pub fn list_unacked_child_work_page(
    unit: &ContinuityUnitOfWork<'_>,
    after_conversation_id: Option<&str>,
    after_goal_id: Option<&str>,
    after_interpretation_key: Option<&str>,
    limit: u64,
) -> Result<Vec<(String, String, i64, Value)>, ContinuityFailure> {
    let limit = admit_page_limit(limit)? as i64;
    let rows: Vec<(String, String, String, String)> = unit
        .query_vec(
            "SELECT conversation_id, source_event_id, interpretation_key, payload
             FROM continuity_source_cursors
             WHERE interpretation_key LIKE 'work:%:child-work-pending'
               AND (
                 ?1 IS NULL
                 OR conversation_id > ?1
                 OR (conversation_id = ?1 AND source_event_id > ?2)
                 OR (
                   conversation_id = ?1
                   AND source_event_id = ?2
                   AND interpretation_key > ?3
                 )
               )
             ORDER BY conversation_id ASC, source_event_id ASC, interpretation_key ASC
             LIMIT ?4",
            rusqlite::params![
                after_conversation_id,
                after_goal_id,
                after_interpretation_key,
                limit
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(sql_failure)?;
    let mut pending = Vec::with_capacity(rows.len());
    for (conversation_id, goal_id, _, raw) in rows {
        let payload: Value = decode_json(&raw)?;
        let revision = payload
            .get("admittedRevision")
            .or_else(|| payload.get("revision"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        pending.push((conversation_id, goal_id, revision, payload));
    }
    Ok(pending)
}

pub fn list_unacked_child_work(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, String, i64, Value)>, ContinuityFailure> {
    let mut pending = Vec::new();
    let mut after_conversation: Option<String> = None;
    let mut after_goal: Option<String> = None;
    let mut after_key: Option<String> = None;
    loop {
        let page = list_unacked_child_work_page(
            unit,
            after_conversation.as_deref(),
            after_goal.as_deref(),
            after_key.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE,
        )?;
        let page_len = page.len();
        if let Some((conversation_id, goal_id, revision, _)) = page.last() {
            after_conversation = Some(conversation_id.clone());
            after_goal = Some(goal_id.clone());
            after_key = Some(child_work_named_key(
                goal_id,
                *revision,
                CHILD_WORK_PENDING_DESIGNATION,
            ));
        }
        pending.extend(page);
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(pending)
}

pub fn list_unapplied_settlements_page(
    unit: &ContinuityUnitOfWork<'_>,
    after_conversation_id: Option<&str>,
    after_settlement_id: Option<&str>,
    limit: u64,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    let limit = admit_page_limit(limit)? as i64;
    let rows: Vec<(String, String, String)> = unit
        .query_vec(
            "SELECT conversation_id, source_event_id, payload
             FROM continuity_source_cursors
             WHERE interpretation_key LIKE 'settlement:%:settlement-pending'
               AND (
                 ?1 IS NULL
                 OR conversation_id > ?1
                 OR (conversation_id = ?1 AND source_event_id > ?2)
               )
             ORDER BY conversation_id ASC, source_event_id ASC
             LIMIT ?3",
            rusqlite::params![after_conversation_id, after_settlement_id, limit],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql_failure)?;
    let mut pending = Vec::with_capacity(rows.len());
    for (conversation_id, settlement_id, raw) in rows {
        pending.push((conversation_id, settlement_id, decode_json(&raw)?));
    }
    Ok(pending)
}

pub fn list_unapplied_settlements(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    let mut pending = Vec::new();
    let mut after_conversation: Option<String> = None;
    let mut after_settlement: Option<String> = None;
    loop {
        let page = list_unapplied_settlements_page(
            unit,
            after_conversation.as_deref(),
            after_settlement.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE,
        )?;
        let page_len = page.len();
        if let Some((conversation_id, settlement_id, _)) = page.last() {
            after_conversation = Some(conversation_id.clone());
            after_settlement = Some(settlement_id.clone());
        }
        pending.extend(page);
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(pending)
}

pub fn find_admitted_parent_grant(
    unit: &ContinuityUnitOfWork<'_>,
    recipient_conversation_id: &str,
    recipient_membership_id: &str,
    source_conversation_id: &str,
    requested: &ContinuitySourceRef,
    basis: &ContinuityParentGrantBasis,
) -> Result<Option<ContinuityParentContextGrant>, ContinuityFailure> {
    let mut after: Option<String> = None;
    loop {
        let page = list_parent_grants(
            unit,
            recipient_conversation_id,
            recipient_membership_id,
            after.as_deref(),
            PENDING_OBLIGATION_PAGE_SIZE,
        )?;
        let page_len = page.len();
        for grant in &page {
            if grant.source_conversation_id != source_conversation_id {
                continue;
            }
            if admit_parent_context_grant(grant, requested, basis).is_ok() {
                return Ok(Some(grant.clone()));
            }
        }
        after = page.last().map(|grant| grant.grant_id.clone());
        if page_len < PENDING_OBLIGATION_PAGE_SIZE as usize {
            return Ok(None);
        }
    }
}

pub fn pending_obligation_scan_evidence(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<(Vec<String>, Vec<String>, i64, i64, usize), ContinuityFailure> {
    let work_plan = explain_plan(
        unit,
        "SELECT conversation_id, source_event_id, interpretation_key, payload
         FROM continuity_source_cursors
         WHERE interpretation_key LIKE 'work:%:child-work-pending'
         ORDER BY conversation_id ASC, source_event_id ASC, interpretation_key ASC
         LIMIT 50",
    )?;
    let settlement_plan = explain_plan(
        unit,
        "SELECT conversation_id, source_event_id, payload
         FROM continuity_source_cursors
         WHERE interpretation_key LIKE 'settlement:%:settlement-pending'
         ORDER BY conversation_id ASC, source_event_id ASC
         LIMIT 50",
    )?;
    let pending_work: i64 = unit
        .query_row(
            "SELECT COUNT(*) FROM continuity_source_cursors
             WHERE interpretation_key LIKE 'work:%:child-work-pending'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    let historical_intent: i64 = unit
        .query_row(
            "SELECT COUNT(*) FROM continuity_source_cursors
             WHERE interpretation_key LIKE 'work:%:child-work-intent'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_failure)?;
    let decoded = list_unacked_child_work(unit)?.len();
    Ok((
        work_plan,
        settlement_plan,
        pending_work,
        historical_intent,
        decoded,
    ))
}

fn explain_plan(
    unit: &ContinuityUnitOfWork<'_>,
    sql: &str,
) -> Result<Vec<String>, ContinuityFailure> {
    unit.query_vec(&format!("EXPLAIN QUERY PLAN {sql}"), [], |row| {
        row.get::<_, String>(3)
    })
    .map_err(sql_failure)
}

pub fn set_goal_wait_due(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    due_at: i64,
    review_policy: &str,
    responsible_party: &str,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    let Some((contract, mut progress)) = load_goal(unit, goal_id)? else {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceUnavailable,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    let owned = goal_conversation_id(unit, goal_id)?;
    if owned.as_deref() != Some(conversation_id) {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    progress.next_attention = Some(ContinuityNextAttention::Wait {
        trigger_ref: format!("due:{due_at}"),
        review_policy: review_policy.to_owned(),
        responsible_party: responsible_party.to_owned(),
        resumption_ref: None,
    });
    upsert_goal(unit, conversation_id, &contract, &progress)?;
    Ok(progress)
}
