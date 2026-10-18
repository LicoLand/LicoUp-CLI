//! Endpoint-local native history membership. This is not portable Conversation data.

use super::*;

pub(super) const TABLE: &str = "CREATE TABLE IF NOT EXISTS conversation_native_sessions (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    membership_id TEXT NOT NULL REFERENCES memberships(id) ON DELETE CASCADE,
    native_session_id TEXT NOT NULL,
    PRIMARY KEY (conversation_id, membership_id, native_session_id)
);";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSessionReference {
    pub membership_id: String,
    pub agent_id: String,
    pub native_session_id: String,
}

impl ConversationStore {
    /// Safe identities for the local native catalog only; paths, bindings and
    /// dispatch handles remain private and are absent from portable exports.
    pub fn native_session_references(
        &self,
        conversation_id: &str,
    ) -> StoreResult<Vec<NativeSessionReference>> {
        validate_identifier(conversation_id, "conversation_id")?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.membership_id, p.agent_id, s.native_session_id
                 FROM conversation_native_sessions s
                 JOIN memberships m ON m.id=s.membership_id AND m.conversation_id=s.conversation_id
                 JOIN principals p ON p.id=m.principal_id
                 WHERE s.conversation_id=?1 AND p.kind='agent'
                 ORDER BY p.agent_id, s.native_session_id, s.membership_id",
            )?;
            statement
                .query_map(params![conversation_id], |row| {
                    Ok(NativeSessionReference {
                        membership_id: row.get(0)?,
                        agent_id: row.get(1)?,
                        native_session_id: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    /// The native sessions Lico archived for one agent. Browse catalogs filter
    /// these out; the agent's own on-disk history is never deleted. This
    /// read-only view opens the store by its portable root so history scans in
    /// helper processes never touch a live writer.
    pub fn archived_native_session_ids(
        portable_root: &Path,
        agent_id: &str,
    ) -> StoreResult<std::collections::BTreeSet<String>> {
        let path = portable_root
            .join("client-state")
            .join("conversations")
            .join(DATABASE_FILE);
        if !path.is_file() {
            return Ok(Default::default());
        }
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: String = connection.query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )?;
        if version != CURRENT_SCHEMA_VERSION {
            return Ok(Default::default());
        }
        let mut statement = connection
            .prepare("SELECT native_session_id FROM archived_native_sessions WHERE agent_id=?1")?;
        let ids = statement
            .query_map(params![agent_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
        Ok(ids)
    }
}

pub(super) fn record_native_session(
    connection: &impl CountedSqlite,
    conversation_id: &str,
    membership_id: &str,
    native_session_id: &str,
) -> StoreResult<()> {
    if native_session_id.trim().is_empty() {
        return Ok(());
    }
    let inserted = connection.execute(
        "INSERT INTO conversation_native_sessions(conversation_id,membership_id,native_session_id)
         SELECT ?1,m.id,?3 FROM memberships m JOIN principals p ON p.id=m.principal_id
         WHERE m.id=?2 AND m.conversation_id=?1 AND p.kind='agent'
         ON CONFLICT DO NOTHING",
        params![conversation_id, membership_id, native_session_id],
    )?;
    if inserted > 0 {
        bump_revision(connection, conversation_id, now_ms())?;
    }
    Ok(())
}

pub(super) fn backfill_native_sessions(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(
        "INSERT OR IGNORE INTO conversation_native_sessions
         SELECT rb.conversation_id,rb.membership_id,rb.runtime_session_id
         FROM runtime_bindings rb JOIN memberships m ON m.id=rb.membership_id AND m.conversation_id=rb.conversation_id
         JOIN principals p ON p.id=m.principal_id
         WHERE p.kind='agent' AND length(trim(rb.runtime_session_id))>0;
         INSERT OR IGNORE INTO conversation_native_sessions
         SELECT d.conversation_id,d.membership_id,json_extract(d.native_provenance,'$.nativeSessionId')
         FROM conversation_dispatches d JOIN memberships m ON m.id=d.membership_id AND m.conversation_id=d.conversation_id
         JOIN principals p ON p.id=m.principal_id
         WHERE d.native_provenance IS NOT NULL AND p.kind='agent'
           AND p.agent_id=json_extract(d.native_provenance,'$.agentId')
           AND length(trim(json_extract(d.native_provenance,'$.nativeSessionId')))>0;",
    )?;
    // The old source key is a byte-length-prefixed adapter identity. Decode
    // that exact stored format; never infer ownership from project/model/time.
    let links = {
        let mut statement = connection.prepare(
            "SELECT conversation_id,native_identity FROM source_links WHERE source_kind='agent-runtime'",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (conversation_id, identity) in links {
        let Some((length, rest)) = identity.split_once(':') else {
            continue;
        };
        let Ok(length) = length.parse::<usize>() else {
            continue;
        };
        let Some(agent_id) = rest.get(..length) else {
            continue;
        };
        let Some(session_id) = rest.get(length..).and_then(|tail| tail.strip_prefix(':')) else {
            continue;
        };
        if agent_id.is_empty() || session_id.trim().is_empty() {
            continue;
        }
        connection.execute(
            "INSERT OR IGNORE INTO conversation_native_sessions
             SELECT m.conversation_id,m.id,?3 FROM memberships m JOIN principals p ON p.id=m.principal_id
             WHERE m.conversation_id=?1 AND p.kind='agent' AND p.agent_id=?2",
            params![conversation_id,agent_id,session_id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(store: &ConversationStore) -> (String, String) {
        let conversation = store
            .create_conversation_with_members(
                "Synthetic group",
                Principal {
                    id: "human:synthetic".into(),
                    kind: PrincipalKind::Human,
                    display_name: "Human".into(),
                    agent_id: None,
                    created_at_unix_ms: 1,
                },
                &[(
                    Principal {
                        id: "agent:synthetic".into(),
                        kind: PrincipalKind::Agent,
                        display_name: "Agent".into(),
                        agent_id: Some("synthetic".into()),
                        created_at_unix_ms: 1,
                    },
                    MembershipAccess::Member,
                )],
            )
            .unwrap();
        let member = conversation
            .memberships
            .iter()
            .find(|m| m.principal.kind == PrincipalKind::Agent)
            .unwrap()
            .id
            .clone();
        (conversation.id, member)
    }

    fn dispatch(
        store: &ConversationStore,
        group: &(String, String),
        session: &str,
    ) -> ConversationRuntimeScope {
        store
            .prepare_runtime_dispatch(
                "synthetic",
                session,
                "Synthetic request",
                Some(&group.0),
                Some(&group.1),
                None,
                None,
            )
            .unwrap()
    }

    #[test]
    fn group_native_sessions_preserve_history_and_isolate_groups_after_reopen() {
        let root = std::env::temp_dir().join(format!("licoup-group-membership-{}", Uuid::new_v4()));
        path_security::ensure_private_dir(&root).unwrap();
        let (first, second, empty, provenance_only) = {
            let store = ConversationStore::open(&root).unwrap();
            let first = group(&store);
            let second = group(&store);
            let empty = group(&store);
            let provenance_only = group(&store);
            let provider = dispatch(&store, &provenance_only, "");
            store
                .record_native_execution_provenance(
                    &provider,
                    "synthetic",
                    "provider-confirmed",
                    Some("provider-turn"),
                    None,
                )
                .unwrap();
            let old = dispatch(&store, &first, "old");
            store
                .bind_runtime_session(&old, "synthetic", "old", None, None)
                .unwrap();
            let new = dispatch(&store, &first, "");
            store
                .bind_runtime_session(&new, "synthetic", "new", None, None)
                .unwrap();
            store
                .bind_runtime_session(&new, "synthetic", "new", None, None)
                .unwrap();
            let other = dispatch(&store, &second, "other");
            store
                .bind_runtime_session(&other, "synthetic", "other", None, None)
                .unwrap();
            let shared = dispatch(&store, &second, "new");
            store
                .bind_runtime_session(&shared, "synthetic", "new", None, None)
                .unwrap();
            dispatch(&store, &empty, "");
            dispatch(&store, &empty, "unverified-request");
            store.leave_member(&first.0, &first.1).unwrap();
            store.checkpoint().unwrap();
            (first, second, empty, provenance_only)
        };
        let store = ConversationStore::open(&root).unwrap();
        let ids = |id: &str| {
            store
                .native_session_references(id)
                .unwrap()
                .into_iter()
                .map(|r| r.native_session_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first.0), vec!["new", "old"]);
        assert_eq!(ids(&second.0), vec!["new", "other"]);
        assert!(ids(&empty.0).is_empty());
        assert_eq!(ids(&provenance_only.0), vec!["provider-confirmed"]);
        assert!(
            !serde_json::to_string(&store.get(&first.0).unwrap())
                .unwrap()
                .contains("nativeSession")
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn group_native_sessions_migration_uses_only_explicit_old_evidence() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE conversations(id TEXT PRIMARY KEY);
          INSERT INTO conversations VALUES('g1'),('g2'),('empty');
          CREATE TABLE principals(id TEXT PRIMARY KEY,kind TEXT,agent_id TEXT);
          INSERT INTO principals VALUES('p','agent','synthetic');
          CREATE TABLE memberships(id TEXT PRIMARY KEY,conversation_id TEXT,principal_id TEXT);
          INSERT INTO memberships VALUES('m1','g1','p'),('m2','g2','p'),('m3','empty','p');
          CREATE TABLE runtime_bindings(conversation_id TEXT,membership_id TEXT,runtime_session_id TEXT);
          INSERT INTO runtime_bindings VALUES('g1','m1','current');
          CREATE TABLE conversation_dispatches(conversation_id TEXT,membership_id TEXT,native_provenance TEXT);
          INSERT INTO conversation_dispatches VALUES('g2','m2','{\"agentId\":\"synthetic\",\"nativeSessionId\":\"historical\"}');
          CREATE TABLE source_links(conversation_id TEXT,source_kind TEXT,native_identity TEXT);
          INSERT INTO source_links VALUES('g1','agent-runtime','9:synthetic:older'),('g1','agent-runtime','9:synthetic:current'),
            ('empty','agent-runtime','pending:dispatch'),('empty','agent-runtime','other:unknown');").unwrap();
        connection.execute_batch(TABLE).unwrap();
        backfill_native_sessions(&connection).unwrap();
        let mut statement = connection.prepare("SELECT conversation_id,native_session_id FROM conversation_native_sessions ORDER BY 1,2").unwrap();
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("g1".into(), "current".into()),
                ("g1".into(), "older".into()),
                ("g2".into(), "historical".into())
            ]
        );
    }
}
