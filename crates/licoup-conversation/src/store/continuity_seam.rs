use super::{
    ConversationStore, CountedSqlite, CountedTransaction, NewEventPart, StoreResult,
    ensure_membership_profile_default, insert_event, new_id, now_ms, upsert_principal,
};
use crate::client_conversation::{
    ConversationEvent, EventKind, EventPartKind, Principal, PrincipalKind,
};
use crate::continuity::generated::{
    ContinuityCommitBasis, ContinuityParentCardAnchor, ContinuitySourceOwnerKind,
    ContinuitySourceRef, ContinuitySourceValidity, ContinuityVisibilityScope,
};
use crate::continuity::migrate::ensure_continuity_schema;
use anyhow::anyhow;
use rusqlite::{OptionalExtension, Params, Row, TransactionBehavior};
use serde_json::Value;

/// Bounded Conversation-store unit of work for continuity tables.
///
/// The checkout cannot escape this closure. M1 durable-state owns statements
/// and calls [`Self::request_commit`].
pub struct ContinuityUnitOfWork<'a> {
    txn: Option<CountedTransaction<'a>>,
    commit_requested: bool,
}

pub struct CanonicalChildRecord {
    pub conversation_id: String,
    pub owner_membership_id: String,
}

pub struct ParentCardRecord {
    pub event: ConversationEvent,
    pub anchor: ContinuityParentCardAnchor,
    pub created_event: ContinuitySourceRef,
}

impl ContinuityUnitOfWork<'_> {
    pub fn request_commit(&mut self) {
        self.commit_requested = true;
    }

    /// Failed units must not keep an earlier commit request. `run_unit`
    /// maps `ContinuityFailure` to a seam `Ok`, so a dirty-migration
    /// `request_commit` would otherwise persist partial business writes.
    pub fn abandon(&mut self) {
        self.commit_requested = false;
    }

    pub fn execute<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.txn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?
            .execute(sql, params)
    }

    pub fn query_row<T, P, F>(&self, sql: &str, params: P, mapper: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.txn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?
            .query_row(sql, params, mapper)
    }

    /// One counted `prepare` plus a keyset page. Callers must not loop
    /// `query_row` per result.
    pub fn query_vec<T, P, F>(&self, sql: &str, params: P, mapper: F) -> rusqlite::Result<Vec<T>>
    where
        P: Params,
        F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
    {
        let txn = self
            .txn
            .as_ref()
            .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;
        let mut statement = CountedSqlite::prepare(txn, sql)?;
        let mapped = statement.query_map(params, mapper)?;
        mapped.collect()
    }

    pub fn read_commit_basis(&self, conversation_id: &str) -> StoreResult<ContinuityCommitBasis> {
        let (revision, epoch, assistant): (i64, i64, Option<String>) = self.query_row(
            "SELECT revision, designation_epoch, assistant_membership_id
             FROM conversations WHERE id=?1",
            [conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(ContinuityCommitBasis {
            conversation_id: conversation_id.to_owned(),
            revision,
            designation_epoch: epoch,
            assistant_membership_id: assistant,
        })
    }

    pub fn conversation_exists(&self, conversation_id: &str) -> StoreResult<bool> {
        let found: i64 = self.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?1)",
            [conversation_id],
            |row| row.get(0),
        )?;
        Ok(found != 0)
    }

    pub fn event_exists(&self, conversation_id: &str, event_id: &str) -> StoreResult<bool> {
        let found: i64 = self.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM events WHERE id=?1 AND conversation_id=?2
             )",
            rusqlite::params![event_id, conversation_id],
            |row| row.get(0),
        )?;
        Ok(found != 0)
    }

    pub fn current_human_owner_membership(
        &self,
        conversation_id: &str,
    ) -> StoreResult<Option<String>> {
        self.query_row(
            "SELECT m.id FROM memberships m
             JOIN principals p ON p.id=m.principal_id
             WHERE m.conversation_id=?1 AND m.status='active'
               AND m.access='owner' AND p.kind='human'
             ORDER BY m.joined_at ASC, m.id ASC LIMIT 1",
            [conversation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn membership_is_active_agent(
        &self,
        conversation_id: &str,
        membership_id: &str,
    ) -> StoreResult<bool> {
        let found: i64 = self.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM memberships m
               JOIN principals p ON p.id=m.principal_id
               WHERE m.id=?1 AND m.conversation_id=?2
                 AND m.status='active' AND p.kind='agent'
             )",
            rusqlite::params![membership_id, conversation_id],
            |row| row.get(0),
        )?;
        Ok(found != 0)
    }

    pub fn event_author_and_sequence(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> StoreResult<Option<(Option<String>, i64)>> {
        self.query_row(
            "SELECT author_membership_id, sequence
             FROM events WHERE id=?1 AND conversation_id=?2",
            rusqlite::params![event_id, conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn event_canonical_parts(
        &self,
        conversation_id: &str,
        event_id: &str,
    ) -> StoreResult<Vec<(String, String, String)>> {
        if !self.event_exists(conversation_id, event_id)? {
            return Ok(Vec::new());
        }
        self.query_vec(
            "SELECT id, kind, content FROM event_parts
             WHERE event_id=?1 AND runtime_cursor IS NULL
             ORDER BY ordinal ASC, id ASC",
            [event_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(Into::into)
    }

    pub fn bump_revision_cas(&self, conversation_id: &str, expected: i64) -> StoreResult<i64> {
        let changed = self.execute(
            "UPDATE conversations
             SET revision=revision+1, updated_at=?3
             WHERE id=?1 AND revision=?2",
            rusqlite::params![conversation_id, expected, now_ms()],
        )?;
        if changed == 0 {
            return Err(anyhow!("conversation_revision_stale"));
        }
        Ok(expected + 1)
    }

    pub fn create_canonical_child(
        &self,
        parent_conversation_id: &str,
        title: &str,
        requested_id: Option<&str>,
    ) -> StoreResult<CanonicalChildRecord> {
        let txn = self
            .txn
            .as_ref()
            .ok_or_else(|| anyhow!("continuity_txn_missing"))?;
        if let Some(requested_id) = requested_id.filter(|id| !id.is_empty()) {
            let existing: Option<String> = self
                .query_row(
                    "SELECT id FROM conversations WHERE id=?1",
                    [requested_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(conversation_id) = existing {
                let owner: String = self.query_row(
                    "SELECT id FROM memberships
                     WHERE conversation_id=?1 AND access='owner' AND status='active'
                     ORDER BY joined_at ASC, id ASC LIMIT 1",
                    [conversation_id.as_str()],
                    |row| row.get(0),
                )?;
                return Ok(CanonicalChildRecord {
                    conversation_id,
                    owner_membership_id: owner,
                });
            }
        }

        let owner: (String, String, String, Option<String>, i64) = self.query_row(
            "SELECT p.id, p.display_name, p.kind, p.agent_id, p.created_at
             FROM memberships m
             JOIN principals p ON p.id=m.principal_id
             WHERE m.conversation_id=?1 AND m.access='owner' AND m.status='active'
             ORDER BY m.joined_at ASC, m.id ASC LIMIT 1",
            [parent_conversation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        let owner_principal = Principal {
            id: owner.0,
            display_name: owner.1,
            kind: if owner.2 == "agent" {
                PrincipalKind::Agent
            } else {
                PrincipalKind::Human
            },
            agent_id: owner.3,
            created_at_unix_ms: owner.4,
        };
        let assistant: Option<(String, String, String, Option<String>, i64)> = self
            .query_row(
                "SELECT p.id, p.display_name, p.kind, p.agent_id, p.created_at
                 FROM conversations c
                 JOIN memberships m ON m.id=c.assistant_membership_id
                 JOIN principals p ON p.id=m.principal_id
                 WHERE c.id=?1",
                [parent_conversation_id],
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
            .optional()?
            .or_else(|| {
                self.query_row(
                    "SELECT p.id, p.display_name, p.kind, p.agent_id, p.created_at
                     FROM memberships m
                     JOIN principals p ON p.id=m.principal_id
                     WHERE m.conversation_id=?1 AND m.status='active' AND p.kind='agent'
                     ORDER BY m.joined_at ASC, m.id ASC LIMIT 1",
                    [parent_conversation_id],
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
                .optional()
                .ok()
                .flatten()
            });

        let child_id = requested_id
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| new_id("conversation"));
        let now = now_ms();
        let is_group = assistant
            .as_ref()
            .is_some_and(|row| row.0 != owner_principal.id && row.2 == "agent");
        upsert_principal(txn, &owner_principal)?;
        txn.execute(
            "INSERT INTO conversations(
               id, title, archived, pinned, is_group, revision, created_at, updated_at
             ) VALUES (?1, ?2, 0, 0, ?3, 0, ?4, ?4)",
            rusqlite::params![child_id, title, is_group as i64, now],
        )?;
        let owner_membership_id = new_id("membership");
        txn.execute(
            "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
             VALUES (?1, ?2, ?3, 'owner', 'active', ?4)",
            rusqlite::params![owner_membership_id, child_id, owner_principal.id, now],
        )?;
        if owner_principal.kind == PrincipalKind::Agent {
            ensure_membership_profile_default(txn, &owner_membership_id, now)?;
        }
        let assistant_membership_id = if let Some(assistant) = assistant {
            if assistant.0 != owner_principal.id {
                let member = Principal {
                    id: assistant.0,
                    display_name: assistant.1,
                    kind: if assistant.2 == "agent" {
                        PrincipalKind::Agent
                    } else {
                        PrincipalKind::Human
                    },
                    agent_id: assistant.3,
                    created_at_unix_ms: assistant.4,
                };
                upsert_principal(txn, &member)?;
                let membership_id = new_id("membership");
                txn.execute(
                    "INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at)
                     VALUES (?1, ?2, ?3, 'member', 'active', ?4)",
                    rusqlite::params![membership_id, child_id, member.id, now],
                )?;
                if member.kind == PrincipalKind::Agent {
                    ensure_membership_profile_default(txn, &membership_id, now)?;
                }
                Some(membership_id)
            } else {
                Some(owner_membership_id.clone())
            }
        } else {
            None
        };
        if let Some(assistant_membership_id) = assistant_membership_id.as_deref() {
            txn.execute(
                "UPDATE conversations SET assistant_membership_id=?1 WHERE id=?2",
                rusqlite::params![assistant_membership_id, child_id],
            )?;
        }
        Ok(CanonicalChildRecord {
            conversation_id: child_id,
            owner_membership_id,
        })
    }

    pub fn append_parent_task_card(
        &self,
        parent_conversation_id: &str,
        author_membership_id: Option<&str>,
        metadata: &Value,
    ) -> StoreResult<ParentCardRecord> {
        let txn = self
            .txn
            .as_ref()
            .ok_or_else(|| anyhow!("continuity_txn_missing"))?;
        let event = insert_event(
            txn,
            &new_id("event"),
            parent_conversation_id,
            author_membership_id,
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Metadata,
                content: serde_json::to_string(metadata)?,
            }],
            None,
            None,
            true,
            true,
            now_ms(),
        )?;
        let part_id = event
            .parts
            .first()
            .map(|part| part.id.clone())
            .ok_or_else(|| anyhow!("continuity_card_part_missing"))?;
        let created_event = ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Event,
            opaque_id: event.id.clone(),
            part_id: Some(part_id.clone()),
            span: None,
            source_revision: event.sequence,
            digest: format!("card:{}:{}", event.id, part_id),
            visibility_scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        };
        Ok(ParentCardRecord {
            anchor: ContinuityParentCardAnchor {
                parent_conversation_id: parent_conversation_id.to_owned(),
                event_id: event.id.clone(),
                sequence: event.sequence,
                part_id: Some(part_id),
            },
            created_event,
            event,
        })
    }

    pub fn update_event_part_content(&self, part_id: &str, content: &Value) -> StoreResult<()> {
        let changed = self.execute(
            "UPDATE event_parts SET content=?2 WHERE id=?1",
            rusqlite::params![part_id, serde_json::to_string(content)?],
        )?;
        if changed == 0 {
            return Err(anyhow!("conversation_event_not_found"));
        }
        Ok(())
    }
}

impl ConversationStore {
    /// Idempotent continuity schema entry. Call before designation-dependent
    /// admission. M2 must invoke this during host startup before serving
    /// continuity work; it is not optional at production handoff.
    pub fn ensure_continuity_migrated(&self) -> StoreResult<bool> {
        self.with_continuity_unit_of_work(|unit| {
            let dirty = ensure_continuity_schema(unit)?;
            if dirty {
                unit.request_commit();
            }
            Ok(dirty)
        })
    }

    pub fn continuity_revocation_generation(&self, conversation_id: &str) -> StoreResult<i64> {
        self.ensure_continuity_migrated()?;
        self.with_connection(|connection| {
            let generation: Option<i64> = connection
                .query_row(
                    "SELECT revocation_generation FROM continuity_scope WHERE conversation_id=?1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(generation.unwrap_or(0))
        })
    }

    pub fn continuity_commit_basis(
        &self,
        conversation_id: &str,
    ) -> StoreResult<ContinuityCommitBasis> {
        self.ensure_continuity_migrated()?;
        self.with_connection(|connection| {
            let (revision, epoch, assistant): (i64, i64, Option<String>) = connection.query_row(
                "SELECT revision, designation_epoch, assistant_membership_id
                 FROM conversations WHERE id=?1",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok(ContinuityCommitBasis {
                conversation_id: conversation_id.to_owned(),
                revision,
                designation_epoch: epoch,
                assistant_membership_id: assistant,
            })
        })
    }

    pub fn with_continuity_unit_of_work<T>(
        &self,
        work: impl FnOnce(&mut ContinuityUnitOfWork<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.with_connection(|connection| {
            let txn = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut unit = ContinuityUnitOfWork {
                txn: Some(txn),
                commit_requested: false,
            };
            let result = work(&mut unit);
            if result.is_ok() && unit.commit_requested {
                let txn = unit
                    .txn
                    .take()
                    .ok_or_else(|| anyhow!("continuity_txn_missing"))?;
                txn.commit()?;
            }
            result
        })
    }
}
