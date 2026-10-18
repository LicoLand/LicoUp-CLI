// Independent target-layout fixture, frozen from the actual Conversation writer
// at cd2b5c8a (schema18). Applied to RELEASED_CONVERSATION_SCHEMA, not to a
// validator-generated schema. The released fixture remains immutable.
pub const CURRENT_CONVERSATION_ADDITIONS: &str = r#"
ALTER TABLE event_parts ADD COLUMN execution_kind TEXT;
ALTER TABLE conversation_dispatches ADD COLUMN request_payload TEXT;
ALTER TABLE conversation_dispatches ADD COLUMN terminal_payload TEXT;
ALTER TABLE conversation_dispatches ADD COLUMN native_provenance TEXT;
CREATE INDEX conversation_dispatches_native_provenance_idx
 ON conversation_dispatches(json_extract(native_provenance,'$.agentId'),json_extract(native_provenance,'$.nativeSessionId'))
 WHERE native_provenance IS NOT NULL;
CREATE INDEX event_parts_runtime_replay_idx ON event_parts(event_id,runtime_cursor,ordinal)
 WHERE runtime_cursor IS NOT NULL;
CREATE TABLE subagent_dispatch_deliveries (
 claim_id TEXT NOT NULL REFERENCES subagent_dispatch_claims(id) ON DELETE CASCADE,
 kind TEXT NOT NULL CHECK(kind IN ('observation','terminal')),
 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
 recipient_membership_id TEXT NOT NULL REFERENCES memberships(id),
 state TEXT NOT NULL CHECK(state IN ('pending','delivering','delivered','failed')),
 terminal_state TEXT, payload TEXT, attempt_count INTEGER NOT NULL DEFAULT 0,
 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, delivered_at INTEGER,
 admitted_turn_id TEXT, PRIMARY KEY (claim_id,kind)
);
CREATE INDEX subagent_dispatch_deliveries_pending_idx ON subagent_dispatch_deliveries(state,conversation_id,recipient_membership_id,updated_at ASC);
CREATE TABLE archived_native_sessions (
 agent_id TEXT NOT NULL, native_session_id TEXT NOT NULL,
 conversation_id TEXT NOT NULL, archived_at INTEGER NOT NULL,
 PRIMARY KEY(agent_id,native_session_id)
);
CREATE TABLE conversation_native_sessions (
 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
 membership_id TEXT NOT NULL REFERENCES memberships(id) ON DELETE CASCADE,
 native_session_id TEXT NOT NULL,
 PRIMARY KEY(conversation_id,membership_id,native_session_id)
);
ALTER TABLE membership_profiles ADD COLUMN preferred_permission_mode TEXT;
ALTER TABLE membership_profiles ADD COLUMN preferred_instructions TEXT;
ALTER TABLE membership_profiles ADD COLUMN preferred_context_window INTEGER;
CREATE TABLE event_admission_keys (
 conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
 admission_key TEXT NOT NULL,
 event_id TEXT,
 sequence INTEGER,
 created_at INTEGER NOT NULL,
 PRIMARY KEY(conversation_id, admission_key)
);
"#;

// Variants follow the retained owner's existing structural upgrade branches,
// not a new historical product endpoint. The business columns absent before
// schema5/schema6/schema7/schema9 must be supplied by the real upgrade.
pub fn supported_conversation_schema(version: u32) -> String {
    let mut sql = RELEASED_CONVERSATION_SCHEMA.to_owned();
    if version <= 4 {
        sql = sql.replace("runtime_cursor INTEGER, ", "");
    }
    if version <= 5 {
        sql = sql.replace(", strategy_revision TEXT", "");
    }
    if version <= 6 {
        sql = sql.replace(
            "assistant_membership_id TEXT REFERENCES memberships(id),",
            "",
        );
    }
    if version <= 8 {
        sql = sql.replace(", preferred_reasoning_effort TEXT", "");
    }
    if version <= 9 {
        let start = sql
            .find("CREATE TABLE IF NOT EXISTS subagent_dispatch_claims")
            .unwrap();
        let end = sql
            .find("CREATE TABLE IF NOT EXISTS subagent_mcp_inbound")
            .unwrap();
        sql.replace_range(start..end, "");
    }
    if version <= 10 {
        let start = sql
            .find("CREATE TABLE IF NOT EXISTS subagent_mcp_inbound")
            .unwrap();
        let end = sql
            .find("CREATE TABLE IF NOT EXISTS migration_provenance")
            .unwrap();
        sql.replace_range(start..end, "");
    }
    if version < 12 {
        sql = sql.replace("CREATE UNIQUE INDEX IF NOT EXISTS memberships_principal_unique\n            ON memberships(conversation_id, principal_id);", "CREATE UNIQUE INDEX memberships_active_unique ON memberships(conversation_id,principal_id) WHERE status='active';");
    }
    if version >= 18 {
        sql.push_str(CURRENT_CONVERSATION_ADDITIONS);
    }
    sql
}
