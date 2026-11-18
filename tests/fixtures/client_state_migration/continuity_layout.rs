// Shared Rust/JavaScript producer fixture. The SQL payload is frozen independently
// from validator expectations and is source code, not a database or user-data dump.
pub const CURRENT_CONTINUITY_SCHEMA: &str = r#"-- Frozen schema-only output of the actual native ConversationService/continuity owner.
-- No user records or validator-generated expected layout are included.
ALTER TABLE conversations ADD COLUMN designation_epoch INTEGER NOT NULL DEFAULT 0;
CREATE TABLE continuity_agreements (
 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, scope TEXT NOT NULL,
 statement_ref TEXT NOT NULL, origin TEXT NOT NULL, effective_revision INTEGER NOT NULL,
 supersedes INTEGER, valid_from INTEGER NOT NULL, valid_until INTEGER,
 revocation_generation INTEGER NOT NULL, superseded_by TEXT, deleted_at INTEGER
);
CREATE TABLE continuity_completion_transitions (
 notification_id TEXT PRIMARY KEY, goal_id TEXT NOT NULL UNIQUE, transition TEXT NOT NULL,
 consumed INTEGER NOT NULL DEFAULT 1, created_at INTEGER NOT NULL
);
CREATE TABLE continuity_derived (
 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, kind TEXT NOT NULL,
 source_opaque_id TEXT NOT NULL, body TEXT NOT NULL, revocation_generation INTEGER NOT NULL,
 invalidated INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER
);
CREATE TABLE continuity_effects (
 logical_effect_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, goal_id TEXT,
 status TEXT NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE continuity_goals (
 goal_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, matter_id TEXT NOT NULL,
 contract TEXT NOT NULL, progress TEXT NOT NULL, lifecycle TEXT NOT NULL,
 control TEXT NOT NULL, revision INTEGER NOT NULL, next_due INTEGER, deleted_at INTEGER
);
CREATE TABLE continuity_idempotency (
 request_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, payload TEXT NOT NULL, receipt TEXT NOT NULL
);
CREATE TABLE continuity_matter_associations (
 conversation_id TEXT NOT NULL, source_event_id TEXT NOT NULL, interpretation_key TEXT NOT NULL,
 matter_id TEXT NOT NULL, association_revision INTEGER NOT NULL, proposed_by TEXT NOT NULL,
 reason_code TEXT NOT NULL, source_ref TEXT NOT NULL, supersedes INTEGER,
 PRIMARY KEY (conversation_id, source_event_id, interpretation_key)
);
CREATE TABLE continuity_matters (
 id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, revision INTEGER NOT NULL, label TEXT NOT NULL,
 association_refs TEXT NOT NULL, created_event TEXT NOT NULL, status TEXT NOT NULL,
 deleted_at INTEGER, deletion_generation INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE continuity_outbox (
 logical_wake_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, goal_id TEXT NOT NULL,
 payload TEXT NOT NULL, created_at INTEGER NOT NULL, consumed_at INTEGER, settlement TEXT NOT NULL
);
CREATE TABLE continuity_parent_grants (
 grant_id TEXT PRIMARY KEY, source_conversation_id TEXT NOT NULL, recipient_conversation_id TEXT NOT NULL,
 recipient_membership_id TEXT NOT NULL, source_refs TEXT NOT NULL, authorized_scopes TEXT NOT NULL,
 status TEXT NOT NULL, request_id TEXT NOT NULL, revocation_generation INTEGER NOT NULL
);
CREATE TABLE continuity_qualification_evidence (
 responsibility_id TEXT NOT NULL, identity_key TEXT NOT NULL, payload TEXT NOT NULL,
 evidence_class TEXT NOT NULL, ingested_at INTEGER NOT NULL, PRIMARY KEY (responsibility_id, identity_key)
);
CREATE TABLE continuity_qualification_invalidations (
 responsibility_id TEXT NOT NULL, identity_key TEXT NOT NULL, withdrawn_at INTEGER NOT NULL,
 PRIMARY KEY (responsibility_id, identity_key)
);
CREATE TABLE continuity_schema (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE continuity_scope (
 conversation_id TEXT PRIMARY KEY, revocation_generation INTEGER NOT NULL DEFAULT 0,
 acl_generation INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE continuity_source_cursors (
 conversation_id TEXT NOT NULL, source_event_id TEXT NOT NULL, interpretation_key TEXT NOT NULL,
 payload TEXT NOT NULL, PRIMARY KEY (conversation_id, source_event_id, interpretation_key)
);
CREATE TABLE continuity_source_revocations (
 conversation_id TEXT NOT NULL, opaque_id TEXT NOT NULL, revocation_generation INTEGER NOT NULL,
 deleted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (conversation_id, opaque_id)
);
CREATE TABLE continuity_task_relations (
 goal_id TEXT PRIMARY KEY, parent_conversation_id TEXT NOT NULL, child_conversation_id TEXT NOT NULL,
 card_event_id TEXT NOT NULL, card_sequence INTEGER NOT NULL, card_part_id TEXT, listing_kind TEXT NOT NULL,
 follow_through_kind TEXT NOT NULL, created_event TEXT NOT NULL, completion_transition TEXT, revision INTEGER NOT NULL
);
CREATE TABLE continuity_work_contexts (
 conversation_id TEXT NOT NULL, membership_id TEXT NOT NULL, matter_id TEXT NOT NULL,
 generation INTEGER NOT NULL, status TEXT NOT NULL, last_reconciled INTEGER NOT NULL,
 PRIMARY KEY (conversation_id, membership_id, matter_id, generation)
);
CREATE INDEX continuity_agreements_scope_idx ON continuity_agreements(conversation_id, revocation_generation, id);
CREATE INDEX continuity_completion_pending_idx ON continuity_completion_transitions(consumed, created_at, notification_id);
CREATE INDEX continuity_derived_src_idx ON continuity_derived(conversation_id, revocation_generation, invalidated, id);
CREATE INDEX continuity_goals_state_idx ON continuity_goals(conversation_id, lifecycle, next_due, goal_id);
CREATE INDEX continuity_matters_scope_idx ON continuity_matters(conversation_id, revision, id);
CREATE INDEX continuity_outbox_goal_idx ON continuity_outbox(goal_id, settlement, created_at);
CREATE INDEX continuity_outbox_pending_idx ON continuity_outbox(conversation_id, consumed_at, logical_wake_id);
CREATE INDEX continuity_parent_grants_recipient_idx ON continuity_parent_grants(recipient_conversation_id, recipient_membership_id, revocation_generation, grant_id);
CREATE INDEX continuity_source_cursors_key_idx ON continuity_source_cursors(interpretation_key);
CREATE INDEX continuity_source_cursors_outstanding_settlement_idx ON continuity_source_cursors(conversation_id, source_event_id, interpretation_key) WHERE interpretation_key LIKE 'settlement:%:settlement-pending';
CREATE INDEX continuity_source_cursors_outstanding_work_idx ON continuity_source_cursors(conversation_id, source_event_id, interpretation_key) WHERE interpretation_key LIKE 'work:%:child-work-pending';
CREATE UNIQUE INDEX continuity_task_relations_child_idx ON continuity_task_relations(child_conversation_id);
CREATE INDEX continuity_task_relations_parent_idx ON continuity_task_relations(parent_conversation_id, card_sequence, goal_id);
CREATE TRIGGER continuity_bump_designation_epoch
 AFTER UPDATE OF assistant_membership_id ON conversations
 WHEN (OLD.assistant_membership_id IS NOT NEW.assistant_membership_id)
 BEGIN
  UPDATE conversations SET designation_epoch = designation_epoch + 1 WHERE id = NEW.id;
 END;
INSERT INTO continuity_schema(key,value) VALUES ('version','7');
"#;
