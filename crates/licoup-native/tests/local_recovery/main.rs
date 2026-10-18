//! Local recovery oracle for the complete archive path.
//!
//! One oracle joins the delivered native owners end to end: a synthetic root arranged at
//! the shape the last published release left (tag `v0.2.1`, frozen by the shared
//! `released_source` fixture) is admitted by the client's own migration owner, gains a
//! genuine workflow package revision through the workflow package owner, and is exported
//! to both plaintext containers and imported into disposable homes.
//!
//! What is real here: `client_state_migration::admit`, the archive owner
//! (`export_data_home`/`import_archive` and the Foundation capture/restore beneath them),
//! `ConversationStore`, `StrategyStore`, `StrategyService`,
//! `ClientStateStore`, `PlatformLlmApiKeyVault`, the SQLite engine and the data-home
//! reference rewrite. Every assertion reads a root back through the owner that owns it.
//!
//! # Released shape and identity
//!
//! The released layouts, rows and root documents are the `released_source` fixture frozen
//! from tag `v0.2.1`; this file does not restate them. The released root records product
//! high-water `0.2.1`, which an un-injected development binary must refuse. Run through
//! `tools/scripts/migration-crate-tests.mjs --native-recovery`, which supplies the planned
//! product identity through the native build script. The released ledger, frontier and
//! owner layouts remain unchanged; the test never lowers admission to fit its binary.
//!
//! # Oracle
//!
//! A version marker alone never satisfies this file: the marker-only case admits an empty
//! root and requires the reopened owners to report none of the arranged content. The
//! credential limitation stays explicit: a root whose inventory cannot travel is still
//! reported as `limited`, never as a complete recovery.

use std::fs;
use std::path::{Path, PathBuf};

use licoup_foundation::core::full_data_root_archive::{
    RecoveryCoverage, RestoreRequest, restore_data_root,
};
use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};
use licoup_native::domain::client_conversation::ConversationStore;
use licoup_native::domain::client_state_migration::{
    admit, domain_state_projection, frontier_projection_struct, running_product_version,
};
use licoup_native::domain::local_recovery::{export_data_home, import_archive};
use licoup_native::domain::workflow_runtime::{StrategyService, synthetic_fixture_package_bytes};
use licoup_native::domain::workflow_store::StrategyStore;
use licoup_native::platform::client_state::ClientStateStore;
use licoup_native::platform::llm_api_key_vault::PlatformLlmApiKeyVault;
use rusqlite::Connection;
use serde_json::{Value, json};

include!("../../../../tests/fixtures/client_state_migration/released_source.rs");
include!("../../../../tests/fixtures/client_state_migration/owner_layouts.rs");

const SETTINGS_COLLECTION: &str = "settings";
const CONVERSATION_SNAPSHOT_ROOT_KEY: &str = "conversationSnapshotRoot";
const USER_PROJECT_PATH: &str = "/tmp/unrelated-user-project";
const USER_NOTES: &str = "client-state/user-notes.txt";
const REVISION_STORE: &str = "client-state/adaptive-flywheel/strategy-packages/revisions";
const CREDENTIAL_METADATA: &str = "llm-api-key-inventory.json";

// ---------------------------------------------------------------------------
// Disposable roots and fixtures
// ---------------------------------------------------------------------------

fn scratch(name: &str) -> PathBuf {
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary directory");
    let root = base.join(format!(
        "licoup-local-recovery-{name}-{}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear scratch root");
    }
    fs::create_dir_all(&root).expect("create scratch root");
    root
}

fn write_json_atomic(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent).expect("private document directory");
    }
    atomic_write_private_text(
        path,
        &serde_json::to_string(value).expect("encode document"),
    )
    .expect("private document write");
}

/// Materialize the frozen released root. The released owner layouts are applied by the
/// released producers themselves (the schema and row constants in the fixture).
fn seed_released_source_root(root: &Path) {
    assert_eq!(
        running_product_version().expect("embedded product identity"),
        "0.3.0",
        "run through the candidate-identity Cargo or migration test entry; never lower the released ledger"
    );
    seed_released_conversation_store(root);
    seed_released_strategy_store(&root.join(RELEASED_STRATEGY_DATABASE));
    ensure_private_dir(&root.join("client-state/migrations/domain-state"))
        .expect("marker directory");
    for (relative, content) in released_root_files() {
        let path = root.join(&relative);
        if relative == RELEASED_CONVERSATION_COMPLETION {
            fs::write(&path, content).expect("completion marker");
            continue;
        }
        let document: Value = serde_json::from_str(&content).expect("released document");
        write_json_atomic(&path, &document);
    }
}

fn seed_released_conversation_store(root: &Path) {
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    fs::create_dir_all(database.parent().expect("database parent")).expect("database directory");
    let connection = Connection::open(&database).expect("open released conversation store");
    connection
        .execute_batch(RELEASED_CONVERSATION_SCHEMA)
        .expect("released conversation layout");
    connection
        .execute_batch(RELEASED_CONVERSATION_ROWS)
        .expect("released conversation rows");
    let version: String = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .expect("released conversation schema identity");
    assert_eq!(version, RELEASED_CONVERSATION_SCHEMA_VERSION);
}

fn seed_released_strategy_store(path: &Path) {
    fs::create_dir_all(path.parent().expect("database parent")).expect("database directory");
    let connection = Connection::open(path).expect("open released strategy store");
    connection
        .execute_batch(RELEASED_STRATEGY_SCHEMA)
        .expect("released strategy layout");
    connection
        .execute_batch(&released_strategy_rows())
        .expect("released strategy rows");
    let version: String = connection
        .query_row(
            "SELECT value FROM strategy_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .expect("released strategy schema identity");
    assert_eq!(version, RELEASED_STRATEGY_META_VERSION);
}

/// Arrange current-format owner content and one genuine workflow package revision.
fn arrange_current_owner_content(root: &Path, work: &Path) -> String {
    let store = ClientStateStore::new(root.join("client-state")).expect("client-state owner");
    store
        .write_collection(
            SETTINGS_COLLECTION,
            json!({
                "theme": "orbital-dark",
                CONVERSATION_SNAPSHOT_ROOT_KEY: root.join("snapshots").display().to_string(),
                "userProjectPath": USER_PROJECT_PATH,
            }),
        )
        .expect("arrange settings");
    atomic_write_private_text(
        &root.join(USER_NOTES),
        &format!(
            "User notes mentioning {} as plain text, not an owned reference.",
            root.display()
        ),
    )
    .expect("arrange user notes");

    let service = StrategyService::open(root).expect("strategy service opens");
    let package_path = work.join("synthetic-package.fixture");
    fs::write(
        &package_path,
        synthetic_fixture_package_bytes().expect("synthetic package"),
    )
    .expect("write synthetic package");
    let prepared = service
        .execute(json!({
            "action": "strategy.package.prepare-import",
            "selectionToken": "recovery-oracle",
            "sourcePath": package_path.display().to_string(),
        }))
        .expect("prepare package");
    assert_eq!(prepared["ok"], true, "prepare must succeed: {prepared}");
    let preparation = &prepared["result"];
    let preparation_id = preparation["preparationId"]
        .as_str()
        .expect("preparation id")
        .to_owned();
    let revision_digest = preparation["revisionDigest"]
        .as_str()
        .expect("revision digest")
        .to_owned();
    let committed = service
        .execute(json!({
            "action": "strategy.package.commit-import",
            "preparationId": preparation_id,
            "expectedRevisionDigest": revision_digest,
        }))
        .expect("commit package");
    assert_eq!(committed["ok"], true, "commit must succeed: {committed}");
    revision_digest
}

fn revision_workflow_path(root: &Path, digest: &str) -> PathBuf {
    root.join(REVISION_STORE)
        .join(digest)
        .join("content")
        .join("workflow.json")
}

fn is_readonly(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .expect("entry metadata")
        .permissions()
        .readonly()
}

fn conversation_titles(root: &Path) -> Vec<(String, String)> {
    ConversationStore::open(root)
        .expect("conversation owner opens the root")
        .list(false)
        .expect("conversation owner lists its store")
        .into_iter()
        .map(|summary| (summary.id, summary.title))
        .collect()
}

fn strategy_definition_ids(root: &Path) -> Vec<String> {
    StrategyStore::open(root)
        .expect("strategy owner opens the root")
        .list_definitions()
        .expect("strategy owner lists definitions")
        .into_iter()
        .map(|summary| summary.definition_id)
        .collect()
}

fn settings(root: &Path) -> Value {
    ClientStateStore::new(root.join("client-state"))
        .expect("client-state owner")
        .read_collection(SETTINGS_COLLECTION)
        .expect("settings read")
}

/// One root entry: relative path, kind, byte length and SHA-256 content digest.
fn root_digest(root: &Path) -> Vec<(String, &'static str, u64, String)> {
    use sha2::{Digest, Sha256};

    fn walk(root: &Path, directory: &Path, entries: &mut Vec<(String, &'static str, u64, String)>) {
        let mut children = fs::read_dir(directory)
            .expect("readable directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            let relative = child
                .strip_prefix(root)
                .expect("entry below root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = fs::symlink_metadata(&child).expect("entry metadata");
            assert!(
                !metadata.file_type().is_symlink(),
                "disposable root must not contain a symbolic link: {relative}"
            );
            if metadata.is_dir() {
                entries.push((format!("{relative}/"), "directory", 0, String::new()));
                walk(root, &child, entries);
            } else {
                let bytes = fs::read(&child).expect("entry bytes");
                entries.push((
                    relative,
                    "file",
                    bytes.len() as u64,
                    format!("{:x}", Sha256::digest(&bytes)),
                ));
            }
        }
    }
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries
}

fn credential_entries(root: &Path) -> Vec<String> {
    PlatformLlmApiKeyVault::at_state_root(root)
        .expect("credential inventory owner opens the data root")
        .list()
        .expect("credential inventory lists")
        .entries
        .into_iter()
        .map(|entry| entry.credential_id)
        .collect()
}

fn assert_projection_is_at_target(
    root: &Path,
    admission: &licoup_native::domain::client_state_migration::AdmissionResult,
) {
    let frontier = frontier_projection_struct().expect("embedded frontier");
    let projection = domain_state_projection(root).expect("domain state projection");
    assert_eq!(projection.len(), frontier.domains.len());
    for domain in &frontier.domains {
        if admission
            .pending_authorization_domain_ids
            .contains(&domain.domain_id)
        {
            continue;
        }
        let state = projection
            .iter()
            .find(|state| state.domain_id == domain.domain_id)
            .unwrap_or_else(|| panic!("domain {} is projected", domain.domain_id));
        assert_eq!(
            state.marker_schema_version,
            Some(domain.target_schema_version),
            "{} must carry its target marker: {state:?}",
            domain.domain_id
        );
    }
}

fn writable_fixture_permissions(metadata: &fs::Metadata) -> fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 })
    }
    #[cfg(not(unix))]
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        permissions
    }
}

fn remove_root(root: &Path) {
    if !root.exists() {
        return;
    }
    // Committed workflow revisions are hardened read-only by their owner; clear the
    // hardening before the disposable root can be removed.
    fn make_writable(path: &Path) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return;
        };
        if metadata.file_type().is_symlink() {
            return;
        }
        if metadata.permissions().readonly() {
            let _ = fs::set_permissions(path, writable_fixture_permissions(&metadata));
        }
        if metadata.is_dir()
            && let Ok(entries) = fs::read_dir(path)
        {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    }
    make_writable(root);
    fs::remove_dir_all(root).expect("remove disposable root");
}

// ---------------------------------------------------------------------------
// AC: both containers round-trip an owner-valid converted home across homes
// ---------------------------------------------------------------------------

#[test]
fn local_recovery_released_root_round_trips_both_containers_across_homes() {
    let fixture = scratch("round-trip");
    let released = fixture.join("released");
    let converted = fixture.join("converted");
    ensure_private_dir(&released).expect("released root");
    ensure_private_dir(&converted).expect("converted root");
    seed_released_source_root(&released);
    copy_tree(&released, &converted);

    // The released layouts are converted by the client's own admission owner.
    let admission = admit(&converted).expect("released root is admitted");
    assert_eq!(admission.status, "ready");
    assert_projection_is_at_target(&converted, &admission);
    // The released business rows survive conversion through their owners.
    assert!(
        conversation_titles(&converted).contains(&(
            RELEASED_CONVERSATION_ID.to_owned(),
            "Synthetic released conversation".to_owned()
        )),
        "the released conversation must survive admission"
    );
    // The built-in temporary definitions are excluded from the list projection;
    // the released definition is read back by its own revision identity.
    let released_definition = StrategyStore::open(&converted)
        .expect("strategy owner")
        .definition_by_revision(RELEASED_DEFINITION_REVISION)
        .expect("the released strategy definition must survive admission");
    assert_eq!(
        released_definition.workflow.metadata.id,
        "assistant-temporary"
    );
    assert!(credential_entries(&converted).is_empty());

    let revision_digest = arrange_current_owner_content(&converted, &fixture);
    let workflow_path = revision_workflow_path(&converted, &revision_digest);
    assert!(
        workflow_path.is_file(),
        "the committed revision content exists"
    );
    assert!(
        is_readonly(&workflow_path),
        "the workflow owner froze the committed revision"
    );

    let before_export = root_digest(&converted);
    let archived_notes = fs::read(converted.join(USER_NOTES)).expect("user notes");

    for (name, target_name) in [
        ("home.zip", "restored-zip"),
        ("home.tar.gz", "restored-tar"),
    ] {
        let archive = fixture.join(name);
        let exported = export_data_home(Some(&converted), &archive, true).expect("export");
        assert_eq!(exported.coverage, RecoveryCoverage::Limited);
        assert_eq!(exported.limitations.len(), 1);
        assert_eq!(exported.limitations[0].domain, "gateway-credential-custody");
        assert_eq!(exported.source_home, converted);

        // A raw generic restore publishes the bytes under fresh permissions: the
        // revision protection is gone, which is exactly what the composition repairs.
        let raw_target = fixture.join(format!("{target_name}-raw"));
        let raw = restore_data_root(&RestoreRequest {
            archive_path: archive.clone(),
            target_root: raw_target.clone(),
        })
        .expect("raw restore");
        assert_eq!(raw.source_home, converted);
        let raw_workflow = revision_workflow_path(&raw_target, &revision_digest);
        assert!(raw_workflow.is_file());
        assert!(
            !is_readonly(&raw_workflow),
            "a generic restore does not carry the revision protection"
        );
        remove_root(&raw_target);

        let target = fixture.join(target_name);
        let imported = import_archive(&archive, &target).expect("import");
        assert_eq!(imported.outcome.source_home, converted);
        assert!(imported.relocated, "a different home must be rebased");
        assert_eq!(
            imported.verified_workflow_revisions,
            vec![revision_digest.clone()],
            "the restored revision is verified through the workflow owner"
        );
        assert_eq!(imported.outcome.coverage, RecoveryCoverage::Limited);

        // Every expected member and business canary survives.
        let titles = conversation_titles(&target);
        assert!(
            titles.contains(&(
                RELEASED_CONVERSATION_ID.to_owned(),
                "Synthetic released conversation".to_owned()
            )),
            "the released conversation survives import: {titles:?}"
        );
        let definitions = strategy_definition_ids(&target);
        assert!(
            definitions.contains(&"fixture-entry-worker".to_owned()),
            "the imported strategy definition survives: {definitions:?}"
        );
        let released_definition = StrategyStore::open(&target)
            .expect("strategy owner")
            .definition_by_revision(RELEASED_DEFINITION_REVISION)
            .expect("the released definition survives import");
        assert_eq!(released_definition.summary.name, "Temporary");
        let imported_definition = StrategyStore::open(&target)
            .expect("strategy owner")
            .definition_by_revision(&revision_digest)
            .expect("the imported revision is readable through its owner");
        assert_eq!(
            imported_definition.workflow.metadata.id,
            "fixture-entry-worker"
        );
        assert!(
            is_readonly(&revision_workflow_path(&target, &revision_digest)),
            "import re-establishes the immutable revision protection"
        );

        // Owner-managed references are rebased; user content and unrelated paths are not.
        let restored_settings = settings(&target);
        assert_eq!(
            restored_settings[CONVERSATION_SNAPSHOT_ROOT_KEY]
                .as_str()
                .expect("snapshot root"),
            target.join("snapshots").display().to_string(),
            "the owned snapshot root follows the restored home"
        );
        assert_eq!(
            restored_settings["userProjectPath"].as_str(),
            Some(USER_PROJECT_PATH),
            "an unrelated absolute path is never rewritten"
        );
        assert_eq!(
            fs::read(target.join(USER_NOTES)).expect("restored notes"),
            archived_notes,
            "plain user content is byte-identical after a different-home restore"
        );

        assert_eq!(
            root_digest(&converted),
            before_export,
            "the captured source is byte-identical after export and {name}"
        );
        remove_root(&target);
    }

    remove_root(&fixture);
}

/// Recursively copy a private tree, preserving permissions, without following links.
fn copy_tree(source: &Path, destination: &Path) {
    let mut entries = fs::read_dir(source)
        .expect("read source root")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        let relative = entry.strip_prefix(source).expect("entry below source");
        let target = destination.join(relative);
        let metadata = fs::symlink_metadata(&entry).expect("entry metadata");
        if metadata.is_dir() {
            ensure_private_dir(&target).expect("private directory");
            copy_tree(&entry, &target);
        } else {
            fs::copy(&entry, &target).expect("copy entry");
            fs::set_permissions(&target, metadata.permissions()).expect("entry permissions");
        }
    }
}

// ---------------------------------------------------------------------------
// AC: incomplete recovery stays explicit
// ---------------------------------------------------------------------------

#[test]
fn local_recovery_missing_credential_metadata_stays_limited() {
    let fixture = scratch("limited");
    let source = fixture.join("source");
    ensure_private_dir(&source).expect("source root");
    seed_released_source_root(&source);
    admit(&source).expect("admission");
    // The protected credential metadata did not travel in this root.
    fs::remove_file(source.join(CREDENTIAL_METADATA)).expect("remove credential metadata");
    assert!(credential_entries(&source).is_empty());

    let archive = fixture.join("limited.zip");
    let exported = export_data_home(Some(&source), &archive, true).expect("export");
    assert_eq!(exported.coverage, RecoveryCoverage::Limited);
    assert_eq!(exported.limitations.len(), 1);
    assert_eq!(exported.limitations[0].domain, "gateway-credential-custody");
    assert!(
        exported.limitations[0].reason.contains("absent"),
        "the limitation names the absent metadata: {}",
        exported.limitations[0].reason
    );

    let target = fixture.join("restored");
    let imported = import_archive(&archive, &target).expect("import");
    assert_eq!(imported.outcome.coverage, RecoveryCoverage::Limited);
    assert_eq!(imported.outcome.limitations, exported.limitations);
    assert!(
        credential_entries(&target).is_empty(),
        "the limitation is truthful: no credential metadata was invented"
    );
    assert!(!target.join(CREDENTIAL_METADATA).exists());

    remove_root(&fixture);
}

// ---------------------------------------------------------------------------
// AC: failed owner preparation preserves the destination before publication
// ---------------------------------------------------------------------------

/// Owner preparation occurs before publication. Failure leaves no created root,
/// preserves the caller's existing empty directory and never changes the source archive.
#[test]
fn failed_owner_preparation_preserves_the_destination_before_publication() {
    let fixture = scratch("checked-cleanup");
    let source = fixture.join("source");
    ensure_private_dir(&source).expect("source root");
    seed_released_source_root(&source);
    admit(&source).expect("admission");
    let work = fixture.join("work");
    fs::create_dir_all(&work).expect("work directory");
    let revision_digest = arrange_current_owner_content(&source, &work);

    // Corrupt the committed revision content: this is exactly the drift the workflow
    // package owner refuses when it re-freezes and reads the revision back.
    let workflow = revision_workflow_path(&source, &revision_digest);
    let metadata = fs::metadata(&workflow).expect("revision metadata");
    fs::set_permissions(&workflow, writable_fixture_permissions(&metadata))
        .expect("unfreeze the revision for the corruption");
    fs::write(&workflow, b"{\"drifted\":true}").expect("corrupt the revision content");

    let archive = fixture.join("drifted.zip");
    let exported = export_data_home(Some(&source), &archive, true)
        .expect("the archive owner captures bytes; revision validation is the importer's step");
    assert_eq!(exported.coverage, RecoveryCoverage::Limited);

    // A failed preparation never publishes a newly created destination.
    let created = fixture.join("created-target");
    let error = import_archive(&archive, &created).expect_err("the drift is refused");
    assert_eq!(error.to_string(), "strategy_revision_content_drifted");
    assert!(
        !created.exists(),
        "a failed preparation never leaves a false recovered destination"
    );
    assert!(
        archive.is_file(),
        "the source archive survives every failed import"
    );

    // A destination the caller named remains in its original empty state.
    let named = fixture.join("named-target");
    ensure_private_dir(&named).expect("caller-named empty destination");
    let error = import_archive(&archive, &named).expect_err("the drift is refused");
    assert_eq!(error.to_string(), "strategy_revision_content_drifted");
    assert!(named.is_dir(), "the caller's own directory is not deleted");
    assert_eq!(
        fs::read_dir(&named)
            .expect("readable caller directory")
            .count(),
        0,
        "the caller's directory holds only the state it had before the import"
    );
    assert!(archive.is_file());

    remove_root(&fixture);
}

// ---------------------------------------------------------------------------
// AC: a version marker alone never satisfies the oracle
// ---------------------------------------------------------------------------

#[test]
fn local_recovery_a_version_marker_alone_does_not_satisfy_the_oracle() {
    let fixture = scratch("marker-only");
    let marker_only = fixture.join("markers");
    ensure_private_dir(&marker_only).expect("marker-only root");

    // Admission on an empty root writes the version markers; the released stores
    // never existed. This is the shape a marker-only recovery claim would rest on.
    let admission = admit(&marker_only).expect("admission on an empty root");
    assert_projection_is_at_target(&marker_only, &admission);

    // The same owner readback the end-to-end oracle uses rejects it, because no
    // owner reports the arranged content.
    assert!(
        conversation_titles(&marker_only).is_empty(),
        "a marker-only root holds no conversation"
    );
    assert!(
        strategy_definition_ids(&marker_only).is_empty(),
        "a marker-only root holds no strategy definition"
    );
    assert!(
        credential_entries(&marker_only).is_empty(),
        "a marker-only root holds no credential metadata"
    );
    let marker_settings = settings(&marker_only);
    assert!(
        marker_settings
            .get(CONVERSATION_SNAPSHOT_ROOT_KEY)
            .is_none(),
        "a marker-only root holds no arranged collection: {marker_settings}"
    );

    remove_root(&fixture);
}
