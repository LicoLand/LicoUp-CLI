// Frozen fixture: the state the last published client leaves.
//
// Provenance is the immutable release tag v0.2.1
// (db0fc4d7ae875332f8b0cab28cda3f3337ac3c9e), not the current frontier and not
// a developer build. The layouts below are the *final* released layouts: the
// released Conversation initializer converges an existing store by dropping
// the partial membership index, creating the principal uniqueness index and
// the pinned conversation index, and adding the late binding/claim columns;
// the released strategy initializer creates the seven `strategy_*` tables and
// then the active-conversation index. `strategy_meta.version` is `'2'` and the
// Conversation inner schema is `'12'`.
//
// The released catalog names `licoup-state-0.1.1`, lists eleven domains (there
// is no `gateway-credential-custody`), every domain sits at target version 1,
// and every domain's only step is `<domainId>.absent-to-1`. The packaged
// product high-water is `0.2.1`.
//
// This file is shared data, included by test targets (unit, recovery and
// standalone migration) with a relative `include!("...")`. It uses only
// `std` and free functions: no synthetic data reaches a normal runtime build,
// and the released layout is defined once, here.

pub const SOURCE_FRONTIER_ID: &str = "licoup-state-0.1.1";
pub const SOURCE_PRODUCT_VERSION: &str = "0.2.1";
pub const LEDGER_SCHEMA: &str = "v0.0.1:client-state-migration-ledger-1";
pub const DOMAIN_MARKER_SCHEMA: &str = "v0.0.1:client-state-domain-marker-1";
pub const RELEASED_DOMAINS: &[&str] = &[
    "adaptive-flywheel",
    "agent-tab-order",
    "agent-tool-allowlist",
    "appearance-presentation",
    "canonical-conversation",
    "client-state",
    "current-view",
    "mobile-home-layout",
    "mobile-relay",
    "skill-hub-preferences",
    "workspace-manifest",
];

pub const RELEASED_CONVERSATION_DATABASE: &str = "client-state/conversations/conversations.sqlite3";
pub const RELEASED_CONVERSATION_SCHEMA_VERSION: &str = "12";
pub const RELEASED_CONVERSATION_COMPLETION: &str =
    "client-state/conversations/migration-v5.complete";
pub const RELEASED_CONVERSATION_COMPLETION_CONTENT: &str = "schema=v5\nstatus=complete\n";
pub const RELEASED_STRATEGY_DATABASE: &str = "client-state/adaptive-flywheel/strategies.sqlite3";
pub const RELEASED_STRATEGY_META_VERSION: &str = "2";
pub const RELEASED_INVENTORY_FILE: &str = "llm-api-key-inventory.json";
pub const RELEASED_INVENTORY_SCHEMA: &str = "licoup.llm-api-key-inventory.v1";

pub const RELEASED_CONVERSATION_ID: &str = "released-conversation";
pub const RELEASED_EVENT_ID: &str = "released-event-1";
pub const RELEASED_DEFINITION_REVISION: &str = "sha256:released-definition";
pub const RELEASED_RUN_ID: &str = "released-run-1";
/// A published legacy workflow document: the actor slot carries no `entry`
/// flag (the field's serde default is false), so the owner's legacy
/// normalization marks the first actor slot as the entry when it converts the
/// definition. Its other fields must survive the rewrite unchanged.
pub const RELEASED_WORKFLOW_JSON: &str = r#"{"schema":"licoup.adaptive-flywheel.workflow.v1","metadata":{"id":"assistant-temporary","name":"Temporary","version":"1"},"limits":{"maxParallelism":2,"maxWorksetItems":16,"maxAttempts":2},"actorSlots":[{"id":"actor","kind":"actor","label":"Actor","required":true}],"runtimes":[],"worksets":[],"initial":"run","states":[{"id":"run","kind":"actor","label":"Run","binding":"actor"},{"id":"done","kind":"succeed","label":"Done"},{"id":"failed","kind":"fail","label":"Failed"}],"transitions":[{"id":"done","from":"run","to":"done","event":"success"},{"id":"failed","from":"run","to":"failed","event":"failure"}]}"#;

/// A valid published `RunSnapshot` serialization for the fixture run.
pub const RELEASED_RUN_SNAPSHOT_JSON: &str = r#"{"runId":"released-run-1","definitionDigest":"sha256:released-definition","semanticsDigest":"sha256:released-semantics","status":"completed","sequence":2,"input":{},"activeStates":[],"completedStates":["done"],"stateVisits":{"run":1,"done":1},"joinArrivals":{},"conversationId":"released-conversation","commands":{}}"#;

/// A valid published `ReducerEvent` serialization for the fixture run event.
pub const RELEASED_RUN_EVENT_JSON: &str = r#"{"type":"start","input":{}}"#;

/// The final released Conversation layout, including the converged indexes.
pub const RELEASED_CONVERSATION_SCHEMA: &str = r#"
         CREATE TABLE IF NOT EXISTS schema_meta (
           key TEXT PRIMARY KEY, value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS principals (
           id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('human','agent')),
           display_name TEXT NOT NULL, agent_id TEXT, created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS conversations (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
           pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1)),
           is_group INTEGER NOT NULL DEFAULT 0 CHECK(is_group IN (0,1)), strategy_revision TEXT,
           assistant_membership_id TEXT REFERENCES memberships(id),
           revision INTEGER NOT NULL DEFAULT 0,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS conversations_updated_idx ON conversations(updated_at DESC, id DESC);
         CREATE INDEX IF NOT EXISTS conversations_pinned_updated_idx ON conversations(pinned DESC, updated_at DESC, id DESC);
         CREATE TABLE IF NOT EXISTS memberships (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           principal_id TEXT NOT NULL REFERENCES principals(id), access TEXT NOT NULL CHECK(access IN ('owner','member')),
           status TEXT NOT NULL CHECK(status IN ('active','left')), joined_at INTEGER NOT NULL, left_at INTEGER
         );
         CREATE UNIQUE INDEX IF NOT EXISTS memberships_principal_unique
           ON memberships(conversation_id, principal_id);
         CREATE INDEX IF NOT EXISTS memberships_conversation_idx ON memberships(conversation_id, status, joined_at);
         CREATE TABLE IF NOT EXISTS membership_profiles (
           membership_id TEXT PRIMARY KEY REFERENCES memberships(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL,
           responsibility TEXT NOT NULL DEFAULT 'member' CHECK(responsibility IN ('assistant','member')),
           required_capabilities TEXT NOT NULL DEFAULT '[]',
           preferred_capabilities TEXT NOT NULL DEFAULT '[]',
           skill_references TEXT NOT NULL DEFAULT '[]',
           preferred_model TEXT, preferred_reasoning_effort TEXT,
           preferred_environment TEXT,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS membership_profiles_membership_idx
           ON membership_profiles(membership_id, revision);
         CREATE TABLE IF NOT EXISTS events (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           sequence INTEGER NOT NULL, author_membership_id TEXT REFERENCES memberships(id), kind TEXT NOT NULL,
           causation_id TEXT, correlation_id TEXT,
           created_at INTEGER NOT NULL, finalized INTEGER NOT NULL DEFAULT 0 CHECK(finalized IN (0,1)),
           UNIQUE(conversation_id, sequence)
         );
         CREATE INDEX IF NOT EXISTS events_conversation_idx ON events(conversation_id, sequence);
         CREATE TABLE IF NOT EXISTS event_parts (
           id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
           ordinal INTEGER NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
           runtime_cursor INTEGER, created_at INTEGER NOT NULL,
           UNIQUE(event_id, ordinal)
         );
         CREATE INDEX IF NOT EXISTS event_parts_event_idx ON event_parts(event_id, ordinal);
         CREATE TABLE IF NOT EXISTS direct_turns (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           source_event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id),
           state TEXT NOT NULL, ordinal INTEGER NOT NULL,
           UNIQUE(source_event_id, membership_id)
         );
         CREATE INDEX IF NOT EXISTS direct_turns_pending_idx
           ON direct_turns(state, conversation_id, ordinal);
         CREATE VIRTUAL TABLE IF NOT EXISTS event_search USING fts5(event_id UNINDEXED, conversation_id UNINDEXED, content);
         CREATE TABLE IF NOT EXISTS source_links (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           source_kind TEXT NOT NULL, native_identity TEXT NOT NULL,
           UNIQUE(source_kind, native_identity)
         );
         CREATE TABLE IF NOT EXISTS runtime_bindings (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id) ON DELETE CASCADE, lane TEXT NOT NULL,
           availability TEXT NOT NULL, safe_reason TEXT,
           runtime_session_id TEXT, runtime_conversation_path TEXT, working_directory TEXT,
           UNIQUE(conversation_id, membership_id, lane)
         );
         CREATE TABLE IF NOT EXISTS conversation_dispatches (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id),
           operation TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('accepted','running','completed','failed','cancel-requested','cancelled')),
           session_mode TEXT NOT NULL CHECK(session_mode IN ('new','resume')),
           runtime_conversation_path TEXT, error_code TEXT,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
          CREATE INDEX IF NOT EXISTS conversation_dispatches_resume_idx
            ON conversation_dispatches(conversation_id, membership_id, state, updated_at DESC);
          CREATE TABLE IF NOT EXISTS subagent_dispatch_claims (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            caller_membership_id TEXT NOT NULL REFERENCES memberships(id),
            target_membership_id TEXT NOT NULL REFERENCES memberships(id),
            parent_dispatch_id TEXT REFERENCES subagent_dispatch_claims(id),
            depth INTEGER NOT NULL CHECK(depth BETWEEN 1 AND 8),
            state TEXT NOT NULL CHECK(state IN (
              'claimed','running','cancel-requested','reconciliation-required',
              'completed','failed','cancelled'
            )),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            watchdog_deadline_unix_ms INTEGER
          );
          CREATE UNIQUE INDEX IF NOT EXISTS subagent_dispatch_claims_active_edge
            ON subagent_dispatch_claims(conversation_id, caller_membership_id, target_membership_id)
            WHERE state IN ('claimed','running','cancel-requested','reconciliation-required');
          CREATE INDEX IF NOT EXISTS subagent_dispatch_claims_parent_idx
            ON subagent_dispatch_claims(parent_dispatch_id);
          CREATE INDEX IF NOT EXISTS subagent_dispatch_claims_target_idx
            ON subagent_dispatch_claims(conversation_id, target_membership_id, updated_at DESC);
          CREATE TABLE IF NOT EXISTS subagent_mcp_inbound (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            caller_membership_id TEXT,
            target_membership_id TEXT,
            tool TEXT NOT NULL CHECK(tool IN (
              'lico_subagent_delegate','lico_subagent_continue','lico_subagent_cancel'
            )),
            outcome TEXT NOT NULL,
            created_at INTEGER NOT NULL
          );
          CREATE INDEX IF NOT EXISTS subagent_mcp_inbound_edge_idx
            ON subagent_mcp_inbound(
              conversation_id, caller_membership_id, target_membership_id, created_at, id
            );
         CREATE TABLE IF NOT EXISTS migration_provenance (
           source_kind TEXT NOT NULL, source_identity TEXT NOT NULL,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           PRIMARY KEY(source_kind, source_identity)
         );"#;

/// Representative released Conversation business rows.
pub const RELEASED_CONVERSATION_ROWS: &str = "
         INSERT INTO schema_meta(key, value) VALUES ('version', '12');
         INSERT INTO principals(id, kind, display_name, agent_id, created_at)
           VALUES ('principal-1', 'human', 'Synthetic Principal', NULL, 1);
         INSERT INTO conversations(id, title, created_at, updated_at)
           VALUES ('released-conversation', 'Synthetic released conversation', 1, 1);
         INSERT INTO memberships(id, conversation_id, principal_id, access, status, joined_at, left_at)
           VALUES ('membership-1', 'released-conversation', 'principal-1', 'owner', 'active', 1, NULL);
         INSERT INTO events(id, conversation_id, sequence, author_membership_id, kind, causation_id, correlation_id, created_at, finalized)
           VALUES ('released-event-1', 'released-conversation', 1, 'membership-1', 'message', NULL, NULL, 1, 1);
         INSERT INTO event_parts(id, event_id, ordinal, kind, content, runtime_cursor, created_at)
           VALUES ('released-part-1', 'released-event-1', 0, 'text', 'Synthetic released content', NULL, 1);";

/// The released strategy layout: the seven-table batch and the active
/// conversation index the released initializer creates afterwards.
pub const RELEASED_STRATEGY_SCHEMA: &str = r#"CREATE TABLE IF NOT EXISTS strategy_meta(
           key TEXT PRIMARY KEY, value TEXT NOT NULL
         );
         INSERT INTO strategy_meta(key, value) VALUES ('version', '2')
           ON CONFLICT(key) DO NOTHING;
         CREATE TABLE IF NOT EXISTS strategy_definitions(
           definition_id TEXT NOT NULL,
           revision_digest TEXT PRIMARY KEY,
           semantics_digest TEXT NOT NULL,
           name TEXT NOT NULL,
           version TEXT NOT NULL,
           workflow_json TEXT NOT NULL,
           asset_count INTEGER NOT NULL,
           imported_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS strategy_definitions_id_idx
           ON strategy_definitions(definition_id, imported_at DESC);
         CREATE TABLE IF NOT EXISTS strategy_bindings(
           revision_digest TEXT NOT NULL REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE,
           slot_id TEXT NOT NULL,
           ordinal INTEGER NOT NULL DEFAULT 0,
           value_id TEXT NOT NULL,
           model TEXT NOT NULL DEFAULT '',
           reasoning_effort TEXT NOT NULL DEFAULT '',
           revision INTEGER NOT NULL,
           PRIMARY KEY(revision_digest, slot_id, ordinal)
         );
         CREATE TABLE IF NOT EXISTS strategy_authorizations(
           revision_digest TEXT NOT NULL REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE,
           revision INTEGER NOT NULL,
           semantics_digest TEXT NOT NULL,
           binding_digest TEXT NOT NULL,
           authorization_digest TEXT NOT NULL,
           active INTEGER NOT NULL,
           created_at INTEGER NOT NULL,
           PRIMARY KEY(revision_digest, revision)
         );
         CREATE UNIQUE INDEX IF NOT EXISTS strategy_authorization_active_idx
           ON strategy_authorizations(revision_digest) WHERE active=1;
         CREATE TABLE IF NOT EXISTS strategy_runs(
           run_id TEXT PRIMARY KEY,
           revision_digest TEXT NOT NULL REFERENCES strategy_definitions(revision_digest),
           semantics_digest TEXT NOT NULL,
           idempotency_key TEXT NOT NULL UNIQUE,
           request_digest TEXT NOT NULL,
           snapshot_json TEXT NOT NULL,
           conversation_id TEXT,
           terminal INTEGER NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS strategy_runs_revision_idx
           ON strategy_runs(revision_digest, updated_at DESC);
         CREATE INDEX IF NOT EXISTS strategy_runs_active_conversation_idx
           ON strategy_runs(revision_digest, conversation_id, terminal, updated_at DESC);
         CREATE TABLE IF NOT EXISTS strategy_run_events(
           run_id TEXT NOT NULL REFERENCES strategy_runs(run_id) ON DELETE CASCADE,
           sequence INTEGER NOT NULL,
           event_type TEXT NOT NULL,
           event_json TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           PRIMARY KEY(run_id, sequence)
         );
         CREATE TABLE IF NOT EXISTS strategy_commands(
           command_id TEXT PRIMARY KEY,
           run_id TEXT NOT NULL REFERENCES strategy_runs(run_id) ON DELETE CASCADE,
           state_id TEXT NOT NULL,
           kind TEXT NOT NULL,
           status TEXT NOT NULL,
           attempt INTEGER NOT NULL,
           attempt_token TEXT NOT NULL,
           command_json TEXT NOT NULL,
           lease_owner TEXT,
           lease_until INTEGER,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS strategy_commands_ready_idx
           ON strategy_commands(status, command_id);
         CREATE INDEX IF NOT EXISTS strategy_commands_lease_idx
           ON strategy_commands(lease_until) WHERE status IN ('claimed', 'running');"#;

pub fn released_step_id(domain_id: &str) -> String {
    format!("{domain_id}.absent-to-1")
}

/// Representative released strategy business rows for a definition, its
/// ordinal binding, its authorization, one run and one run event.
pub fn released_strategy_rows() -> String {
    format!(
        "INSERT INTO strategy_definitions(
           definition_id, revision_digest, semantics_digest, name, version,
           workflow_json, asset_count, imported_at
         ) VALUES (
           'assistant-temporary', '{RELEASED_DEFINITION_REVISION}', 'sha256:released-semantics',
           'Temporary', '1', '{RELEASED_WORKFLOW_JSON}', 0, 1
         );
         INSERT INTO strategy_bindings(
           revision_digest, slot_id, ordinal, value_id, model, reasoning_effort, revision
         ) VALUES ('{RELEASED_DEFINITION_REVISION}', 'actor', 0, 'lico-basic', '', '', 1);
         INSERT INTO strategy_authorizations(
           revision_digest, revision, semantics_digest, binding_digest, authorization_digest, active, created_at
         ) VALUES (
           '{RELEASED_DEFINITION_REVISION}', 1, 'sha256:released-semantics', 'sha256:released-bindings',
           'sha256:released-authorization', 1, 1
         );
         INSERT INTO strategy_runs(
           run_id, revision_digest, semantics_digest, idempotency_key, request_digest,
           snapshot_json, conversation_id, terminal, created_at, updated_at
         ) VALUES (
           '{RELEASED_RUN_ID}', '{RELEASED_DEFINITION_REVISION}', 'sha256:released-semantics',
           'released-idempotency', 'sha256:released-request', '{RELEASED_RUN_SNAPSHOT_JSON}',
           '{RELEASED_CONVERSATION_ID}', 1, 1, 1
         );
         INSERT INTO strategy_run_events(run_id, sequence, event_type, event_json, created_at)
           VALUES ('{RELEASED_RUN_ID}', 1, 'start', '{RELEASED_RUN_EVENT_JSON}', 1);"
    )
}

/// The schema a store has after the published producer upgraded an older
/// database in place: the same seven tables, but `strategy_runs.terminal` was
/// added by `ensure_column` and is therefore nullable. This is a source the
/// released product itself produced, not a new historical format.
pub fn released_strategy_schema_producer_upgraded() -> String {
    RELEASED_STRATEGY_SCHEMA.replace("terminal INTEGER NOT NULL,", "terminal INTEGER,")
}

pub fn released_marker_json(domain_id: &str) -> String {
    format!(
        "{{\"schemaVersion\":\"{DOMAIN_MARKER_SCHEMA}\",\"domainId\":\"{domain_id}\",\"authoritativeSchemaVersion\":1}}"
    )
}

pub fn released_ledger_json() -> String {
    let domains = RELEASED_DOMAINS
        .iter()
        .map(|domain_id| {
            format!(
                "\"{domain_id}\":{{\"schemaVersion\":1,\"completedStepIds\":[\"{}\"]}}",
                released_step_id(domain_id)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"schemaVersion\":\"{LEDGER_SCHEMA}\",\"highestAdmittedProductVersion\":\"{SOURCE_PRODUCT_VERSION}\",\"frontierId\":\"{SOURCE_FRONTIER_ID}\",\"domains\":{{{domains}}}}}"
    )
}

pub fn released_inventory_json() -> &'static str {
    r#"{"schemaVersion":"licoup.llm-api-key-inventory.v1","leaseDays":7,"entries":[]}"#
}

/// Every non-SQL released root file as (relative path, exact content): the
/// ledger, the eleven domain markers, the Conversation completion marker, and
/// the root-level credential inventory document. No custody marker is included
/// because the released client had no custody domain, and the inventory is
/// metadata rather than custody proof.
pub fn released_root_files() -> Vec<(String, String)> {
    let mut files = Vec::new();
    files.push((
        "client-state/migrations/ledger.json".to_owned(),
        released_ledger_json(),
    ));
    for domain_id in RELEASED_DOMAINS {
        files.push((
            format!("client-state/migrations/domain-state/{domain_id}.json"),
            released_marker_json(domain_id),
        ));
    }
    files.push((
        RELEASED_CONVERSATION_COMPLETION.to_owned(),
        RELEASED_CONVERSATION_COMPLETION_CONTENT.to_owned(),
    ));
    files.push((
        RELEASED_INVENTORY_FILE.to_owned(),
        released_inventory_json().to_owned(),
    ));
    files
}
