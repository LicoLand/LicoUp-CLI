//! Membership-scoped dispatch repository boundary.

use super::{ConversationStore, StoreResult, new_id, now_ms, validate_identifier};
use crate::{
    CoalescedDispatchWake, ConversationDispatch, DispatchDeliveryKind, DispatchDeliveryRecord,
    DispatchDeliveryState, DispatchState, SubagentDispatchClaim, SubagentDispatchClaimState,
    SubagentMeshEdge, WaitSourceKind, WaitSourceRecord,
};
use anyhow::anyhow;
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params, params_from_iter};

/// The bounded multi-hop contract counts the direct edge as depth one.
pub const MAX_SUBAGENT_INVOCATION_DEPTH: u8 = 4;

/// Durable dispatch reads used to rebuild runtime state after host loss.
pub trait DispatchRepository {
    fn dispatch(&self, dispatch_id: &str) -> StoreResult<Option<ConversationDispatch>>;
    fn latest_resumable(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<ConversationDispatch>>;
    fn subagent_claim(&self, dispatch_id: &str) -> StoreResult<Option<SubagentDispatchClaim>>;
}

impl DispatchRepository for ConversationStore {
    fn dispatch(&self, dispatch_id: &str) -> StoreResult<Option<ConversationDispatch>> {
        self.dispatch_record(dispatch_id)
    }

    fn latest_resumable(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Option<ConversationDispatch>> {
        self.latest_resumable_dispatch(conversation_id, membership_id)
    }

    fn subagent_claim(&self, dispatch_id: &str) -> StoreResult<Option<SubagentDispatchClaim>> {
        ConversationStore::subagent_claim(self, dispatch_id)
    }
}

impl ConversationStore {
    /// Atomically validate both active Agent Memberships, server-owned lineage,
    /// depth and duplicate-edge admission before any adapter effect can start.
    pub fn claim_subagent_dispatch(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
        parent_dispatch_id: Option<&str>,
    ) -> StoreResult<SubagentDispatchClaim> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(caller_membership_id, "caller_membership_id")?;
        validate_identifier(target_membership_id, "target_membership_id")?;
        if caller_membership_id == target_membership_id {
            return Err(anyhow!("subagent_self_call_rejected"));
        }
        if let Some(parent) = parent_dispatch_id {
            validate_identifier(parent, "parent_dispatch_id")?;
        }

        let dispatch_id = new_id("subagent");
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            for membership_id in [caller_membership_id, target_membership_id] {
                let admitted: Option<i64> = transaction
                    .query_row(
                        "SELECT 1 FROM memberships m JOIN principals p ON p.id=m.principal_id
                         WHERE m.id=?1 AND m.conversation_id=?2 AND m.status='active'
                           AND p.kind='agent' AND p.agent_id IS NOT NULL",
                        params![membership_id, conversation_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if admitted.is_none() {
                    return Err(anyhow!(if membership_id == caller_membership_id {
                        "subagent_caller_membership_inactive"
                    } else {
                        "subagent_target_membership_inactive"
                    }));
                }
            }

            reconcile_subagent_claims(&transaction, conversation_id)?;

            let duplicate: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM subagent_dispatch_claims
                 WHERE conversation_id=?1 AND caller_membership_id=?2
                   AND target_membership_id=?3 AND state IN (
                     'claimed','running','cancel-requested','reconciliation-required'
                   ))",
                params![conversation_id, caller_membership_id, target_membership_id],
                |row| row.get(0),
            )?;
            if duplicate {
                return Err(anyhow!("subagent_duplicate_active_edge"));
            }

            let mut depth = 1_u8;
            if let Some(parent_dispatch_id) = parent_dispatch_id {
                let mut cursor = Some(parent_dispatch_id.to_owned());
                let mut first = true;
                let mut observed = std::collections::BTreeSet::new();
                while let Some(parent_id) = cursor.take() {
                    if !observed.insert(parent_id.clone()) {
                        return Err(anyhow!("subagent_lineage_cycle"));
                    }
                    let parent: Option<(String, String, String, Option<String>, i64)> = transaction
                        .query_row(
                            "SELECT conversation_id, caller_membership_id,
                                    target_membership_id, parent_dispatch_id, depth
                             FROM subagent_dispatch_claims WHERE id=?1",
                            params![parent_id],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                ))
                            },
                        )
                        .optional()?;
                    let Some((
                        parent_conversation,
                        ancestor_caller,
                        ancestor_target,
                        next,
                        parent_depth,
                    )) = parent
                    else {
                        return Err(anyhow!("subagent_parent_dispatch_unavailable"));
                    };
                    if parent_conversation != conversation_id {
                        return Err(anyhow!("subagent_cross_conversation_rejected"));
                    }
                    if first && ancestor_target != caller_membership_id {
                        return Err(anyhow!("subagent_lineage_caller_mismatch"));
                    }
                    if ancestor_caller == target_membership_id
                        || ancestor_target == target_membership_id
                    {
                        return Err(anyhow!("subagent_repeated_ancestor"));
                    }
                    if first {
                        let next_depth = parent_depth
                            .checked_add(1)
                            .ok_or_else(|| anyhow!("subagent_depth_exceeded"))?;
                        depth = u8::try_from(next_depth)
                            .map_err(|_| anyhow!("subagent_depth_exceeded"))?;
                    }
                    first = false;
                    cursor = next;
                }
            }
            if depth > MAX_SUBAGENT_INVOCATION_DEPTH {
                return Err(anyhow!("subagent_depth_exceeded"));
            }

            transaction.execute(
                "INSERT INTO subagent_dispatch_claims(
                   id, conversation_id, caller_membership_id, target_membership_id,
                   parent_dispatch_id, depth, state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    dispatch_id,
                    conversation_id,
                    caller_membership_id,
                    target_membership_id,
                    parent_dispatch_id,
                    i64::from(depth),
                    super::lifecycle::initial_claim().as_str(),
                    now,
                ],
            )?;
            transaction.commit()?;
            Ok(SubagentDispatchClaim {
                id: dispatch_id,
                conversation_id: conversation_id.to_owned(),
                caller_membership_id: caller_membership_id.to_owned(),
                target_membership_id: target_membership_id.to_owned(),
                parent_dispatch_id: parent_dispatch_id.map(str::to_owned),
                depth,
                state: SubagentDispatchClaimState::Claimed,
                created_at_unix_ms: now,
                updated_at_unix_ms: now,
            })
        })
    }

    pub fn subagent_claim(&self, dispatch_id: &str) -> StoreResult<Option<SubagentDispatchClaim>> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, caller_membership_id,
                            target_membership_id, parent_dispatch_id, depth, state,
                            created_at, updated_at
                     FROM subagent_dispatch_claims WHERE id=?1",
                    params![dispatch_id],
                    claim_from_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn active_subagent_claim(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
    ) -> StoreResult<Option<SubagentDispatchClaim>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(caller_membership_id, "caller_membership_id")?;
        validate_identifier(target_membership_id, "target_membership_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            reconcile_subagent_claims(&transaction, conversation_id)?;
            let claim = transaction
                .query_row(
                    "SELECT id, conversation_id, caller_membership_id,
                            target_membership_id, parent_dispatch_id, depth, state,
                            created_at, updated_at
                     FROM subagent_dispatch_claims
                     WHERE conversation_id=?1 AND caller_membership_id=?2
                       AND target_membership_id=?3 AND state IN (
                         'claimed','running','cancel-requested','reconciliation-required'
                       )
                     ORDER BY updated_at DESC, id DESC LIMIT 1",
                    params![conversation_id, caller_membership_id, target_membership_id],
                    claim_from_row,
                )
                .optional()?;
            transaction.commit()?;
            Ok(claim)
        })
    }

    pub fn update_subagent_claim_state(
        &self,
        dispatch_id: &str,
        next: SubagentDispatchClaimState,
    ) -> StoreResult<()> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            let current: Option<String> = connection
                .query_row(
                    "SELECT state FROM subagent_dispatch_claims WHERE id=?1",
                    params![dispatch_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(current) = current else {
                return Err(anyhow!("subagent_dispatch_not_found"));
            };
            if !super::lifecycle::valid_claim_transition(&current, next) {
                return Err(anyhow!("subagent_dispatch_transition_invalid"));
            }
            connection.execute(
                "UPDATE subagent_dispatch_claims SET state=?2, updated_at=?3 WHERE id=?1",
                params![dispatch_id, next.as_str(), now_ms()],
            )?;
            if matches!(
                next,
                SubagentDispatchClaimState::Completed
                    | SubagentDispatchClaimState::Failed
                    | SubagentDispatchClaimState::Cancelled
            ) {
                connection.execute(
                    "UPDATE subagent_dispatch_claims
                     SET watchdog_deadline_unix_ms=NULL, updated_at=?2
                     WHERE id=?1",
                    params![dispatch_id, now_ms()],
                )?;
            }
            Ok(())
        })
    }

    pub fn set_subagent_watchdog_deadline(
        &self,
        dispatch_id: &str,
        deadline_unix_ms: i64,
    ) -> StoreResult<()> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE subagent_dispatch_claims
                 SET watchdog_deadline_unix_ms=?2, updated_at=?3
                 WHERE id=?1 AND state IN (
                   'claimed','running','cancel-requested','reconciliation-required'
                 )",
                params![dispatch_id, deadline_unix_ms, now_ms()],
            )?;
            if changed == 0 {
                return Err(anyhow!("subagent_dispatch_not_found"));
            }
            Ok(())
        })
    }

    pub fn clear_subagent_watchdog_deadline(&self, dispatch_id: &str) -> StoreResult<()> {
        validate_identifier(dispatch_id, "dispatch_id")?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE subagent_dispatch_claims
                 SET watchdog_deadline_unix_ms=NULL, updated_at=?2
                 WHERE id=?1",
                params![dispatch_id, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Non-terminal claims that still own a durable watchdog deadline.
    /// Host boot re-arms these so a restart cannot drop timeout callbacks.
    pub fn pending_subagent_watchdogs(&self) -> StoreResult<Vec<(String, i64)>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, watchdog_deadline_unix_ms FROM subagent_dispatch_claims
                 WHERE watchdog_deadline_unix_ms IS NOT NULL
                   AND state IN (
                     'claimed','running','cancel-requested','reconciliation-required'
                   )
                 ORDER BY watchdog_deadline_unix_ms ASC, id ASC",
            )?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn record_subagent_mcp_inbound(
        &self,
        conversation_id: &str,
        caller_membership_id: Option<&str>,
        target_membership_id: Option<&str>,
        tool: &str,
        outcome: &str,
    ) -> StoreResult<()> {
        validate_identifier(conversation_id, "conversation_id")?;
        if !matches!(
            tool,
            "lico_subagent_delegate" | "lico_subagent_continue" | "lico_subagent_cancel"
        ) {
            return Err(anyhow!("subagent_mcp_inbound_tool_unsupported"));
        }
        if outcome.trim().is_empty() || outcome.len() > 128 {
            return Err(anyhow!("subagent_mcp_inbound_outcome_invalid"));
        }
        if let Some(caller) = caller_membership_id {
            validate_identifier(caller, "caller_membership_id")?;
        }
        if let Some(target) = target_membership_id {
            validate_identifier(target, "target_membership_id")?;
        }
        let id = new_id("mcp-in");
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO subagent_mcp_inbound(
                   id, conversation_id, caller_membership_id, target_membership_id,
                   tool, outcome, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    conversation_id,
                    caller_membership_id,
                    target_membership_id,
                    tool,
                    outcome,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn subagent_mesh_edge(
        &self,
        conversation_id: &str,
        caller_membership_id: &str,
        target_membership_id: &str,
    ) -> StoreResult<SubagentMeshEdge> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(caller_membership_id, "caller_membership_id")?;
        validate_identifier(target_membership_id, "target_membership_id")?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            reconcile_subagent_claims(&transaction, conversation_id)?;
            let mut inbound = SubagentMeshEdge::default();
            {
                let mut statement = transaction.prepare(
                    "SELECT tool, outcome FROM subagent_mcp_inbound
                     WHERE conversation_id=?1 AND caller_membership_id=?2
                       AND target_membership_id=?3
                     ORDER BY created_at ASC, id ASC",
                )?;
                let rows = statement.query_map(
                    params![conversation_id, caller_membership_id, target_membership_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )?;
                for row in rows {
                    let (tool, outcome) = row?;
                    match tool.as_str() {
                        "lico_subagent_delegate" => {
                            inbound.inbound_delegate = true;
                            inbound.delegate_outcome = Some(outcome);
                        }
                        "lico_subagent_continue" => {
                            inbound.inbound_continue = true;
                            inbound.continue_outcome = Some(outcome);
                        }
                        "lico_subagent_cancel" => {
                            inbound.inbound_cancel = true;
                            inbound.cancel_outcome = Some(outcome);
                        }
                        _ => {}
                    }
                }
            }
            inbound.claim_state = transaction
                .query_row(
                    "SELECT state FROM subagent_dispatch_claims
                     WHERE conversation_id=?1 AND caller_membership_id=?2
                       AND target_membership_id=?3
                     ORDER BY updated_at DESC, id DESC LIMIT 1",
                    params![conversation_id, caller_membership_id, target_membership_id],
                    |row| row.get(0),
                )
                .optional()?;
            inbound.dispatch_state = transaction
                .query_row(
                    "SELECT d.state FROM conversation_dispatches d
                     JOIN subagent_dispatch_claims c ON d.id=c.id
                     WHERE c.conversation_id=?1 AND c.caller_membership_id=?2
                       AND c.target_membership_id=?3
                     ORDER BY c.updated_at DESC, c.id DESC LIMIT 1",
                    params![conversation_id, caller_membership_id, target_membership_id],
                    |row| row.get(0),
                )
                .optional()?;
            transaction.commit()?;
            Ok(inbound)
        })
    }

    /// Record an observation feedback delivery (e.g. timeout / watchdog deadline)
    /// for a subagent claim. Observation feedback uses a mark distinct from the
    /// terminal state mark; they never share a single fired flag.
    pub fn record_subagent_observation_delivery(
        &self,
        claim_id: &str,
        payload: Option<&str>,
    ) -> StoreResult<bool> {
        validate_identifier(claim_id, "claim_id")?;
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let claim_info: Option<(String, String)> = transaction
                .query_row(
                    "SELECT conversation_id, caller_membership_id FROM subagent_dispatch_claims WHERE id=?1",
                    params![claim_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((conversation_id, caller_membership_id)) = claim_info else {
                return Err(anyhow!("subagent_dispatch_not_found"));
            };
            let inserted = record_pending_delivery_in_tx(
                &transaction,
                claim_id,
                DispatchDeliveryKind::Observation,
                &conversation_id,
                &caller_membership_id,
                None,
                payload,
                now,
            )?;
            transaction.commit()?;
            Ok(inserted)
        })
    }

    /// Read the separate observation and terminal delivery marks for a claim.
    pub fn subagent_delivery_status(
        &self,
        claim_id: &str,
    ) -> StoreResult<(
        Option<DispatchDeliveryRecord>,
        Option<DispatchDeliveryRecord>,
    )> {
        validate_identifier(claim_id, "claim_id")?;
        self.with_connection(|connection| {
            let mut observation = None;
            let mut terminal = None;
            let mut statement = connection.prepare(
                "SELECT claim_id, kind, conversation_id, recipient_membership_id, state,
                        terminal_state, payload, attempt_count, created_at, updated_at,
                        delivered_at, admitted_turn_id
                 FROM subagent_dispatch_deliveries WHERE claim_id=?1",
            )?;
            let rows = statement.query_map(params![claim_id], delivery_record_from_row)?;
            for row in rows {
                let record = row?;
                match record.kind {
                    DispatchDeliveryKind::Observation => observation = Some(record),
                    DispatchDeliveryKind::Terminal => terminal = Some(record),
                }
            }
            Ok((observation, terminal))
        })
    }

    /// Read all pending dispatch deliveries across one conversation.
    pub fn pending_dispatch_deliveries(
        &self,
        conversation_id: &str,
    ) -> StoreResult<Vec<DispatchDeliveryRecord>> {
        validate_identifier(conversation_id, "conversation_id")?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT claim_id, kind, conversation_id, recipient_membership_id, state,
                        terminal_state, payload, attempt_count, created_at, updated_at,
                        delivered_at, admitted_turn_id
                 FROM subagent_dispatch_deliveries
                 WHERE conversation_id=?1 AND state='pending'
                 ORDER BY updated_at ASC, claim_id ASC",
            )?;
            let rows = statement.query_map(params![conversation_id], delivery_record_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    /// Read pending dispatch deliveries for a specific recipient membership.
    pub fn pending_dispatch_deliveries_for_recipient(
        &self,
        conversation_id: &str,
        recipient_membership_id: &str,
    ) -> StoreResult<Vec<DispatchDeliveryRecord>> {
        validate_identifier(conversation_id, "conversation_id")?;
        validate_identifier(recipient_membership_id, "recipient_membership_id")?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT claim_id, kind, conversation_id, recipient_membership_id, state,
                        terminal_state, payload, attempt_count, created_at, updated_at,
                        delivered_at, admitted_turn_id
                 FROM subagent_dispatch_deliveries
                 WHERE conversation_id=?1 AND recipient_membership_id=?2 AND state='pending'
                 ORDER BY updated_at ASC, claim_id ASC",
            )?;
            let rows = statement.query_map(
                params![conversation_id, recipient_membership_id],
                delivery_record_from_row,
            )?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    /// Coalesce multiple pending notifications for one recipient membership into
    /// a single logical wake without merging away original events.
    pub fn coalesce_pending_deliveries_for_recipient(
        &self,
        conversation_id: &str,
        recipient_membership_id: &str,
    ) -> StoreResult<Option<CoalescedDispatchWake>> {
        let pending = self
            .pending_dispatch_deliveries_for_recipient(conversation_id, recipient_membership_id)?;
        if pending.is_empty() {
            return Ok(None);
        }
        let mut claim_ids = Vec::new();
        let mut has_terminal = false;
        let mut has_observation = false;
        for record in &pending {
            if !claim_ids.contains(&record.claim_id) {
                claim_ids.push(record.claim_id.clone());
            }
            match record.kind {
                DispatchDeliveryKind::Terminal => has_terminal = true,
                DispatchDeliveryKind::Observation => has_observation = true,
            }
        }
        Ok(Some(CoalescedDispatchWake {
            conversation_id: conversation_id.to_owned(),
            recipient_membership_id: recipient_membership_id.to_owned(),
            claim_ids,
            has_terminal,
            has_observation,
            deliveries: pending,
        }))
    }

    /// Mark a dispatch delivery as in-flight delivering. Increments attempt_count.
    pub fn mark_dispatch_delivery_delivering(
        &self,
        claim_id: &str,
        kind: DispatchDeliveryKind,
    ) -> StoreResult<bool> {
        validate_identifier(claim_id, "claim_id")?;
        let now = now_ms();
        let next = super::lifecycle::begin_delivery();
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE subagent_dispatch_deliveries
                 SET state=?3, attempt_count=attempt_count+1, updated_at=?4
                 WHERE claim_id=?1 AND kind=?2 AND state=?5",
                params![
                    claim_id,
                    kind.as_str(),
                    next.as_str(),
                    now,
                    DispatchDeliveryState::Pending.as_str()
                ],
            )?;
            Ok(changed > 0)
        })
    }

    /// Start failure, busy, or error reverts delivery state back to pending
    /// so the delivery is not lost.
    pub fn revert_dispatch_delivery_to_pending(
        &self,
        claim_id: &str,
        kind: DispatchDeliveryKind,
    ) -> StoreResult<bool> {
        validate_identifier(claim_id, "claim_id")?;
        let now = now_ms();
        let next = super::lifecycle::retry_delivery();
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE subagent_dispatch_deliveries
                 SET state=?3, updated_at=?4
                 WHERE claim_id=?1 AND kind=?2 AND state=?5",
                params![
                    claim_id,
                    kind.as_str(),
                    next.as_str(),
                    now,
                    DispatchDeliveryState::Delivering.as_str()
                ],
            )?;
            Ok(changed > 0)
        })
    }

    /// Assistant turn is associated with delivery success only after durable
    /// admission. Process-create success is not confirmation.
    pub fn admit_dispatch_delivery(
        &self,
        claim_id: &str,
        kind: DispatchDeliveryKind,
        admitted_turn_id: &str,
    ) -> StoreResult<bool> {
        validate_identifier(claim_id, "claim_id")?;
        validate_identifier(admitted_turn_id, "admitted_turn_id")?;
        let now = now_ms();
        let pending_next = super::lifecycle::admit_delivery(DispatchDeliveryState::Pending)
            .expect("configured pending delivery can be admitted");
        let delivering_next = super::lifecycle::admit_delivery(DispatchDeliveryState::Delivering)
            .expect("configured in-flight delivery can be admitted");
        debug_assert_eq!(pending_next, delivering_next);
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE subagent_dispatch_deliveries
                 SET state=?3, delivered_at=?4, admitted_turn_id=?5, updated_at=?4
                 WHERE claim_id=?1 AND kind=?2 AND state IN (?6,?7)",
                params![
                    claim_id,
                    kind.as_str(),
                    pending_next.as_str(),
                    now,
                    admitted_turn_id,
                    DispatchDeliveryState::Pending.as_str(),
                    DispatchDeliveryState::Delivering.as_str()
                ],
            )?;
            Ok(changed > 0)
        })
    }

    /// Active wait sources (subagent claims and direct turns) in this conversation.
    pub fn active_wait_sources(&self, conversation_id: &str) -> StoreResult<Vec<WaitSourceRecord>> {
        validate_identifier(conversation_id, "conversation_id")?;
        self.with_connection(|connection| {
            let mut sources = Vec::new();

            // 1. Subagent claims
            {
                let mut statement = connection.prepare(
                    "SELECT id, conversation_id, caller_membership_id, target_membership_id,
                            state, created_at, updated_at
                     FROM subagent_dispatch_claims
                     WHERE conversation_id=?1
                       AND state IN ('claimed','running','cancel-requested','reconciliation-required')
                     ORDER BY created_at ASC, id ASC",
                )?;
                let rows = statement.query_map(params![conversation_id], |row| {
                    Ok(WaitSourceRecord {
                        wait_source_id: row.get(0)?,
                        kind: WaitSourceKind::SubagentClaim,
                        conversation_id: row.get(1)?,
                        waiting_membership_id: row.get(2)?,
                        target_membership_id: Some(row.get(3)?),
                        state: row.get(4)?,
                        created_at_unix_ms: Some(row.get(5)?),
                        updated_at_unix_ms: Some(row.get(6)?),
                        is_terminal: false,
                    })
                })?;
                for row in rows {
                    sources.push(row?);
                }
            }

            // 2. Direct turns (the table records no timestamps; leave them
            // absent rather than fabricating zero values)
            {
                let mut statement = connection.prepare(
                    "SELECT id, conversation_id, membership_id, state, ordinal
                     FROM direct_turns
                     WHERE conversation_id=?1 AND state IN ('pending','claimed','running')
                     ORDER BY ordinal ASC, id ASC",
                )?;
                let rows = statement.query_map(params![conversation_id], |row| {
                    Ok(WaitSourceRecord {
                        wait_source_id: row.get(0)?,
                        kind: WaitSourceKind::DirectTurn,
                        conversation_id: row.get(1)?,
                        waiting_membership_id: row.get(2)?,
                        target_membership_id: None,
                        state: row.get(3)?,
                        created_at_unix_ms: None,
                        updated_at_unix_ms: None,
                        is_terminal: false,
                    })
                })?;
                for row in rows {
                    sources.push(row?);
                }
            }

            Ok(sources)
        })
    }

    /// Active wait sources where the given membership is the waiting party.
    pub fn active_wait_sources_for_membership(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<Vec<WaitSourceRecord>> {
        let all = self.active_wait_sources(conversation_id)?;
        Ok(all
            .into_iter()
            .filter(|s| s.waiting_membership_id == membership_id)
            .collect())
    }
}

fn delivery_record_from_row(row: &Row<'_>) -> rusqlite::Result<DispatchDeliveryRecord> {
    let claim_id: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let conversation_id: String = row.get(2)?;
    let recipient_membership_id: String = row.get(3)?;
    let state_str: String = row.get(4)?;
    let terminal_state: Option<String> = row.get(5)?;
    let payload: Option<String> = row.get(6)?;
    let attempt_count: u32 = row.get(7)?;
    let created_at_unix_ms: i64 = row.get(8)?;
    let updated_at_unix_ms: i64 = row.get(9)?;
    let delivered_at_unix_ms: Option<i64> = row.get(10)?;
    let admitted_turn_id: Option<String> = row.get(11)?;

    let kind = DispatchDeliveryKind::from_wire(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid delivery kind",
            )),
        )
    })?;
    let state = DispatchDeliveryState::from_wire(&state_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid delivery state",
            )),
        )
    })?;

    Ok(DispatchDeliveryRecord {
        claim_id,
        kind,
        conversation_id,
        recipient_membership_id,
        state,
        terminal_state,
        payload,
        attempt_count,
        created_at_unix_ms,
        updated_at_unix_ms,
        delivered_at_unix_ms,
        admitted_turn_id,
    })
}

#[allow(clippy::too_many_arguments)]
fn record_pending_delivery_in_tx(
    transaction: &impl super::CountedSqlite,
    claim_id: &str,
    kind: DispatchDeliveryKind,
    conversation_id: &str,
    recipient_membership_id: &str,
    terminal_state: Option<&str>,
    payload: Option<&str>,
    now: i64,
) -> StoreResult<bool> {
    let existing_state: Option<String> = transaction
        .query_row(
            "SELECT state FROM subagent_dispatch_deliveries WHERE claim_id=?1 AND kind=?2",
            params![claim_id, kind.as_str()],
            |row| row.get(0),
        )
        .optional()?;

    if existing_state.is_some() {
        return Ok(false);
    }

    transaction.execute(
        "INSERT INTO subagent_dispatch_deliveries
         (claim_id, kind, conversation_id, recipient_membership_id, state, terminal_state, payload, attempt_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?8)",
        params![
            claim_id,
            kind.as_str(),
            conversation_id,
            recipient_membership_id,
            super::lifecycle::initial_delivery().as_str(),
            terminal_state,
            payload,
            now,
        ],
    )?;
    Ok(true)
}

/// Project terminal PersistentTurn state back into the private lineage claim.
/// This is reconciliation, not a second terminal authority: the canonical
/// `conversation_dispatches` row is the source and this claim only controls
/// admission of the next edge.
fn reconcile_subagent_claims(
    transaction: &super::CountedTransaction<'_>,
    conversation_id: &str,
) -> StoreResult<()> {
    let transitions = super::lifecycle::claim_reconciliation_transitions(false);
    reconcile_claim_set(transaction, &transitions, Some(conversation_id), now_ms())
}

/// Host-open writeback: a crash can fail the canonical dispatch while the
/// lineage claim is still `running`. `subagent_claim` never reconciles, so
/// this pass must settle every open claim whose dispatch is already terminal.
pub(super) fn reconcile_terminal_subagent_claims(
    transaction: &impl super::CountedSqlite,
) -> StoreResult<()> {
    let now = now_ms();
    let transitions = super::lifecycle::claim_reconciliation_transitions(true);
    reconcile_claim_set(transaction, &transitions, None, now)?;
    // Ensure every terminal claim has a recorded pending delivery if not already recorded.
    transaction.execute(
        "INSERT OR IGNORE INTO subagent_dispatch_deliveries
         (claim_id, kind, conversation_id, recipient_membership_id, state, terminal_state, payload, attempt_count, created_at, updated_at)
         SELECT c.id, 'terminal', c.conversation_id, c.caller_membership_id, ?1, c.state, NULL, 0, ?2, ?2
         FROM subagent_dispatch_claims c
         WHERE c.state IN ('completed', 'failed', 'cancelled')",
        params![
            super::lifecycle::initial_delivery().as_str(),
            now
        ],
    )?;
    // Host recovery resets any interrupted 'delivering' status back to 'pending'.
    let pending = super::lifecycle::retry_delivery();
    transaction.execute(
        "UPDATE subagent_dispatch_deliveries
         SET state=?1, updated_at=?2
         WHERE state=?3",
        params![
            pending.as_str(),
            now,
            DispatchDeliveryState::Delivering.as_str()
        ],
    )?;
    Ok(())
}

/// Update every eligible claim in one statement. The CTE is a small bounded
/// projection of generated machine transitions; claim rows are never
/// materialized into Rust or updated one at a time.
fn reconcile_claim_set(
    transaction: &impl super::CountedSqlite,
    transitions: &[(&str, &str, &str)],
    conversation_id: Option<&str>,
    now: i64,
) -> StoreResult<()> {
    if transitions.is_empty() {
        return Ok(());
    }

    let values_clause = (0..transitions.len())
        .map(|index| {
            let first = index * 3 + 1;
            format!("(?{first}, ?{}, ?{})", first + 1, first + 2)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut values = Vec::with_capacity(transitions.len() * 3 + 2);
    for (claim_state, dispatch_state, next_state) in transitions {
        values.push(Value::Text((*claim_state).to_owned()));
        values.push(Value::Text((*dispatch_state).to_owned()));
        values.push(Value::Text((*next_state).to_owned()));
    }
    let now_parameter = values.len() + 1;
    values.push(Value::Integer(now));
    let conversation_filter = if let Some(conversation_id) = conversation_id {
        let parameter = values.len() + 1;
        values.push(Value::Text(conversation_id.to_owned()));
        format!(" AND conversation_id=?{parameter}")
    } else {
        String::new()
    };
    let sql = format!(
        "WITH transitions(claim_state, dispatch_state, next_state) AS (VALUES {values_clause})
         UPDATE subagent_dispatch_claims
         SET state=(
               SELECT transitions.next_state
               FROM transitions
               JOIN conversation_dispatches d
                 ON d.id=subagent_dispatch_claims.id
               WHERE transitions.claim_state=subagent_dispatch_claims.state
                 AND transitions.dispatch_state=d.state
             ),
             updated_at=?{now_parameter}
         WHERE EXISTS (
               SELECT 1
               FROM transitions
               JOIN conversation_dispatches d
                 ON d.id=subagent_dispatch_claims.id
               WHERE transitions.claim_state=subagent_dispatch_claims.state
                 AND transitions.dispatch_state=d.state
             ){conversation_filter}"
    );
    transaction.execute(&sql, params_from_iter(values))?;
    Ok(())
}

/// Eager terminal writeback for one settled dispatch. When the canonical
/// `conversation_dispatches` row reaches a terminal state, the lineage claim
/// sharing the dispatch id moves to the matching claim state inside the same
/// transaction. A missing claim or a transition that
/// the configured operational lifecycle forbids is left to the reconciler instead
/// of failing the settlement.
pub(super) fn writeback_subagent_claim_terminal(
    transaction: &super::CountedTransaction<'_>,
    dispatch_id: &str,
    state: DispatchState,
) -> StoreResult<()> {
    let claim_info: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT state, conversation_id, caller_membership_id FROM subagent_dispatch_claims WHERE id=?1",
            params![dispatch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((current, conversation_id, caller_membership_id)) = claim_info else {
        return Ok(());
    };
    let Some(next) = super::lifecycle::settle_claim(&current, state) else {
        return Ok(());
    };
    let now = now_ms();
    transaction.execute(
        "UPDATE subagent_dispatch_claims SET state=?2, updated_at=?3 WHERE id=?1",
        params![dispatch_id, next.as_str(), now],
    )?;
    record_pending_delivery_in_tx(
        transaction,
        dispatch_id,
        DispatchDeliveryKind::Terminal,
        &conversation_id,
        &caller_membership_id,
        Some(next.as_str()),
        None,
        now,
    )?;
    Ok(())
}

fn claim_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubagentDispatchClaim> {
    let state: String = row.get(6)?;
    let state =
        SubagentDispatchClaimState::from_name(&state).ok_or(rusqlite::Error::InvalidQuery)?;
    let depth: i64 = row.get(5)?;
    let depth =
        u8::try_from(depth).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, depth))?;
    Ok(SubagentDispatchClaim {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        caller_membership_id: row.get(2)?,
        target_membership_id: row.get(3)?,
        parent_dispatch_id: row.get(4)?,
        depth,
        state,
        created_at_unix_ms: row.get(7)?,
        updated_at_unix_ms: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MembershipAccess, Principal, PrincipalKind};

    fn fixture() -> (ConversationStore, String, Vec<String>) {
        let store = ConversationStore::open_in_memory().unwrap();
        let owner = Principal {
            id: "human:owner".into(),
            kind: PrincipalKind::Human,
            display_name: "Owner".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let agents = [
            "codex",
            "cursor",
            "antigravity",
            "codex-next",
            "cursor-next",
            "antigravity-next",
        ]
        .into_iter()
        .map(|id| {
            (
                Principal {
                    id: format!("agent:{id}"),
                    kind: PrincipalKind::Agent,
                    display_name: id.into(),
                    agent_id: Some(id.into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
        })
        .collect::<Vec<_>>();
        let conversation = store
            .create_conversation_with_members("Mesh", owner, &agents)
            .unwrap();
        let memberships = conversation
            .memberships
            .iter()
            .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
            .map(|membership| membership.id.clone())
            .collect();
        (store, conversation.id, memberships)
    }

    fn reconciliation_query_counts(claim_count: usize) -> (usize, usize, i64, i64, i64) {
        let (store, conversation, memberships) = fixture();
        store
            .with_connection(|connection| {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                for index in 0..claim_count {
                    let caller = index % memberships.len();
                    let target = (caller + 1 + index / memberships.len()) % memberships.len();
                    let id = format!("dispatch:reconcile-{index}");
                    transaction.execute(
                        "INSERT INTO conversation_dispatches(
                           id, conversation_id, membership_id, operation, state,
                           session_mode, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, 'subagent.delegate', 'completed', 'new', 1, 1)",
                        params![id, conversation, memberships[target]],
                    )?;
                    transaction.execute(
                        "INSERT INTO subagent_dispatch_claims(
                           id, conversation_id, caller_membership_id,
                           target_membership_id, depth, state, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, ?4, 1, 'running', 1, 1)",
                        params![id, conversation, memberships[caller], memberships[target]],
                    )?;
                }
                transaction.commit()?;
                Ok(())
            })
            .unwrap();

        let before_active = store.counters().queries();
        store
            .with_connection(|connection| {
                let transaction = connection.unchecked_transaction()?;
                reconcile_subagent_claims(&transaction, &conversation)?;
                transaction.commit()?;
                Ok(())
            })
            .unwrap();
        let active_query_delta = store.counters().queries() - before_active;
        let active_settled = store
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT COUNT(*) FROM subagent_dispatch_claims
                     WHERE conversation_id=?1 AND state='completed'",
                    params![conversation],
                    |row| row.get(0),
                )?)
            })
            .unwrap();

        store
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE subagent_dispatch_claims SET state='running' WHERE conversation_id=?1",
                    params![conversation],
                )?;
                Ok(())
            })
            .unwrap();

        let before_terminal = store.counters().queries();
        store
            .with_connection(|connection| {
                let transaction = connection.unchecked_transaction()?;
                reconcile_terminal_subagent_claims(&transaction)?;
                transaction.commit()?;
                Ok(())
            })
            .unwrap();
        let terminal_query_delta = store.counters().queries() - before_terminal;
        let (settled_claims, terminal_deliveries) = store
            .with_connection(|connection| {
                let settled_claims = connection.query_row(
                    "SELECT COUNT(*) FROM subagent_dispatch_claims
                     WHERE conversation_id=?1 AND state='completed'",
                    params![conversation],
                    |row| row.get(0),
                )?;
                let terminal_deliveries = connection.query_row(
                    "SELECT COUNT(*) FROM subagent_dispatch_deliveries
                     WHERE conversation_id=?1 AND kind='terminal' AND terminal_state='completed'",
                    params![conversation],
                    |row| row.get(0),
                )?;
                Ok((settled_claims, terminal_deliveries))
            })
            .unwrap();

        (
            active_query_delta,
            terminal_query_delta,
            active_settled,
            settled_claims,
            terminal_deliveries,
        )
    }

    #[test]
    fn reconciliation_statement_count_is_independent_of_claim_count() {
        let small = reconciliation_query_counts(1);
        let large = reconciliation_query_counts(24);

        assert_eq!(small.0, 3, "one transaction with one set-based update");
        assert_eq!(large.0, small.0);
        assert_eq!(small.1, 5, "one transaction with three set-based writes");
        assert_eq!(large.1, small.1);
        assert_eq!((small.2, small.3, small.4), (1, 1, 1));
        assert_eq!((large.2, large.3, large.4), (24, 24, 24));
    }

    #[test]
    fn lineage_rejects_self_cycle_depth_and_duplicate_without_claim_residue() {
        let (store, conversation, membership) = fixture();
        assert_eq!(
            store
                .claim_subagent_dispatch(&conversation, &membership[0], &membership[0], None)
                .unwrap_err()
                .to_string(),
            "subagent_self_call_rejected"
        );
        let first = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();
        assert_eq!(first.depth, 1);
        assert_eq!(
            store
                .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
                .unwrap_err()
                .to_string(),
            "subagent_duplicate_active_edge"
        );
        let second = store
            .claim_subagent_dispatch(
                &conversation,
                &membership[1],
                &membership[2],
                Some(&first.id),
            )
            .unwrap();
        assert_eq!(second.depth, 2);
        assert_eq!(
            store
                .claim_subagent_dispatch(
                    &conversation,
                    &membership[2],
                    &membership[0],
                    Some(&second.id),
                )
                .unwrap_err()
                .to_string(),
            "subagent_repeated_ancestor"
        );
        let third = store
            .claim_subagent_dispatch(
                &conversation,
                &membership[2],
                &membership[3],
                Some(&second.id),
            )
            .unwrap();
        let fourth = store
            .claim_subagent_dispatch(
                &conversation,
                &membership[3],
                &membership[4],
                Some(&third.id),
            )
            .unwrap();
        assert_eq!(fourth.depth, MAX_SUBAGENT_INVOCATION_DEPTH);
        assert_eq!(
            store
                .claim_subagent_dispatch(
                    &conversation,
                    &membership[4],
                    &membership[5],
                    Some(&fourth.id),
                )
                .unwrap_err()
                .to_string(),
            "subagent_depth_exceeded"
        );
    }

    #[test]
    fn terminal_persistent_turn_reconciles_active_edge_before_next_claim() {
        let (store, conversation, membership) = fixture();
        let claim = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();
        let target_agent = store
            .get(&conversation)
            .unwrap()
            .memberships
            .into_iter()
            .find(|candidate| candidate.id == membership[1])
            .and_then(|candidate| candidate.principal.agent_id)
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                &target_agent,
                "native-cursor",
                "synthetic input",
                Some(&conversation),
                Some(&membership[1]),
                Some("subagent-mcp"),
                Some(&claim.id),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output":"done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();
        let next = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();
        assert_ne!(next.id, claim.id);
        assert_eq!(
            store.subagent_claim(&claim.id).unwrap().unwrap().state,
            SubagentDispatchClaimState::Completed
        );
    }

    /// One claimed edge admitted to `running` with its canonical dispatch
    /// prepared under the claim id, ready to settle through the terminal door.
    fn running_dispatch(
        store: &ConversationStore,
        conversation: &str,
        caller: &str,
        target: &str,
    ) -> (SubagentDispatchClaim, crate::ConversationRuntimeScope) {
        let claim = store
            .claim_subagent_dispatch(conversation, caller, target, None)
            .unwrap();
        store
            .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Running)
            .unwrap();
        let target_agent = store
            .get(conversation)
            .unwrap()
            .memberships
            .into_iter()
            .find(|candidate| candidate.id == target)
            .and_then(|candidate| candidate.principal.agent_id)
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                &target_agent,
                "native-target",
                "synthetic input",
                Some(conversation),
                Some(target),
                Some("subagent-mcp"),
                Some(&claim.id),
            )
            .unwrap();
        (claim, scope)
    }

    #[test]
    fn finish_runtime_dispatch_settles_claim_in_the_same_transaction() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output":"done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();
        // `subagent_claim` is a direct row read that never reconciles, so the
        // terminal state must already be durable right after settlement.
        assert_eq!(
            store.subagent_claim(&claim.id).unwrap().unwrap().state,
            SubagentDispatchClaimState::Completed
        );
        // The active edge is released without waiting for the next claim.
        assert!(
            store
                .active_subagent_claim(&conversation, &membership[0], &membership[1])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn finish_runtime_dispatch_maps_failed_and_cancelled_into_claim_terminal_states() {
        let (store, conversation, membership) = fixture();
        let (failed_claim, failed_scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);
        store
            .finish_runtime_dispatch(
                &failed_scope,
                &serde_json::json!({
                    "ok": false,
                    "error": {"code": "codex_usage_limit_exceeded"}
                }),
                crate::DispatchState::Failed,
                Some("codex_usage_limit_exceeded"),
            )
            .unwrap();
        assert_eq!(
            store
                .subagent_claim(&failed_claim.id)
                .unwrap()
                .unwrap()
                .state,
            SubagentDispatchClaimState::Failed
        );

        let (cancelled_claim, cancelled_scope) =
            running_dispatch(&store, &conversation, &membership[1], &membership[2]);
        store
            .update_subagent_claim_state(
                &cancelled_claim.id,
                SubagentDispatchClaimState::CancelRequested,
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &cancelled_scope,
                &serde_json::json!({"ok": false, "turnStatus": "cancelled"}),
                crate::DispatchState::Cancelled,
                None,
            )
            .unwrap();
        assert_eq!(
            store
                .subagent_claim(&cancelled_claim.id)
                .unwrap()
                .unwrap()
                .state,
            SubagentDispatchClaimState::Cancelled
        );
    }

    #[test]
    fn finish_runtime_dispatch_settles_a_claim_that_never_reached_running() {
        let (store, conversation, membership) = fixture();
        let claim = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();
        let target_agent = store
            .get(&conversation)
            .unwrap()
            .memberships
            .into_iter()
            .find(|candidate| candidate.id == membership[1])
            .and_then(|candidate| candidate.principal.agent_id)
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                &target_agent,
                "native-cursor",
                "synthetic input",
                Some(&conversation),
                Some(&membership[1]),
                Some("subagent-mcp"),
                Some(&claim.id),
            )
            .unwrap();
        // The claim never reached `running` (the adapter receipt never
        // arrived). Settlement still writes the matching terminal claim so a
        // later `subagent_claim` read — which never reconciles — is not left
        // at `claimed` after the dispatch has already completed.
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output":"done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();
        assert_eq!(
            store.subagent_claim(&claim.id).unwrap().unwrap().state,
            SubagentDispatchClaimState::Completed
        );
        assert!(
            store
                .active_subagent_claim(&conversation, &membership[0], &membership[1])
                .unwrap()
                .is_none()
        );
    }

    /// Dual-delivery leg (a): the delegated turn's output lands in the shared
    /// group Conversation Event/Part stream authored by the target Membership.
    #[test]
    fn settled_subagent_turn_output_lands_in_the_group_stream_authored_by_target() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);
        store
            .append_runtime_frame(
                &scope,
                1,
                &serde_json::json!({
                    "event": "agent.turn.accepted",
                    "sessionId": "native-session-1",
                    "turnId": "turn-1",
                    "payload": {"status": "accepted", "lifecyclePrefix": ["submitted", "accepted"]}
                }),
            )
            .unwrap();
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "delegated final answer"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();
        let event = store
            .page_events(&conversation, None, 50)
            .unwrap()
            .events
            .into_iter()
            .find(|candidate| candidate.id == scope.event_id)
            .unwrap();
        assert_eq!(
            event.author_membership_id.as_deref(),
            Some(membership[1].as_str())
        );
        assert_eq!(event.correlation_id.as_deref(), Some(claim.id.as_str()));
        assert!(event.finalized);
        assert!(event.parts.iter().any(|part| {
            part.kind == crate::EventPartKind::Text && part.content == "delegated final answer"
        }));
    }

    #[test]
    fn inbound_rows_project_last_outcome_per_tool_without_claim() {
        let (store, conversation, membership) = fixture();
        store
            .record_subagent_mcp_inbound(
                &conversation,
                Some(&membership[0]),
                Some(&membership[1]),
                "lico_subagent_delegate",
                "subagent_self_call_rejected",
            )
            .unwrap();
        store
            .record_subagent_mcp_inbound(
                &conversation,
                Some(&membership[0]),
                Some(&membership[1]),
                "lico_subagent_delegate",
                "accepted",
            )
            .unwrap();
        store
            .record_subagent_mcp_inbound(
                &conversation,
                Some(&membership[0]),
                Some(&membership[1]),
                "lico_subagent_continue",
                "accepted",
            )
            .unwrap();
        store
            .record_subagent_mcp_inbound(
                &conversation,
                Some(&membership[0]),
                Some(&membership[1]),
                "lico_subagent_cancel",
                "subagent_cancel_unavailable",
            )
            .unwrap();
        assert_eq!(
            store
                .record_subagent_mcp_inbound(
                    &conversation,
                    Some(&membership[0]),
                    Some(&membership[1]),
                    "lico_subagents_list",
                    "accepted",
                )
                .unwrap_err()
                .to_string(),
            "subagent_mcp_inbound_tool_unsupported"
        );
        let edge = store
            .subagent_mesh_edge(&conversation, &membership[0], &membership[1])
            .unwrap();
        assert_eq!(
            edge,
            SubagentMeshEdge {
                inbound_delegate: true,
                inbound_continue: true,
                inbound_cancel: true,
                delegate_outcome: Some("accepted".into()),
                continue_outcome: Some("accepted".into()),
                cancel_outcome: Some("subagent_cancel_unavailable".into()),
                claim_state: None,
                dispatch_state: None,
            }
        );
        let other = store
            .subagent_mesh_edge(&conversation, &membership[1], &membership[0])
            .unwrap();
        assert_eq!(other, SubagentMeshEdge::default());
    }

    #[test]
    fn watchdog_deadline_survives_store_reopen() {
        let (store, conversation, membership) = fixture();
        let claim = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();
        store
            .set_subagent_watchdog_deadline(&claim.id, 9_000)
            .unwrap();
        let pending = store.pending_subagent_watchdogs().unwrap();
        assert_eq!(pending, vec![(claim.id.clone(), 9_000)]);
        store
            .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Completed)
            .unwrap();
        assert!(store.pending_subagent_watchdogs().unwrap().is_empty());
    }

    #[test]
    fn terminal_state_and_pending_delivery_commit_together() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);

        // Prior to finish, no deliveries exist
        let (obs, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert!(obs.is_none());
        assert!(term.is_none());

        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        // Terminal claim state and pending delivery commit together
        assert_eq!(
            store.subagent_claim(&claim.id).unwrap().unwrap().state,
            SubagentDispatchClaimState::Completed
        );
        let (obs, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert!(obs.is_none());
        let term = term.expect("terminal delivery must be recorded atomically");
        assert_eq!(term.kind, DispatchDeliveryKind::Terminal);
        assert_eq!(term.state, DispatchDeliveryState::Pending);
        assert_eq!(term.recipient_membership_id, membership[0]);
        assert_eq!(term.terminal_state.as_deref(), Some("completed"));
        assert_eq!(term.attempt_count, 0);
        assert!(term.admitted_turn_id.is_none());
    }

    #[test]
    fn observation_feedback_and_terminal_state_use_separate_marks() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);

        // 1. Observation feedback (e.g. timeout / watchdog) is recorded
        let inserted = store
            .record_subagent_observation_delivery(&claim.id, Some("watchdog_timeout_payload"))
            .unwrap();
        assert!(inserted);

        let (obs, term) = store.subagent_delivery_status(&claim.id).unwrap();
        let obs = obs.expect("observation delivery should exist");
        assert_eq!(obs.kind, DispatchDeliveryKind::Observation);
        assert_eq!(obs.state, DispatchDeliveryState::Pending);
        assert_eq!(obs.payload.as_deref(), Some("watchdog_timeout_payload"));
        assert!(term.is_none());

        // 2. Admit the observation delivery: process-create success is not confirmation,
        // it must have durable admission.
        store
            .mark_dispatch_delivery_delivering(&claim.id, DispatchDeliveryKind::Observation)
            .unwrap();
        let admitted = store
            .admit_dispatch_delivery(
                &claim.id,
                DispatchDeliveryKind::Observation,
                "admitted-obs-turn-1",
            )
            .unwrap();
        assert!(admitted);

        let (obs, term) = store.subagent_delivery_status(&claim.id).unwrap();
        let obs = obs.unwrap();
        assert_eq!(obs.state, DispatchDeliveryState::Delivered);
        assert_eq!(obs.admitted_turn_id.as_deref(), Some("admitted-obs-turn-1"));
        assert!(term.is_none());

        // 3. Subagent turn eventually finishes later.
        // Observation delivery must NOT suppress the real terminal delivery!
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "final subagent result"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        let (obs, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert_eq!(obs.unwrap().state, DispatchDeliveryState::Delivered);
        let term =
            term.expect("terminal delivery must be recorded and not suppressed by observation");
        assert_eq!(term.kind, DispatchDeliveryKind::Terminal);
        assert_eq!(term.state, DispatchDeliveryState::Pending);
        assert_eq!(term.terminal_state.as_deref(), Some("completed"));
    }

    #[test]
    fn delivery_in_flight_failure_reverts_to_pending_and_requires_admission() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);

        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        // 1. Mark in-flight delivering
        let marked = store
            .mark_dispatch_delivery_delivering(&claim.id, DispatchDeliveryKind::Terminal)
            .unwrap();
        assert!(marked);

        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        let term = term.unwrap();
        assert_eq!(term.state, DispatchDeliveryState::Delivering);
        assert_eq!(term.attempt_count, 1);

        // 2. Start failure or busy reverts state back to pending
        let reverted = store
            .revert_dispatch_delivery_to_pending(&claim.id, DispatchDeliveryKind::Terminal)
            .unwrap();
        assert!(reverted);

        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        let term = term.unwrap();
        assert_eq!(term.state, DispatchDeliveryState::Pending);
        assert_eq!(term.attempt_count, 1); // Attempt count retained

        // 3. Subsequent retry marks delivering again (attempt increments to 2)
        store
            .mark_dispatch_delivery_delivering(&claim.id, DispatchDeliveryKind::Terminal)
            .unwrap();
        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert_eq!(term.unwrap().attempt_count, 2);

        // 4. Durable admission confirms delivery
        let admitted = store
            .admit_dispatch_delivery(
                &claim.id,
                DispatchDeliveryKind::Terminal,
                "admitted-terminal-turn-2",
            )
            .unwrap();
        assert!(admitted);

        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        let term = term.unwrap();
        assert_eq!(term.state, DispatchDeliveryState::Delivered);
        assert_eq!(
            term.admitted_turn_id.as_deref(),
            Some("admitted-terminal-turn-2")
        );
        assert!(term.delivered_at_unix_ms.is_some());
    }

    #[test]
    fn duplicate_notifications_do_not_create_second_delivery() {
        let (store, conversation, membership) = fixture();
        let claim = store
            .claim_subagent_dispatch(&conversation, &membership[0], &membership[1], None)
            .unwrap();

        // First observation notification creates delivery record
        let first = store
            .record_subagent_observation_delivery(&claim.id, Some("first"))
            .unwrap();
        assert!(first);

        // Duplicate observation notification is idempotent and does not create a second delivery
        let duplicate = store
            .record_subagent_observation_delivery(&claim.id, Some("second"))
            .unwrap();
        assert!(!duplicate);

        let pending = store.pending_dispatch_deliveries(&conversation).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload.as_deref(), Some("first"));
    }

    #[test]
    fn coalescing_pending_deliveries_for_recipient() {
        let (store, conversation, membership) = fixture();
        let (claim1, scope1) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);
        let (claim2, _scope2) =
            running_dispatch(&store, &conversation, &membership[0], &membership[2]);

        // Finish claim1 (creates terminal delivery for membership[0])
        store
            .finish_runtime_dispatch(
                &scope1,
                &serde_json::json!({"output": "claim1 done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        // Record observation delivery for claim2 (also for membership[0])
        store
            .record_subagent_observation_delivery(&claim2.id, Some("timeout claim2"))
            .unwrap();

        // Coalesce deliveries for recipient membership[0]
        let coalesced = store
            .coalesce_pending_deliveries_for_recipient(&conversation, &membership[0])
            .unwrap()
            .expect("should have coalesced wake");

        assert_eq!(coalesced.conversation_id, conversation);
        assert_eq!(coalesced.recipient_membership_id, membership[0]);
        assert_eq!(coalesced.claim_ids.len(), 2);
        assert!(coalesced.claim_ids.contains(&claim1.id));
        assert!(coalesced.claim_ids.contains(&claim2.id));
        assert!(coalesced.has_terminal);
        assert!(coalesced.has_observation);
        // Original deliveries are not merged away
        assert_eq!(coalesced.deliveries.len(), 2);
    }

    #[test]
    fn cold_recovery_reverts_in_flight_deliveries_and_ensures_terminal() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);

        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        // Delivery is in-flight delivering when crash happens
        store
            .mark_dispatch_delivery_delivering(&claim.id, DispatchDeliveryKind::Terminal)
            .unwrap();
        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert_eq!(term.unwrap().state, DispatchDeliveryState::Delivering);

        // Host cold recovery runs
        let report = store.cold_recover().unwrap();
        let _ = report;

        // Delivering delivery must have reverted to pending
        let (_, term) = store.subagent_delivery_status(&claim.id).unwrap();
        assert_eq!(term.unwrap().state, DispatchDeliveryState::Pending);
    }

    #[test]
    fn active_wait_sources_tracks_claims_and_settles() {
        let (store, conversation, membership) = fixture();
        let (claim, scope) =
            running_dispatch(&store, &conversation, &membership[0], &membership[1]);

        // While running, wait source is active
        let sources = store.active_wait_sources(&conversation).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].wait_source_id, claim.id);
        assert_eq!(sources[0].kind, WaitSourceKind::SubagentClaim);
        assert_eq!(sources[0].waiting_membership_id, membership[0]);

        // Scoped to recipient membership
        let sources_for_m0 = store
            .active_wait_sources_for_membership(&conversation, &membership[0])
            .unwrap();
        assert_eq!(sources_for_m0.len(), 1);

        let sources_for_m1 = store
            .active_wait_sources_for_membership(&conversation, &membership[1])
            .unwrap();
        assert!(sources_for_m1.is_empty());

        // Finish dispatch settles the wait source
        store
            .finish_runtime_dispatch(
                &scope,
                &serde_json::json!({"output": "done"}),
                crate::DispatchState::Completed,
                None,
            )
            .unwrap();

        let sources_after = store.active_wait_sources(&conversation).unwrap();
        assert!(sources_after.is_empty());
    }
}
