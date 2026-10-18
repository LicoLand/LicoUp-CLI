//! Declared project work in the real scheduling and stop questions.
//!
//! Every case is synthetic and reads the durable rows the store holds: a
//! registered project, one imported plan slice, and the inputs its work items
//! declare. What is proved is that readiness follows each work item's own
//! declarations, that a stop scope names exactly the work that waits on the
//! selection, and that admitted responsibility survives any status until the
//! explicit change rules retire it.

use licoup_project::{
    ArtifactReference, AuthorityReference, PLAN_DOCUMENT_SCHEMA, PlanDocument,
    ProjectAuthorityDirectory, ProjectId, ProjectIdentitySource, ProjectIdentityStore,
    ProjectRegistration, SCHEDULE_PROJECT_UNAUTHORIZED, SCHEDULE_STAGE, WorkDependency, WorkItemId,
    WorkRef,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The authority owner a test composes: it admits exactly the owner reference.
struct GrantedAuthorities;

impl ProjectAuthorityDirectory for GrantedAuthorities {
    fn admits(&self, reference: &AuthorityReference) -> bool {
        reference == &owner_authority()
    }
}

/// One temporary data root, removed when the case ends.
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "licoup-project-schedule-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the temporary root is creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn authorized_root(&self, label: &str) -> PathBuf {
        let root = self.path.join("authorized").join(label);
        std::fs::create_dir_all(&root).expect("the synthetic authorized root is creatable");
        root
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn owner_authority() -> AuthorityReference {
    AuthorityReference::membership("membership:owner").expect("a bounded membership reference")
}

fn project(value: &str) -> ProjectId {
    ProjectId::declare(value).expect("a bounded project identity")
}

fn item(value: &str) -> WorkItemId {
    WorkItemId::declare(value).expect("a bounded work-item identity")
}

fn work_ref(project_id: &str, work_item_id: &str) -> WorkRef {
    WorkRef::new(project(project_id), item(work_item_id))
}

fn register(store: &ProjectIdentityStore, project_id: &str, plan_id: &str, root: &Path) {
    store
        .register(
            &GrantedAuthorities,
            &ProjectRegistration {
                identity: ProjectIdentitySource::Declared {
                    project_id: project_id.to_owned(),
                },
                display_name: format!("Synthetic project {project_id}"),
                authorized_root: root.to_string_lossy().into_owned(),
                authority: Some(owner_authority()),
                workspace_id: "workspace:shared".to_owned(),
                plan_id: plan_id.to_owned(),
            },
        )
        .expect("the synthetic project registers");
}

/// One declared work item that admits cleanly.
fn work_item(id: &str) -> Value {
    json!({
        "workItemId": id,
        "outcome": format!("Deliver {id}."),
        "acceptance": ["The declared outcome holds."],
        "inputs": [],
        "roles": [{"roleId": "role:maintainer", "scope": "work-item"}],
        "sourceAnchor": format!("heading:{id}"),
    })
}

/// Import one synthetic slice over the given work items.
///
/// The document declares the plan identity its project was registered under:
/// the store admits a slice only when its plan is the plan that registration
/// carries, and a plan identity is a caller declaration rather than something
/// derived from the project identity.
fn import(store: &ProjectIdentityStore, project_id: &str, plan_id: &str, work_items: Vec<Value>) {
    let document = json!({
        "schema": PLAN_DOCUMENT_SCHEMA,
        "projectId": project_id,
        "planId": plan_id,
        "source": {
            "sourceId": "source:roadmap",
            "sourceKind": "markdown",
            "locator": "docs/roadmap.md",
        },
        "workItems": work_items,
    });
    let admission = PlanDocument::from_value(document)
        .expect("the synthetic document is canonical")
        .admit()
        .expect("the synthetic document admits");
    store
        .apply_import(&admission, 0)
        .expect("the synthetic import applies");
}

/// Declare one local input: `consumer` waits for the result of `producer`.
fn declare_local_input(
    store: &ProjectIdentityStore,
    project_id: &str,
    consumer: &str,
    producer: &str,
    path: &str,
) {
    store
        .admit_dependency(&WorkDependency {
            project_id: project(project_id),
            work_item_id: item(consumer),
            artifact: ArtifactReference::local(item(producer), path).expect("a declared location"),
        })
        .expect("the synthetic input is declared");
}

/// A project whose plan is `A → C`, with an independent `B` beside it.
fn dependent_plan(label: &str) -> (TempRoot, ProjectIdentityStore) {
    let root = TempRoot::new(label);
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    register(
        &store,
        "project:alpha",
        "plan:alpha",
        &root.authorized_root("alpha"),
    );
    import(
        &store,
        "project:alpha",
        "plan:alpha",
        vec![
            work_item("work:a"),
            work_item("work:b"),
            work_item("work:c"),
        ],
    );
    declare_local_input(&store, "project:alpha", "work:c", "work:a", "build/a.json");
    (root, store)
}

#[test]
fn a_waiter_is_not_ready_until_its_own_producer_is_materialized() {
    let (_root, store) = dependent_plan("readiness");
    let readiness = store
        .work_readiness(&project("project:alpha"))
        .expect("the readiness reads");

    // `C` waits only on `A`. `B` is independent and gates nothing.
    assert!(readiness.is_ready(&item("work:a")));
    assert!(readiness.is_ready(&item("work:b")));
    assert!(!readiness.is_ready(&item("work:c")));
    assert_eq!(
        readiness.blocked_by(&item("work:c")),
        Some([work_ref("project:alpha", "work:a")].as_slice())
    );
}

#[test]
fn an_independent_branch_never_gates_a_ready_one() {
    let (root, store) = dependent_plan("independent");
    // `A`'s declared result now exists inside the authorized root.
    let declared = root.authorized_root("alpha").join("build");
    std::fs::create_dir_all(&declared).expect("the declared directory is creatable");
    std::fs::write(declared.join("a.json"), "{}\n").expect("the declared result is writable");

    let readiness = store
        .work_readiness(&project("project:alpha"))
        .expect("the readiness reads");
    assert!(
        readiness.is_ready(&item("work:c")),
        "C begins as soon as A's declared result exists, before independent B completes"
    );
    assert!(
        readiness.blocked.is_empty(),
        "no work item is blocked once its own producers are materialized"
    );
}

#[test]
fn a_stop_scope_releases_the_selection_and_its_declared_consumers_only() {
    let (_root, store) = dependent_plan("stop-scope");
    let scope = store
        .stop_scope(&project("project:alpha"), &item("work:a"))
        .expect("the scope reads");

    assert!(scope.releases(&work_ref("project:alpha", "work:a")));
    assert!(
        scope.releases(&work_ref("project:alpha", "work:c")),
        "the declared consumer of the selection is released with it"
    );
    assert!(
        !scope.releases(&work_ref("project:alpha", "work:b")),
        "an independent branch keeps its own course"
    );
    assert!(!scope.crosses_projects());
    assert_eq!(scope.projects, vec![project("project:alpha")]);
}

#[test]
fn a_stop_in_one_project_leaves_unrelated_projects_alone() {
    let root = TempRoot::new("cross-project");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    for (project_id, plan_id, label) in [
        ("project:alpha", "plan:alpha", "alpha"),
        ("project:beta", "plan:beta", "beta"),
    ] {
        register(&store, project_id, plan_id, &root.authorized_root(label));
    }
    import(
        &store,
        "project:alpha",
        "plan:alpha",
        vec![work_item("work:a"), work_item("work:b")],
    );
    import(
        &store,
        "project:beta",
        "plan:beta",
        vec![work_item("work:c"), work_item("work:independent")],
    );
    // Beta's `C` waits on alpha's `A`: the declared cross-project edge is what a
    // stop follows, and only that edge.
    store
        .admit_dependency(&WorkDependency {
            project_id: project("project:beta"),
            work_item_id: item("work:c"),
            artifact: ArtifactReference::cross_project(project("project:alpha"), item("work:a")),
        })
        .expect("the cross-project input is declared");

    let scope = store
        .stop_scope(&project("project:alpha"), &item("work:a"))
        .expect("the scope reads");
    assert!(scope.crosses_projects());
    assert!(scope.releases(&work_ref("project:beta", "work:c")));
    assert!(
        !scope.releases(&work_ref("project:beta", "work:independent")),
        "unrelated work in another project continues"
    );
    assert!(
        !scope.releases(&work_ref("project:alpha", "work:b")),
        "unrelated work in the same project continues"
    );

    let beta = store
        .outstanding_work(&project("project:beta"))
        .expect("beta's responsibility reads");
    assert_eq!(beta.admitted_work_items, 2);
}

#[test]
fn admitted_responsibility_survives_until_the_declaration_is_retired() {
    let (_root, store) = dependent_plan("outstanding");
    let outstanding = store
        .outstanding_work(&project("project:alpha"))
        .expect("the responsibility reads");
    assert_eq!(outstanding.admitted_work_items, 3);
    assert_eq!(outstanding.ready, 2);
    assert_eq!(outstanding.blocked, 1);
    assert_eq!(outstanding.declared_inputs, 1);
    assert!(
        !outstanding.settled,
        "admitted work is responsibility until the declaration is retired"
    );

    // Reading it again — a refresh, a view change, a fresh process — changes
    // nothing: the answer is the durable rows, not a status anyone can set.
    let again = store
        .outstanding_work(&project("project:alpha"))
        .expect("the responsibility reads again");
    assert_eq!(again, outstanding);
}

#[test]
fn a_project_with_no_admitted_work_and_no_declared_input_is_settled() {
    let root = TempRoot::new("settled");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    register(
        &store,
        "project:alpha",
        "plan:alpha",
        &root.authorized_root("alpha"),
    );
    let outstanding = store
        .outstanding_work(&project("project:alpha"))
        .expect("the responsibility reads");
    assert_eq!(outstanding.admitted_work_items, 0);
    assert_eq!(outstanding.declared_inputs, 0);
    assert!(outstanding.settled);
}

#[test]
fn a_scheduling_question_about_an_unregistered_project_is_refused() {
    let root = TempRoot::new("unauthorized");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let failure = store
        .work_readiness(&project("project:absent"))
        .expect_err("an unregistered project is refused");
    assert_eq!(failure.code(), SCHEDULE_PROJECT_UNAUTHORIZED);
    assert_eq!(failure.stage(), SCHEDULE_STAGE);
}
