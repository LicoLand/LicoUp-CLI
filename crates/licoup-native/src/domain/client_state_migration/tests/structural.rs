use super::*;

fn assert_preserved(root: &Path, before: &BTreeMap<String, Vec<u8>>) {
    let after = durable_file_bytes(root);
    for (path, bytes) in before {
        assert!(
            after.get(path) == Some(bytes),
            "changed pre-existing durable file: {path}"
        );
    }
    // Admission may acquire its existing lock, but may not publish migration
    // artifacts, domain markers or a replacement database on refusal.
    let added = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect::<Vec<_>>();
    assert!(
        added
            .iter()
            .all(|path| path.as_str() == "client-state/migrations/admission.lock"),
        "new durable files: {added:?}"
    );
}

fn replace_conversation(root: &Path, version: u32, ddl: &str, before: &str, after: &str) {
    let path = root.join(RELEASED_CONVERSATION_DATABASE);
    fs::remove_file(&path).unwrap();
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(before).unwrap();
    connection.execute_batch(ddl).unwrap();
    connection
        .execute(
            "INSERT INTO schema_meta VALUES ('version',?1)",
            [version.to_string()],
        )
        .unwrap();
    connection.execute_batch(after).unwrap();
}

#[test]
fn supported_owner_layouts_admit_read_write_and_reopen() {
    for version in (1..=12).chain([18]) {
        let root =
            std::env::temp_dir().join(format!("licoup-owner-layout-{}", uuid::Uuid::new_v4()));
        seed_released_source_root(&root);
        let rows = RELEASED_CONVERSATION_ROWS.replace(
            "INSERT INTO schema_meta(key, value) VALUES ('version', '12');",
            "",
        );
        let rows = if version <= 4 {
            rows.replace(", runtime_cursor", "")
                .replace("content', NULL, 1)", "content', 1)")
        } else {
            rows
        };
        replace_conversation(
            &root,
            version,
            &supported_conversation_schema(version),
            "",
            &rows,
        );
        let before = durable_file_bytes(&root);
        let connection = Connection::open_with_flags(
            root.join(RELEASED_CONVERSATION_DATABASE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            licoup_conversation::store::validate_migration_source(&connection).unwrap(),
            Some(version.to_string()),
            "schema {version}"
        );
        drop(connection);
        assert_preserved(&root, &before);
        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap().status,
            "ready",
            "schema {version}"
        );
        let owner = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
        let conversation = owner.get(RELEASED_CONVERSATION_ID).unwrap();
        assert_eq!(conversation.title, "Synthetic released conversation");
        assert_eq!(
            conversation.memberships[0].principal.display_name,
            "Synthetic Principal"
        );
        assert_eq!(
            owner
                .page_events(RELEASED_CONVERSATION_ID, None, 10)
                .unwrap()
                .events[0]
                .id,
            RELEASED_EVENT_ID
        );
        let guard = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
        guard
            .execute_batch("PRAGMA foreign_keys=ON; BEGIN;")
            .unwrap();
        assert!(
            guard
                .execute(
                    "INSERT INTO principals VALUES ('invalid-kind','other','Synthetic',NULL,1)",
                    []
                )
                .is_err()
        );
        guard.execute_batch("INSERT INTO subagent_dispatch_claims(id,conversation_id,caller_membership_id,target_membership_id,depth,state,created_at,updated_at) VALUES ('claim','released-conversation','membership-1','membership-1',1,'claimed',1,1);").unwrap();
        assert!(guard.execute("INSERT INTO subagent_dispatch_claims(id,conversation_id,caller_membership_id,target_membership_id,depth,state,created_at,updated_at) VALUES ('duplicate','released-conversation','membership-1','membership-1',1,'claimed',1,1)", []).is_err());
        guard.execute_batch("ROLLBACK;").unwrap();
        drop(guard);
        owner
            .rename_conversation(RELEASED_CONVERSATION_ID, "Synthetic updated title")
            .unwrap();
        // Normal owner insertion exercises defaults, optional values and FK/PK
        // constraints, not just a successful connection or SELECT 1.
        let principal = licoup_conversation::Principal {
            id: "new-principal".into(),
            kind: licoup_conversation::PrincipalKind::Human,
            display_name: "New synthetic principal".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let created = owner
            .create_conversation("Synthetic new conversation", principal)
            .unwrap();
        let created_id = created.id.clone();
        drop(owner);
        let reopened = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
        assert_eq!(
            reopened.get(&created_id).unwrap().title,
            "Synthetic new conversation"
        );
        assert_eq!(
            reopened.get(RELEASED_CONVERSATION_ID).unwrap().title,
            "Synthetic updated title"
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn released_strategy_producer_variants_support_normal_owner_writes() {
    for nullable in [false, true] {
        let root =
            std::env::temp_dir().join(format!("licoup-strategy-writes-{}", uuid::Uuid::new_v4()));
        seed_released_source_root(&root);
        if nullable {
            let path = root.join(RELEASED_STRATEGY_DATABASE);
            fs::remove_file(&path).unwrap();
            let connection = Connection::open(path).unwrap();
            connection
                .execute_batch(&released_strategy_schema_producer_upgraded())
                .unwrap();
            connection.execute_batch(&released_strategy_rows()).unwrap();
            connection
                .execute("UPDATE strategy_runs SET terminal=NULL", [])
                .unwrap();
        }
        admit_as_version(&root, "0.3.0").unwrap();
        let owner = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
        let definition = owner
            .definition_by_revision(RELEASED_DEFINITION_REVISION)
            .unwrap();
        let inserted = owner
            .register_definition(
                "sha256:synthetic-new-definition",
                "sha256:synthetic-new-semantics",
                &definition.workflow,
                0,
                2,
            )
            .unwrap();
        assert_eq!(inserted.workflow, definition.workflow);
        assert_eq!(
            owner
                .run(RELEASED_RUN_ID)
                .unwrap()
                .conversation_id
                .as_deref(),
            Some(RELEASED_CONVERSATION_ID)
        );
        drop(owner);
        let reopened = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
        assert_eq!(
            reopened
                .definition_by_revision("sha256:synthetic-new-definition")
                .unwrap()
                .workflow,
            definition.workflow
        );
        let terminal: i64 = Connection::open(root.join(RELEASED_STRATEGY_DATABASE))
            .unwrap()
            .query_row(
                "SELECT terminal FROM strategy_runs WHERE run_id=?1",
                [RELEASED_RUN_ID],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(terminal, 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn structural_defects_refuse_before_any_durable_admission_write() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../../../tests/fixtures/client_state_migration/structural_cases.json"
    ))
    .unwrap();
    for version in [11, 12, 18] {
        for case in &cases {
            let name = case["name"].as_str().unwrap();
            let root = std::env::temp_dir().join(format!(
                "licoup-structural-refusal-{}",
                uuid::Uuid::new_v4()
            ));
            seed_released_source_root(&root);
            let mut ddl = supported_conversation_schema(version);
            if let Some(from) = case["from"].as_str() {
                assert!(ddl.contains(from), "{version}: {name}");
                ddl = ddl.replace(from, case["to"].as_str().unwrap());
            }
            // IF NOT EXISTS makes the malformed, pre-existing upgrade-created
            // table survive even for an otherwise complete current fixture.
            if case["before"].as_bool() == Some(true) {
                ddl = ddl.replace(
                    "CREATE TABLE subagent_dispatch_deliveries",
                    "CREATE TABLE IF NOT EXISTS subagent_dispatch_deliveries",
                );
            }
            let sql = case["sql"].as_str().unwrap_or("");
            let (before, after) = if case["before"].as_bool() == Some(true) {
                (sql, "")
            } else {
                ("", sql)
            };
            replace_conversation(&root, version, &ddl, before, after);
            if let Some(rows) = case["rows"].as_str() {
                Connection::open(root.join(RELEASED_CONVERSATION_DATABASE))
                    .unwrap()
                    .execute_batch(rows)
                    .unwrap();
            }
            let bytes = durable_file_bytes(&root);
            let connection = Connection::open_with_flags(
                root.join(RELEASED_CONVERSATION_DATABASE),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert!(
                licoup_conversation::store::validate_migration_source(&connection).is_err(),
                "schema {version}: {name}"
            );
            drop(connection);
            assert_eq!(
                admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
                "unsupported_state_shape",
                "schema {version}: {name}"
            );
            assert_preserved(&root, &bytes);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn strategy_retained_defaults_actions_and_auxiliary_tables_refuse_without_writes() {
    for (from, to, extra) in [
        ("model TEXT NOT NULL DEFAULT ''", "model TEXT NOT NULL", ""),
        (
            "REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE",
            "REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE ON UPDATE CASCADE",
            "",
        ),
        (
            "model TEXT NOT NULL DEFAULT ''",
            "model TEXT NOT NULL DEFAULT 'unexpected'",
            "",
        ),
        (
            "",
            "",
            "CREATE TABLE workflow_queue(request_id TEXT PRIMARY KEY);",
        ),
        (
            "",
            "",
            "CREATE TRIGGER deny_run BEFORE INSERT ON strategy_runs BEGIN SELECT RAISE(ABORT,'synthetic refusal'); END;",
        ),
    ] {
        for version in [2, 3] {
            let root = std::env::temp_dir()
                .join(format!("licoup-strategy-refusal-{}", uuid::Uuid::new_v4()));
            seed_released_source_root(&root);
            let path = root.join(RELEASED_STRATEGY_DATABASE);
            fs::remove_file(&path).unwrap();
            let database = Connection::open(&path).unwrap();
            let ddl = if from.is_empty() {
                RELEASED_STRATEGY_SCHEMA.to_owned()
            } else {
                RELEASED_STRATEGY_SCHEMA.replace(from, to)
            };
            database.execute_batch(&ddl).unwrap();
            database.execute_batch(extra).unwrap();
            database
                .execute("UPDATE strategy_meta SET value=?1", [version.to_string()])
                .unwrap();
            drop(database);
            let before = durable_file_bytes(&root);
            assert_eq!(
                admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
                "unsupported_state_shape"
            );
            assert_preserved(&root, &before);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
