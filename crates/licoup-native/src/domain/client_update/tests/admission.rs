//! The idle gate behind the update status projection.
//!
//! The status verb publishes the host's own maintenance decision
//! (`domain::work_admission`) and derives its phase from that answer. These
//! tests build the data root through the owning store's own writes, so the gate
//! is exercised against the shapes production produces.

use super::support::*;
use licoup_conversation::{ConversationStore, DispatchState};
use licoup_foundation::platform::paths::set_portable_data_dir_override;

/// One host data root, installed as this thread's portable data home for as
/// long as the guard lives. The override is thread-local, so the decision the
/// status verb reads comes from this fixture and never from ambient state.
struct HostDataRoot {
    root: PathBuf,
    previous: Option<PathBuf>,
}

impl HostDataRoot {
    fn new(root: PathBuf) -> Self {
        fs::create_dir_all(&root).unwrap();
        licoup_foundation::platform::file_security::ensure_private_dir(&root).unwrap();
        licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))
            .unwrap();
        let previous = set_portable_data_dir_override(Some(root.clone()));
        Self { root, previous }
    }

    /// One admitted dispatch this host has not settled.
    fn admit_unfinished_dispatch(&self) -> (ConversationStore, String, String) {
        let store = ConversationStore::open(&self.root).unwrap();
        let scope = store
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
        (store, scope.dispatch_id, scope.event_id)
    }
}

impl Drop for HostDataRoot {
    fn drop(&mut self) {
        set_portable_data_dir_override(self.previous.take());
    }
}

fn status_of(fixture: &UpdateFixture) -> Value {
    status(&json!({"stagingRoot": fixture.staging})).unwrap()
}

#[test]
fn client_update_status_reports_idle_only_while_the_host_owns_no_unfinished_work() {
    let fixture = UpdateFixture::new();
    let host = HostDataRoot::new(fixture.root.join("data"));

    let idle = status_of(&fixture);
    assert_eq!(idle["phase"], "idle");
    assert_eq!(idle["productionReady"], true);
    assert_eq!(idle["admission"]["decision"], "idle");
    assert_eq!(idle["admission"]["blockers"], json!([]));
    assert_redacted(&idle, &fixture.root);

    // One admitted dispatch this host has not settled: a maintenance switch
    // would replace installed state underneath it.
    let (store, dispatch_id, event_id) = host.admit_unfinished_dispatch();
    let blocked = status_of(&fixture);
    assert_ne!(
        blocked["phase"], "idle",
        "unfinished local work is never reported idle"
    );
    assert_eq!(blocked["phase"], "blocked");
    assert_eq!(blocked["productionReady"], false);
    assert_eq!(blocked["admission"]["decision"], "blocked");
    let blockers = blocked["admission"]["blockers"].as_array().unwrap();
    let dispatch = blockers
        .iter()
        .find(|blocker| blocker["identity"] == json!(dispatch_id))
        .expect("the admitted dispatch is reported as the blocker it is");
    assert_eq!(dispatch["owner"], "canonical-conversation");
    assert_eq!(dispatch["kind"], "conversation-dispatch");
    assert_eq!(dispatch["state"], "accepted");
    assert_redacted(&blocked, &fixture.root);

    // Settling that work reopens the gate: the answer tracks the work itself.
    store.finalize_event(&event_id).unwrap();
    store
        .update_dispatch(&dispatch_id, DispatchState::Running, None, None)
        .unwrap();
    store
        .update_dispatch(&dispatch_id, DispatchState::Completed, None, None)
        .unwrap();
    let settled = status_of(&fixture);
    assert_eq!(settled["phase"], "idle");
    assert_eq!(settled["productionReady"], true);
    assert_eq!(settled["admission"]["decision"], "idle");
}

#[test]
fn client_update_status_reports_a_held_barrier_as_locked() {
    let fixture = UpdateFixture::new();
    let _host = HostDataRoot::new(fixture.root.join("data"));

    // Another maintenance operation already holds the close-admission barrier.
    let barrier =
        match crate::domain::work_admission::WorkAdmission::open(fixture.root.join("data"))
            .begin_maintenance(
                crate::domain::work_admission::MaintenanceOperation::ClientReplacement,
            ) {
            Ok(crate::domain::work_admission::MaintenanceAdmission::Held(barrier)) => barrier,
            other => panic!("expected the barrier, got {other:?}"),
        };
    assert_eq!(barrier.state, "claimed");

    let closed = status_of(&fixture);
    assert_eq!(closed["phase"], "blocked");
    assert_eq!(closed["productionReady"], false);
    assert_eq!(closed["admission"]["decision"], "closed");
    assert_eq!(
        closed["admission"]["barrier"]["operation"],
        "client-replacement"
    );

    // Releasing the barrier reopens the gate for the next switch.
    crate::domain::work_admission::WorkAdmission::open(fixture.root.join("data"))
        .release_admission()
        .unwrap();
    let reopened = status_of(&fixture);
    assert_eq!(reopened["phase"], "idle");
    assert_eq!(reopened["productionReady"], true);
    assert_eq!(reopened["admission"]["decision"], "idle");
}
