//! Explicit plan import: idempotent replay, stale refusal, retention and scope.
//!
//! Every case here is synthetic: the authorized roots are temporary
//! directories, the authority references name no real deployment grant, and the
//! documents are written by the case itself. What is proved is the durable
//! behaviour of one import over the real store: repeated imports create no
//! duplicates, a stale apply conflicts instead of overwriting, work a source
//! omits is retained, and an unrelated project is untouched.

use licoup_project::{
    ArtifactReference, AuthorityReference, IMPORT_PLAN_MISMATCH, IMPORT_PROJECT_UNAUTHORIZED,
    IMPORT_STALE_APPLY, PLAN_DOCUMENT_SCHEMA, PROJECT_IMPORT_SOURCE_COLUMNS,
    PROJECT_PLAN_IMPORT_COLUMNS, PROJECT_STORE_SCHEMA_VERSION, PlanAdmission, PlanDocument,
    ProjectAuthorityDirectory, ProjectId, ProjectIdentitySource, ProjectIdentityStore,
    ProjectRegistration, SourceId, WorkItemId,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The authority owner a test composes: it admits exactly what it was given.
struct GrantedAuthorities {
    granted: Vec<AuthorityReference>,
}

impl ProjectAuthorityDirectory for GrantedAuthorities {
    fn admits(&self, reference: &AuthorityReference) -> bool {
        self.granted.contains(reference)
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
            "licoup-project-import-{label}-{}-{nanos}-{unique}",
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

/// Register one synthetic project carrying `plan_id` as its plan identity.
fn register(store: &ProjectIdentityStore, project_id: &str, plan_id: &str, root: &Path) {
    store
        .register(
            &GrantedAuthorities {
                granted: vec![owner_authority()],
            },
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

fn project(value: &str) -> ProjectId {
    ProjectId::declare(value).expect("a bounded project identity")
}

fn item(value: &str) -> WorkItemId {
    WorkItemId::declare(value).expect("a bounded work-item identity")
}

fn source(value: &str) -> SourceId {
    SourceId::declare(value).expect("a bounded source identity")
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

/// One canonical document over the given work items.
fn document(project_id: &str, plan_id: &str, source_id: &str, work_items: Vec<Value>) -> Value {
    json!({
        "schema": PLAN_DOCUMENT_SCHEMA,
        "projectId": project_id,
        "planId": plan_id,
        "source": {
            "sourceId": source_id,
            "sourceKind": "markdown",
            "locator": format!("docs/{source_id}.md"),
        },
        "workItems": work_items,
    })
}

fn admit(value: Value) -> PlanAdmission {
    PlanDocument::from_value(value)
        .expect("the synthetic document is canonical")
        .admit()
        .expect("the synthetic document admits")
}

/// One registered project, its store, and one admitting document over it.
fn fixture(label: &str) -> (TempRoot, ProjectIdentityStore, PlanAdmission) {
    let root = TempRoot::new(label);
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorized = root.authorized_root(label);
    register(&store, "project:alpha", "plan:alpha", &authorized);
    let document = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![work_item("work:read"), work_item("work:write")],
    ));
    (root, store, document)
}

fn row_count(store: &ProjectIdentityStore, table: &str) -> i64 {
    Connection::open(store.database_path())
        .expect("the database opens")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("the synthetic count is readable")
}

fn table_columns(store: &ProjectIdentityStore, table: &str) -> Vec<String> {
    let connection = Connection::open(store.database_path()).expect("the database opens");
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("the schema is readable");
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("the schema rows are readable")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("the schema rows decode")
}

#[test]
fn one_import_creates_a_source_owned_slice_with_its_correspondence() {
    let (_root, store, document) = fixture("create");
    let outcome = store
        .apply_import(&document, 0)
        .expect("the first import applies");
    assert!(outcome.applied);
    assert_eq!(outcome.change.revision, 1);
    assert!(!outcome.change.replayed);
    assert_eq!(
        outcome.change.added,
        vec![item("work:read"), item("work:write")]
    );
    assert!(outcome.change.unchanged.is_empty());
    assert!(outcome.change.retained.is_empty());
    assert_eq!(outcome.change.mapping.len(), 2);
    assert_eq!(
        outcome.change.mapping[1].source_anchor,
        "heading:work:write"
    );
    assert_eq!(outcome.change.source_id.as_str(), "source:roadmap");

    let slice = store
        .import_slice(&project("project:alpha"), &source("source:roadmap"))
        .expect("the slice reads")
        .expect("the slice exists");
    assert_eq!(slice.revision, 1);
    assert_eq!(slice.plan_id.as_str(), "plan:alpha");
    assert_eq!(slice.source_locator.as_str(), "docs/source:roadmap.md");
    assert_eq!(
        slice.work_item_ids,
        vec![item("work:read"), item("work:write")]
    );
    assert_eq!(slice.digest, document.document.digest());
}

#[test]
fn repeating_an_identical_import_changes_nothing() {
    let (_root, store, document) = fixture("replay");
    store
        .apply_import(&document, 0)
        .expect("the import applies");
    let rows_before = row_count(&store, "project_plan_imports");

    let replay = store
        .apply_import(&document, 1)
        .expect("the replay is admitted");
    assert!(!replay.applied, "an identical document is one effect");
    assert!(replay.change.replayed);
    assert_eq!(
        replay.change.revision, 1,
        "a replay does not consume a revision"
    );
    assert_eq!(row_count(&store, "project_plan_imports"), rows_before);
    assert_eq!(
        store.dependencies(&project("project:alpha")).unwrap().len(),
        0
    );
}

#[test]
fn an_apply_that_expects_another_state_conflicts_and_overwrites_nothing() {
    let (_root, store, first) = fixture("stale");
    store
        .apply_import(&first, 0)
        .expect("the first import applies");

    // A caller that previewed nothing (revision 0) must not overwrite what a
    // concurrent import already admitted.
    let stale = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![work_item("work:read")],
    ));
    let failure = store
        .apply_import(&stale, 0)
        .expect_err("a stale apply is refused");
    assert_eq!(failure.code(), IMPORT_STALE_APPLY);
    assert_eq!(failure.stage(), "project/import");
    assert!(
        failure
            .detail()
            .is_some_and(|detail| detail.contains("revision 1 is current")),
        "{failure:?}"
    );

    let slice = store
        .import_slice(&project("project:alpha"), &source("source:roadmap"))
        .expect("the slice reads")
        .expect("the slice exists");
    assert_eq!(slice.revision, 1);
    assert_eq!(
        slice.work_item_ids.len(),
        2,
        "the refused apply changed nothing"
    );

    // The caller that re-previews at revision 1 can then apply the change.
    let applied = store
        .apply_import(&stale, 1)
        .expect("the current revision applies");
    assert!(applied.applied);
    assert_eq!(applied.change.revision, 2);
    assert_eq!(applied.change.retained, vec![item("work:write")]);
}

#[test]
fn an_omitted_work_item_is_retained_and_never_deleted_or_cancelled() {
    let (_root, store, first) = fixture("retain");
    store
        .apply_import(&first, 0)
        .expect("the first import applies");

    let shortened = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![work_item("work:read")],
    ));
    let outcome = store
        .apply_import(&shortened, 1)
        .expect("the shortened document applies");
    assert_eq!(outcome.change.added, Vec::<WorkItemId>::new());
    assert_eq!(outcome.change.unchanged, vec![item("work:read")]);
    assert_eq!(
        outcome.change.retained,
        vec![item("work:write")],
        "an omission is reported, not performed"
    );

    let slice = store
        .import_slice(&project("project:alpha"), &source("source:roadmap"))
        .unwrap()
        .unwrap();
    assert_eq!(
        slice.work_item_ids,
        vec![item("work:read"), item("work:write")],
        "the omitted work item stays admitted"
    );
}

#[test]
fn another_project_or_another_source_owns_its_own_slice() {
    let root = TempRoot::new("isolation");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    register(
        &store,
        "project:alpha",
        "plan:alpha",
        &root.authorized_root("alpha"),
    );
    register(
        &store,
        "project:beta",
        "plan:beta",
        &root.authorized_root("beta"),
    );
    let alpha = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![work_item("work:read")],
    ));
    let beta = admit(document(
        "project:beta",
        "plan:beta",
        "source:roadmap",
        vec![work_item("work:beta")],
    ));
    let second_source = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:notes",
        vec![work_item("work:note")],
    ));

    store.apply_import(&alpha, 0).expect("alpha applies");
    store.apply_import(&beta, 0).expect("beta applies");
    store
        .apply_import(&second_source, 0)
        .expect("the second source applies");

    let beta_slice = store
        .import_slice(&project("project:beta"), &source("source:roadmap"))
        .unwrap()
        .unwrap();
    assert_eq!(beta_slice.work_item_ids, vec![item("work:beta")]);
    assert_eq!(beta_slice.plan_id.as_str(), "plan:beta");

    let notes = store
        .import_slice(&project("project:alpha"), &source("source:notes"))
        .unwrap()
        .unwrap();
    assert_eq!(notes.work_item_ids, vec![item("work:note")]);
    assert_eq!(notes.revision, 1);
    let roadmap = store
        .import_slice(&project("project:alpha"), &source("source:roadmap"))
        .unwrap()
        .unwrap();
    assert_eq!(
        roadmap.work_item_ids,
        vec![item("work:read")],
        "a second source does not rewrite the first"
    );
    assert_eq!(row_count(&store, "project_import_sources"), 3);
}

#[test]
fn an_unregistered_project_or_a_foreign_plan_is_refused_before_any_effect() {
    let root = TempRoot::new("refusals");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    register(
        &store,
        "project:alpha",
        "plan:alpha",
        &root.authorized_root("alpha"),
    );

    let unknown = admit(document(
        "project:absent",
        "plan:absent",
        "source:roadmap",
        vec![work_item("work:read")],
    ));
    let failure = store
        .apply_import(&unknown, 0)
        .expect_err("an unregistered project is refused");
    assert_eq!(failure.code(), IMPORT_PROJECT_UNAUTHORIZED);

    // A plan identity the registration does not carry is a different plan, not
    // a second plan of the same project.
    let foreign = admit(document(
        "project:alpha",
        "plan:somewhere-else",
        "source:roadmap",
        vec![work_item("work:read")],
    ));
    let failure = store
        .apply_import(&foreign, 0)
        .expect_err("a foreign plan is refused");
    assert_eq!(failure.code(), IMPORT_PLAN_MISMATCH);

    assert_eq!(row_count(&store, "project_plan_imports"), 0);
    assert_eq!(row_count(&store, "project_import_sources"), 0);
    assert!(
        store
            .import_slice(&project("project:alpha"), &source("source:roadmap"))
            .unwrap()
            .is_none(),
        "a refused import leaves no slice behind"
    );
}

#[test]
fn declared_inputs_are_admitted_once_through_the_dependency_owner() {
    let (_root, store, _document) = fixture("inputs");
    let mut consumer = work_item("work:write");
    consumer["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:read", "path": "build/out.json"}
    ]);
    let document = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![consumer, work_item("work:read")],
    ));

    let outcome = store
        .apply_import(&document, 0)
        .expect("the document with an input applies");
    assert_eq!(outcome.change.input_count, 1);
    let declared = store.dependencies(&project("project:alpha")).unwrap();
    assert_eq!(declared.len(), 1);
    assert_eq!(
        declared[0].dependency.artifact,
        ArtifactReference::local(item("work:read"), "build/out.json").unwrap()
    );

    // A replay admits nothing a second time: the edge keeps its original order.
    let sequence = declared[0].dependency_sequence;
    store
        .apply_import(&document, 1)
        .expect("the replay is admitted");
    let declared = store.dependencies(&project("project:alpha")).unwrap();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].dependency_sequence, sequence);
}

#[test]
fn an_input_outside_the_authorized_root_refuses_the_whole_import() {
    let (_root, store, _document) = fixture("escape");
    let mut consumer = work_item("work:write");
    consumer["inputs"] = json!([
        {"kind": "local", "producerWorkItemId": "work:read", "path": "../outside/out.json"}
    ]);
    let document = admit(document(
        "project:alpha",
        "plan:alpha",
        "source:roadmap",
        vec![consumer, work_item("work:read")],
    ));

    let failure = store
        .apply_import(&document, 0)
        .expect_err("an escaping input is refused");
    assert_eq!(
        failure.code(),
        "project_artifact_reference_escapes_authorized_root"
    );
    assert_eq!(
        row_count(&store, "project_plan_imports"),
        0,
        "the refused input leaves neither the edge nor the slice"
    );
    assert_eq!(row_count(&store, "project_import_sources"), 0);
}

#[test]
fn a_preview_reports_the_change_the_same_input_would_apply() {
    let (_root, store, document) = fixture("preview");
    let preview = store
        .preview_import(&document)
        .expect("the preview computes");
    assert_eq!(preview.revision, 0);
    assert_eq!(preview.added.len(), 2);
    assert_eq!(preview.digest, document.document.digest());
    assert_eq!(row_count(&store, "project_plan_imports"), 0);

    let outcome = store
        .apply_import(&document, preview.revision)
        .expect("the previewed import applies");
    let mut applied = outcome.change;
    applied.revision = 0;
    assert_eq!(applied, preview, "a preview is the apply that has not run");
}

#[test]
fn the_import_rows_hold_no_field_that_could_carry_a_run_or_an_acceptance() {
    let (_root, store, _document) = fixture("schema");
    let version: String = Connection::open(store.database_path())
        .expect("the database opens")
        .query_row(
            "SELECT value FROM project_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .expect("the version is recorded");
    assert_eq!(version, PROJECT_STORE_SCHEMA_VERSION);

    assert_eq!(
        table_columns(&store, "project_plan_imports"),
        PROJECT_PLAN_IMPORT_COLUMNS
    );
    assert_eq!(
        table_columns(&store, "project_import_sources"),
        PROJECT_IMPORT_SOURCE_COLUMNS
    );

    // A slice stores what the source declared and nothing about what happened.
    for forbidden in [
        "status",
        "state",
        "started_at",
        "completed_at",
        "accepted",
        "executed",
        "observed",
    ] {
        assert!(
            !PROJECT_PLAN_IMPORT_COLUMNS.contains(&forbidden)
                && !PROJECT_IMPORT_SOURCE_COLUMNS.contains(&forbidden),
            "{forbidden} must not be a stored import field"
        );
    }
}

#[test]
fn a_store_this_owner_wrote_reopens_in_the_next_process() {
    let root = TempRoot::new("reopen");
    let admitted = {
        let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
        register(
            &store,
            "project:alpha",
            "plan:alpha",
            &root.authorized_root("alpha"),
        );
        let admitted = admit(document(
            "project:alpha",
            "plan:alpha",
            "source:roadmap",
            vec![work_item("work:read")],
        ));
        store
            .apply_import(&admitted, 0)
            .expect("the import applies");
        admitted
    };

    // The next process validates the recorded shape before it reads anything, so
    // a recorded version that drifted from the owner's own constant — the defect
    // this case exists for — would be refused here rather than half-opened.
    let reopened = ProjectIdentityStore::open(root.path()).expect("a written store reopens");
    let slice = reopened
        .import_slice(&project("project:alpha"), &source("source:roadmap"))
        .expect("the slice reads")
        .expect("the slice exists");
    assert_eq!(slice.revision, 1);
    assert_eq!(slice.work_item_ids, vec![item("work:read")]);
    assert_eq!(slice.digest, admitted.document.digest());
    assert!(
        reopened
            .read(&project("project:alpha"))
            .expect("the registration reads")
            .is_some(),
        "the registration the first process wrote survives the reopen"
    );
}
