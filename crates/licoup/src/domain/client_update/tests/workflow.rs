use super::super::apply::{ADMISSION_BLOCKED, ADMISSION_CLOSED, Replacement, admit_replacement};
use super::support::*;
use crate::domain::work_admission::{AdmissionDecision, MaintenanceOperation, WorkAdmission};
use licoup_conversation::ConversationStore;

#[test]
fn client_update_check_download_verify_and_plan_share_one_digest_bound_receipt() {
    let fixture = UpdateFixture::new();
    let check_params = fixture.params(fixture.manifest());
    let checked = check(&check_params).unwrap();
    let mut params = check_params;
    params.as_object_mut().unwrap().remove("targetReleaseTrack");
    let downloaded = download(&params).unwrap();
    let verified = verify(&params).unwrap();
    let planned = apply(&params).unwrap();
    let receipt_id = checked["artifactReceipt"]["receiptId"].clone();
    assert_eq!(
        checked["artifactReceipt"]["schemaVersion"],
        CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA
    );
    assert_eq!(downloaded["artifactReceipt"]["receiptId"], receipt_id);
    assert_eq!(verified["artifactReceipt"]["receiptId"], receipt_id);
    assert_eq!(planned["artifactReceipt"]["receiptId"], receipt_id);
    assert_eq!(planned["phase"], "applyPlanned");
    assert_eq!(planned["executed"], false);
    for value in [&checked, &downloaded, &verified, &planned] {
        assert_eq!(value["runningVersion"], checked["runningVersion"]);
        assert_eq!(value["runningReleaseTrack"], checked["runningReleaseTrack"]);
        assert_eq!(value["targetReleaseTrack"], checked["targetReleaseTrack"]);
        assert_redacted(value, &fixture.root);
    }
}

#[test]
fn client_update_status_and_dispatch_do_not_project_local_file_names_or_paths() {
    let fixture = UpdateFixture::new();
    fs::create_dir_all(&fixture.staging).unwrap();
    fs::write(fixture.staging.join("private-local-name.bin"), b"private").unwrap();
    let result = status(&json!({"stagingRoot": fixture.staging})).unwrap();
    assert_eq!(result["stagedArtifactCount"], 1);
    assert!(!result.to_string().contains("private-local-name.bin"));
    assert_redacted(&result, &fixture.root);
    assert!(
        dispatch(&["update".into(), "unknown".into()], &json!({}))
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
}

#[test]
fn client_update_later_phases_require_the_exact_signed_check_receipt() {
    let fixture = UpdateFixture::new();
    let mut unchecked = fixture.params(fixture.manifest());
    unchecked
        .as_object_mut()
        .unwrap()
        .remove("targetReleaseTrack");
    assert!(
        download(&unchecked)
            .unwrap_err()
            .to_string()
            .contains("check receipt is required")
    );

    let checked = fixture.checked_params(fixture.manifest());
    let replacement = fixture.sign_manifest(
        fixture.unsigned_manifest(json!([release("998.0.0", fixture.artifact(TARGET_ID),)])),
    );
    let mut substituted = checked;
    substituted["manifestJson"] = replacement;
    assert!(
        download(&substituted)
            .unwrap_err()
            .to_string()
            .contains("does not match the signed check receipt")
    );
}

/// The close-admission barrier's record inside one data root.
const BARRIER_RECORD: &str = "client-state/maintenance-admission.json";

/// The update handoff a live apply writes before it hands ownership over.
const HANDOFF_RECORD: &str = "client-state/migrations/update-handoff.json";

/// One synthetic data root the maintenance decision is taken about.
fn data_root(fixture: &UpdateFixture) -> PathBuf {
    let root = fixture.root.join("data");
    licoup_foundation::platform::file_security::ensure_private_dir(&root).unwrap();
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))
        .unwrap();
    root
}

/// Check, download and stage the fixture release, then describe one live apply
/// against `root`.
///
/// The install root is a filesystem root, which no platform accepts as a
/// replacement target, so an apply that gets past admission aborts in the plan
/// builder before the first write. That keeps the abort path deterministic on
/// every platform instead of depending on one platform's installer strategy.
fn live_apply_params(fixture: &UpdateFixture, root: &Path) -> Value {
    let params = fixture.checked_params(fixture.manifest());
    download(&params).unwrap();
    let mut params = params;
    params["execute"] = json!(true);
    params["dataRoot"] = json!(root);
    params["installRoot"] = json!("/");
    params["guiPid"] = json!("4242");
    params["waitForScript"] = json!(true);
    params
}

/// A host that still owns unfinished local work refuses a live apply, and the
/// refusal is the vocabulary the client's update projection renders rather than
/// a transport failure. Nothing is taken and nothing is staged for replacement.
#[test]
fn client_update_live_apply_is_refused_while_local_work_is_unfinished() {
    let fixture = UpdateFixture::new();
    let root = data_root(&fixture);
    let store = ConversationStore::open(&root).unwrap();
    store
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .unwrap();
    drop(store);

    let params = live_apply_params(&fixture, &root);
    let refused = apply(&params).unwrap();

    assert_eq!(refused["ok"], false);
    assert_eq!(refused["phase"], "blocked");
    assert_eq!(refused["errorCode"], ADMISSION_BLOCKED);
    assert_eq!(refused["reasonCode"], ADMISSION_BLOCKED);
    assert_eq!(refused["executed"], false);
    assert_eq!(refused["admission"]["decision"], "blocked");
    assert!(
        !refused["admission"]["blockers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the refusal names the work that blocks it: {refused}"
    );
    assert_redacted(&refused, &fixture.root);
    assert!(
        !root.join(BARRIER_RECORD).exists(),
        "a refused switch takes no barrier"
    );
    assert!(
        !root.join(HANDOFF_RECORD).exists(),
        "a refused switch stages no replacement"
    );
}

/// A host whose admission is already closed refuses a second switch with its own
/// code, and the barrier the first switch holds is left alone.
#[test]
fn client_update_live_apply_is_refused_while_another_switch_holds_admission() {
    let fixture = UpdateFixture::new();
    let root = data_root(&fixture);
    let held = WorkAdmission::open(&root)
        .begin_maintenance(MaintenanceOperation::ClientReplacement)
        .unwrap();
    assert!(held.barrier().is_some(), "the fixture holds the barrier");

    let params = live_apply_params(&fixture, &root);
    let refused = apply(&params).unwrap();

    assert_eq!(refused["phase"], "blocked");
    assert_eq!(refused["errorCode"], ADMISSION_CLOSED);
    assert_eq!(refused["admission"]["decision"], "closed");
    assert_eq!(refused["executed"], false);
    assert!(
        root.join(BARRIER_RECORD).exists(),
        "the refusal never releases the barrier it did not take"
    );
    assert!(
        WorkAdmission::open(&root).barrier().unwrap().is_some(),
        "the first switch still holds admission"
    );
}

/// An idle host admits the switch and the barrier is durable before anything is
/// replaced, so a second switch cannot begin while the first one holds it.
#[test]
fn client_update_live_apply_takes_the_barrier_when_the_host_is_idle() {
    let fixture = UpdateFixture::new();
    let root = data_root(&fixture);

    assert!(matches!(admit_replacement(&root), Replacement::Held { .. }));
    assert!(
        root.join(BARRIER_RECORD).exists(),
        "the admitted switch holds a durable barrier"
    );
    match admit_replacement(&root) {
        Replacement::Refused { code, answer } => {
            assert_eq!(code, ADMISSION_CLOSED);
            assert_eq!(answer["decision"], "closed");
        }
        Replacement::Held { .. } => panic!("a held barrier must refuse a second switch"),
    }

    WorkAdmission::open(&root).release_admission().unwrap();
    assert!(!root.join(BARRIER_RECORD).exists());
}

/// A switch that aborts before ownership passes to the candidate releases the
/// barrier it took: a failed apply must not leave the host closed to every later
/// maintenance operation.
#[test]
fn client_update_live_apply_that_aborts_releases_the_barrier_it_took() {
    let fixture = UpdateFixture::new();
    let root = data_root(&fixture);

    let params = live_apply_params(&fixture, &root);
    let error = apply(&params).unwrap_err().to_string();

    assert!(
        !error.contains(ADMISSION_BLOCKED) && !error.contains(ADMISSION_CLOSED),
        "the host was idle, so admission is not what failed: {error}"
    );
    assert!(
        !root.join(BARRIER_RECORD).exists(),
        "an aborted switch must not leave admission closed"
    );
    assert!(
        !root.join(HANDOFF_RECORD).exists(),
        "an aborted switch hands no ownership over"
    );
    assert_eq!(
        WorkAdmission::open(&root).admission().unwrap().decision,
        AdmissionDecision::Idle
    );
}
