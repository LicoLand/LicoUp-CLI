//! The cross-container interoperability oracle: the client CLI and the standalone tool
//! must consume each other's archives.
//!
//! Both entry points already share one owner — the native recovery composition and the
//! Foundation full-data-root owner — so this suite does not test two implementations; it
//! tests that the two entry points really are wired to that one owner and that the
//! archives they produce are interchangeable. The oracle is owner readback: the restored
//! roots are read through the Conversation store, the strategy store, the client-state
//! collection owner and the workflow package owner, not through the archive's own
//! manifest, and never through a marker.
//!
//! The inputs are the frozen `v0.2.1` released fixture, and the suite therefore runs
//! under the planned candidate identity (see `tests/support/mod.rs`). A marker-only root
//! is exercised as the negative control: it must not satisfy this oracle, and the pending
//! credential custody limitation must stay visible on both container paths.

mod support;

use std::fs;
use std::path::Path;

use licoup_native::domain::client_conversation::ConversationStore;
use licoup_native::domain::workflow_runtime::{StrategyService, synthetic_fixture_package_bytes};
use licoup_native::domain::workflow_store::StrategyStore;
use licoup_native::platform::client_state::ClientStateStore;
use licoup_native::platform::llm_api_key_vault::{
    LegacyCredentialMigrationDisposition, PlatformLlmApiKeyVault,
};
use serde_json::json;
use support::*;

const SETTINGS_COLLECTION: &str = "settings";
const SNAPSHOT_ROOT_KEY: &str = "conversationSnapshotRoot";
const UNRELATED_USER_PATH: &str = "/tmp/unrelated-user-project";
const REVISION_STORE: &str = "client-state/adaptive-flywheel/strategy-packages/revisions";
const CREDENTIAL_DOMAIN: &str = "gateway-credential-custody";

fn requires_credential_authorization() -> bool {
    PlatformLlmApiKeyVault::legacy_credential_migration_disposition()
        .expect("platform credential disposition")
        == LegacyCredentialMigrationDisposition::RequiresAuthorization
}

/// Arrange current-format owner content the released producers also write: one settings
/// collection with an owner-managed reference, and one genuine committed workflow revision.
fn arrange_current_owner_content(root: &Path, work: &Path) -> String {
    let store = ClientStateStore::new(root.join("client-state")).expect("client-state owner");
    store
        .write_collection(
            SETTINGS_COLLECTION,
            json!({
                SNAPSHOT_ROOT_KEY: root.join("snapshots").display().to_string(),
                "userProjectPath": UNRELATED_USER_PATH,
            }),
        )
        .expect("arrange settings");

    let service = StrategyService::open(root).expect("strategy service opens");
    let package_path = work.join("synthetic-package.fixture");
    fs::create_dir_all(work).expect("work directory");
    fs::write(
        &package_path,
        synthetic_fixture_package_bytes().expect("synthetic package"),
    )
    .expect("write synthetic package");
    let prepared = service
        .execute(json!({
            "action": "strategy.package.prepare-import",
            "selectionToken": "interoperability-oracle",
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

/// Read one root through its owners and require every released fact to be present.
fn require_owner_readback(root: &Path, restored_snapshot_root: &Path, revision_digest: &str) {
    let conversations: Vec<(String, String)> = ConversationStore::open(root)
        .expect("conversation owner opens the restored root")
        .list(false)
        .expect("conversation owner lists its store")
        .into_iter()
        .map(|summary| (summary.id, summary.title))
        .collect();
    assert!(
        conversations
            .iter()
            .any(|(id, _)| id == RELEASED_CONVERSATION_ID),
        "the restored release's conversation is readable: {conversations:?}"
    );

    let strategies = StrategyStore::open(root).expect("strategy owner opens the restored root");
    let released_definition = strategies
        .definition_by_revision(RELEASED_DEFINITION_REVISION)
        .expect("the restored release's strategy definition is readable by its revision");
    assert_eq!(
        released_definition.workflow.metadata.id,
        "assistant-temporary"
    );
    let definitions: Vec<String> = strategies
        .list_definitions()
        .expect("strategy owner lists definitions")
        .into_iter()
        .map(|summary| summary.definition_id)
        .collect();
    assert!(
        definitions.iter().any(|id| id == "fixture-entry-worker"),
        "the committed workflow revision is listed: {definitions:?}"
    );

    let settings = ClientStateStore::new(root.join("client-state"))
        .expect("client-state owner")
        .read_collection(SETTINGS_COLLECTION)
        .expect("settings read");
    assert_eq!(
        settings[SNAPSHOT_ROOT_KEY].as_str(),
        Some(restored_snapshot_root.display().to_string().as_str()),
        "the owner-managed snapshot reference follows the restored home"
    );
    assert_eq!(
        settings["userProjectPath"].as_str(),
        Some(UNRELATED_USER_PATH),
        "an unrelated user path is not rewritten"
    );

    let workflow = root
        .join(REVISION_STORE)
        .join(revision_digest)
        .join("content")
        .join("workflow.json");
    assert!(
        workflow.is_file(),
        "the committed workflow revision is restored: {}",
        workflow.display()
    );
    let metadata = fs::symlink_metadata(&workflow).expect("revision metadata");
    assert!(
        metadata.permissions().readonly(),
        "the restored revision is re-frozen read-only"
    );
}

/// The marker-only negative control: a root that carries only the durable markers and no
/// owner stores must not satisfy the owner readback oracle.
fn marker_only_root(root: &Path) {
    let ledger = root.join("client-state/migrations/ledger.json");
    fs::create_dir_all(ledger.parent().expect("ledger parent")).expect("ledger directory");
    fs::write(&ledger, released_ledger_json()).expect("ledger");
    let markers = root.join("client-state/migrations/domain-state");
    fs::create_dir_all(&markers).expect("marker directory");
    for domain in RELEASED_DOMAINS {
        fs::write(
            markers.join(format!("{domain}.json")),
            released_marker_json(domain),
        )
        .expect("marker");
    }
}

/// Whether the owner readback oracle accepts one root.
fn owner_readback_accepts(root: &Path) -> bool {
    let conversation = ConversationStore::open(root)
        .ok()
        .and_then(|store| store.list(false).ok())
        .is_some_and(|summaries| {
            summaries
                .iter()
                .any(|summary| summary.id == RELEASED_CONVERSATION_ID)
        });
    let definition = StrategyStore::open(root)
        .ok()
        .and_then(|store| {
            store
                .definition_by_revision(RELEASED_DEFINITION_REVISION)
                .ok()
        })
        .is_some();
    conversation && definition
}

fn limitation_domains(report: &serde_json::Value) -> Vec<String> {
    report["limitations"]
        .as_array()
        .expect("limitations")
        .iter()
        .map(|limitation| limitation["domain"].as_str().expect("domain").to_owned())
        .collect()
}

#[test]
fn the_client_cli_and_the_tool_restore_each_others_archives() {
    assert_candidate_identity();
    let fixture = TestRoot::new("interop");
    let source = fixture.join("source");
    seed_released_root(&source);
    let work = fixture.join("work");
    fs::create_dir_all(&work).expect("work directory");

    // Bring the frozen released root to the candidate format through the client's own
    // owner, driven by the tool's `convert` verb. The platform credential owner decides
    // whether custody requires a separate authorization step.
    let (code, converted_report) = run_tool(&[
        "convert",
        "--data-root",
        source.to_str().expect("utf-8 source"),
        "--writers-stopped",
    ]);
    let requires_authorization = requires_credential_authorization();
    assert_eq!(code, if requires_authorization { 1 } else { 0 });
    assert_eq!(
        converted_report["status"],
        if requires_authorization {
            "pendingAuthorization"
        } else {
            "converted"
        }
    );
    assert_eq!(
        converted_report["stillOwed"]
            .as_array()
            .expect("stillOwed")
            .iter()
            .any(|domain| domain == CREDENTIAL_DOMAIN),
        requires_authorization,
        "the report follows the credential owner's platform verdict: {converted_report}"
    );

    let revision_digest = arrange_current_owner_content(&source, &work);

    // The client CLI runs with an isolated selected home; the roots it reads and writes are
    // named explicitly, exactly as an operator recovering a home would name them.
    let home = fixture.join("home");
    let selected = fixture.join("selected");
    fs::create_dir_all(&home).expect("fixture home");
    fs::create_dir_all(&selected).expect("selected home");
    save_data_home_locator(&home, &selected);

    // Direction one: the CLI exports, the tool restores.
    let cli_archive = fixture.join("archives/cli.zip");
    fs::create_dir_all(cli_archive.parent().expect("archive directory"))
        .expect("archive directory");
    let (code, exported) = run_client_cli(
        &home,
        &[
            "backup",
            "export",
            cli_archive.to_str().expect("utf-8 archive"),
            "--data-root",
            source.to_str().expect("utf-8 source"),
            "--writers-stopped",
        ],
    );
    assert_eq!(code, 0, "the CLI export must succeed: {exported}");
    assert_eq!(exported["coverage"], "limited");
    assert_eq!(limitation_domains(&exported), vec![CREDENTIAL_DOMAIN]);

    let tool_target = fixture.join("restored/tool-from-cli");
    let (code, imported) = run_tool(&[
        "import",
        "--archive",
        cli_archive.to_str().expect("utf-8 archive"),
        "--target-root",
        tool_target.to_str().expect("utf-8 target"),
    ]);
    assert_eq!(code, 0, "the tool must restore the CLI archive: {imported}");
    assert_eq!(imported["status"], "imported");
    assert_eq!(imported["coverage"], "limited");
    assert_eq!(limitation_domains(&imported), vec![CREDENTIAL_DOMAIN]);
    assert_eq!(imported["relocated"], true);
    assert!(
        imported["verifiedWorkflowRevisions"]
            .as_array()
            .expect("verifiedWorkflowRevisions")
            .iter()
            .any(|digest| digest == revision_digest.as_str()),
        "the workflow owner verified the restored revision: {imported}"
    );
    require_owner_readback(
        &tool_target,
        &tool_target.join("snapshots"),
        &revision_digest,
    );

    // Direction two: the tool exports, the CLI restores.
    let tool_archive = fixture.join("archives/tool.tar.gz");
    let (code, exported) = run_tool(&[
        "export",
        "--data-root",
        source.to_str().expect("utf-8 source"),
        "--archive",
        tool_archive.to_str().expect("utf-8 archive"),
        "--writers-stopped",
    ]);
    assert_eq!(code, 0, "the tool export must succeed: {exported}");
    assert_eq!(exported["status"], "exported");
    assert_eq!(exported["coverage"], "limited");
    assert_eq!(limitation_domains(&exported), vec![CREDENTIAL_DOMAIN]);

    let cli_target = fixture.join("restored/cli-from-tool");
    let (code, imported) = run_client_cli(
        &home,
        &[
            "backup",
            "import",
            tool_archive.to_str().expect("utf-8 archive"),
            "--target-root",
            cli_target.to_str().expect("utf-8 target"),
        ],
    );
    assert_eq!(code, 0, "the CLI must restore the tool archive: {imported}");
    require_owner_readback(&cli_target, &cli_target.join("snapshots"), &revision_digest);

    // The two containers really carried the same logical payload: both directions restore
    // the same released owner content.
    for root in [&tool_target, &cli_target] {
        assert!(
            owner_readback_accepts(root),
            "{} passes the oracle",
            root.display()
        );
    }
}

/// A marker-only root cannot satisfy the oracle, on either cross-container direction.
#[test]
fn a_marker_only_application_root_is_refused_by_the_oracle() {
    assert_candidate_identity();
    let fixture = TestRoot::new("marker-only");
    let source = fixture.join("marker-only-root");
    marker_only_root(&source);

    let home = fixture.join("home");
    let selected = fixture.join("selected");
    fs::create_dir_all(&home).expect("fixture home");
    fs::create_dir_all(&selected).expect("selected home");
    save_data_home_locator(&home, &selected);

    // The CLI exports the marker-only root; the tool restores it.
    let cli_archive = fixture.join("archives/marker-only.zip");
    fs::create_dir_all(cli_archive.parent().expect("archive directory"))
        .expect("archive directory");
    let (code, exported) = run_client_cli(
        &home,
        &[
            "backup",
            "export",
            cli_archive.to_str().expect("utf-8 archive"),
            "--data-root",
            source.to_str().expect("utf-8 source"),
            "--writers-stopped",
        ],
    );
    assert_eq!(code, 0, "{exported}");
    assert_eq!(exported["coverage"], "limited");
    assert_eq!(
        limitation_domains(&exported),
        vec![CREDENTIAL_DOMAIN],
        "pending custody stays visible even for a marker-only root"
    );

    let tool_target = fixture.join("restored/tool-from-marker-only");
    let (code, imported) = run_tool(&[
        "import",
        "--archive",
        cli_archive.to_str().expect("utf-8 archive"),
        "--target-root",
        tool_target.to_str().expect("utf-8 target"),
    ]);
    assert_eq!(code, 0, "the archive itself is well-formed: {imported}");
    assert_eq!(imported["coverage"], "limited");
    assert_eq!(limitation_domains(&imported), vec![CREDENTIAL_DOMAIN]);
    assert!(
        !owner_readback_accepts(&tool_target),
        "markers alone must not satisfy the owner readback oracle"
    );

    // Direction two, to prove the refusal is not an artifact of one container path.
    let tool_archive = fixture.join("archives/marker-only.tar.gz");
    let (code, exported) = run_tool(&[
        "export",
        "--data-root",
        source.to_str().expect("utf-8 source"),
        "--archive",
        tool_archive.to_str().expect("utf-8 archive"),
        "--writers-stopped",
    ]);
    assert_eq!(code, 0, "{exported}");
    assert_eq!(limitation_domains(&exported), vec![CREDENTIAL_DOMAIN]);

    let cli_target = fixture.join("restored/cli-from-marker-only");
    let (code, imported) = run_client_cli(
        &home,
        &[
            "backup",
            "import",
            tool_archive.to_str().expect("utf-8 archive"),
            "--target-root",
            cli_target.to_str().expect("utf-8 target"),
        ],
    );
    assert_eq!(code, 0, "{imported}");
    assert!(
        !owner_readback_accepts(&cli_target),
        "markers alone must not satisfy the owner readback oracle"
    );
}

/// The tool refuses to publish into an occupied destination and names the owner's own code.
#[test]
fn the_tool_import_refuses_an_occupied_destination() {
    let fixture = TestRoot::new("occupied");
    let source = fixture.join("source");
    marker_only_root(&source);
    let archive = fixture.join("archives/backup.zip");
    fs::create_dir_all(archive.parent().expect("archive directory")).expect("archive directory");
    let (code, exported) = run_tool(&[
        "export",
        "--data-root",
        source.to_str().expect("utf-8 source"),
        "--archive",
        archive.to_str().expect("utf-8 archive"),
        "--writers-stopped",
    ]);
    assert_eq!(code, 0, "{exported}");

    let occupied = fixture.join("occupied");
    fs::create_dir_all(&occupied).expect("occupied destination");
    write_file(&occupied, "keep.txt", b"existing content");
    let (code, report) = run_tool(&[
        "import",
        "--archive",
        archive.to_str().expect("utf-8 archive"),
        "--target-root",
        occupied.to_str().expect("utf-8 target"),
    ]);
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["error"], "archive_target_not_empty");
    assert_eq!(
        fs::read(occupied.join("keep.txt")).expect("existing entry survives"),
        b"existing content"
    );
}

/// Every verb returns exactly one JSON report with a typed exit status.
#[test]
fn every_verb_returns_one_json_report() {
    let fixture = TestRoot::new("reports");
    let source = fixture.join("source");
    seed_released_root(&source);
    let archive = fixture.join("archives/backup.zip");
    fs::create_dir_all(archive.parent().expect("archive directory")).expect("archive directory");

    let (code, report) = run_tool(&["inspect", "--data-root", source.to_str().expect("source")]);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "inspected");
    assert_eq!(report["frontierId"], "licoup-state-0.3.0");

    let (code, report) = run_tool(&["plan", "--data-root", source.to_str().expect("source")]);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "planned");

    let (code, report) = run_tool(&[
        "resume",
        "--data-root",
        source.to_str().expect("source"),
        "--writers-stopped",
    ]);
    assert_eq!(
        code, 1,
        "nothing was interrupted here, and the released root still owes a domain: {report}"
    );
    assert_eq!(report["status"], "noOp");
    assert!(
        report["stillOwed"]
            .as_array()
            .expect("stillOwed")
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the owed domain is named: {report}"
    );

    let (code, report) = run_tool(&[
        "export",
        "--data-root",
        source.to_str().expect("source"),
        "--archive",
        archive.to_str().expect("archive"),
        "--writers-stopped",
    ]);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "exported");

    let target = fixture.join("target");
    let (code, report) = run_tool(&[
        "import",
        "--archive",
        archive.to_str().expect("archive"),
        "--target-root",
        target.to_str().expect("target"),
    ]);
    assert_eq!(code, 0);
    assert_eq!(report["status"], "imported");

    let (code, report) = run_tool(&[
        "convert",
        "--data-root",
        source.to_str().expect("source"),
        "--writers-stopped",
    ]);
    let requires_authorization = requires_credential_authorization();
    assert_eq!(code, if requires_authorization { 1 } else { 0 });
    assert_eq!(
        report["status"],
        if requires_authorization {
            "pendingAuthorization"
        } else {
            "converted"
        }
    );
    assert!(report["domains"].is_array());
}
