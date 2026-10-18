//! Declared dependency and artifact inputs: admission rules, explicit
//! artifact states, the consumer set of a blocked producer, and durability.
//!
//! Every case here is synthetic: the authorized roots are temporary
//! directories, the authority references name no real deployment grant, and
//! nothing outside a declared root is read.

use licoup_project::{
    ArtifactReference, ArtifactState, AuthorityReference, DeclaredDependency,
    PROJECT_DEPENDENCY_COLUMNS, PROJECT_STORE_SCHEMA_VERSION, ProjectAuthorityDirectory,
    ProjectFailure, ProjectId, ProjectIdentitySource, ProjectIdentityStore, ProjectRegistration,
    WorkDependency, WorkItemId, WorkRef,
};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The authority owner a test composes: it admits exactly the references it was
/// given and nothing else.
struct GrantedAuthorities {
    granted: Vec<AuthorityReference>,
}

impl GrantedAuthorities {
    fn granting(references: impl IntoIterator<Item = AuthorityReference>) -> Self {
        Self {
            granted: references.into_iter().collect(),
        }
    }
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
            "licoup-project-deps-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the temporary root is creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// The real authorized root of one synthetic project. It is created because
    /// this case reads artifact materialization; naming a project never needs
    /// the directory to exist.
    fn authorized_root(&self, label: &str) -> PathBuf {
        let root = self.path.join("authorized").join(label);
        std::fs::create_dir_all(&root).expect("the synthetic authorized root is creatable");
        root
    }

    /// A sibling directory beside the authorized roots: the location an
    /// escaping reference would reach if it were followed.
    fn beside_authorized_roots(&self, label: &str) -> PathBuf {
        let outside = self.path.join("outside").join(label);
        std::fs::create_dir_all(&outside).expect("the synthetic outside root is creatable");
        outside
    }

    fn write(&self, at: &Path, relative: &str, contents: &str) -> PathBuf {
        let location = at.join(relative);
        if let Some(parent) = location.parent() {
            std::fs::create_dir_all(parent).expect("the synthetic parent is creatable");
        }
        std::fs::write(&location, contents).expect("the synthetic artifact is writable");
        location
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

fn authorities() -> GrantedAuthorities {
    GrantedAuthorities::granting([owner_authority()])
}

/// Register one synthetic project whose authorized root really exists.
fn register(store: &ProjectIdentityStore, project_id: &str, authorized_root: &Path) {
    store
        .register(
            &authorities(),
            &ProjectRegistration {
                identity: ProjectIdentitySource::Declared {
                    project_id: project_id.to_owned(),
                },
                display_name: format!("Synthetic project {project_id}"),
                authorized_root: authorized_root.to_string_lossy().into_owned(),
                authority: Some(owner_authority()),
                workspace_id: "workspace:shared".to_owned(),
                plan_id: format!("plan:{project_id}"),
            },
        )
        .expect("the synthetic project registers");
}

fn item(value: &str) -> WorkItemId {
    WorkItemId::declare(value).expect("a bounded work-item identity")
}

fn project(value: &str) -> ProjectId {
    ProjectId::declare(value).expect("a bounded project identity")
}

fn work_ref(project_id: &str, work_item_id: &str) -> WorkRef {
    WorkRef::new(project(project_id), item(work_item_id))
}

fn local_dependency(
    project_id: &str,
    consumer: &str,
    producer: &str,
    path: &str,
) -> WorkDependency {
    WorkDependency {
        project_id: project(project_id),
        work_item_id: item(consumer),
        artifact: ArtifactReference::local(item(producer), path).expect("a declared location"),
    }
}

fn cross_project_dependency(
    project_id: &str,
    consumer: &str,
    producer_project_id: &str,
    producer: &str,
) -> WorkDependency {
    WorkDependency {
        project_id: project(project_id),
        work_item_id: item(consumer),
        artifact: ArtifactReference::cross_project(project(producer_project_id), item(producer)),
    }
}

fn states(declared: &[DeclaredDependency]) -> Vec<(WorkRef, WorkRef, ArtifactState)> {
    declared
        .iter()
        .map(|entry| (entry.consumer(), entry.producer(), entry.artifact_state))
        .collect()
}

#[test]
fn a_local_dependency_reports_materialization_explicitly_and_never_freezes_it() {
    let root = TempRoot::new("local-state");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorized = root.authorized_root("alpha");
    register(&store, "alpha-project", &authorized);
    root.write(&authorized, "dist/out.bin", "synthetic build output");

    let present = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("a declared location inside the root is admitted");
    assert_eq!(present.dependency_sequence, 1);
    assert_eq!(present.artifact_state, ArtifactState::Materialized);

    let absent = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "package",
            "test",
            "dist/pending.bin",
        ))
        .expect("an absent location is admitted as missing, not refused");
    assert_eq!(absent.dependency_sequence, 2);
    assert_eq!(absent.artifact_state, ArtifactState::Missing);

    assert_eq!(
        states(
            &store
                .dependencies(&project("alpha-project"))
                .expect("the listing succeeds")
        ),
        vec![
            (
                work_ref("alpha-project", "test"),
                work_ref("alpha-project", "build"),
                ArtifactState::Materialized,
            ),
            (
                work_ref("alpha-project", "package"),
                work_ref("alpha-project", "test"),
                ArtifactState::Missing,
            ),
        ]
    );
    let unresolved = store
        .unresolved_artifacts(&project("alpha-project"))
        .expect("the report succeeds");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(
        unresolved[0].consumer(),
        work_ref("alpha-project", "package")
    );
    assert_eq!(
        unresolved[0].dependency.artifact,
        ArtifactReference::local(item("test"), "dist/pending.bin").expect("a declared location"),
        "the report names the declared reference itself, not a substitute"
    );

    // The state describes the result, not a remembered answer: once the
    // producer's output exists, the same declaration reports it.
    root.write(&authorized, "dist/pending.bin", "synthetic later output");
    assert!(
        store
            .unresolved_artifacts(&project("alpha-project"))
            .expect("the report succeeds")
            .is_empty(),
        "a materialized result is no longer reported unresolved"
    );
}

#[test]
fn a_dependency_that_would_close_a_cycle_is_refused_with_its_path() {
    let root = TempRoot::new("cycle");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);

    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "lint",
            "build",
            "build/lint.log",
        ))
        .expect("lint depends on build");
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "package",
            "lint",
            "dist/package.tar",
        ))
        .expect("package depends on lint");
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "package",
        ))
        .expect("bravo integrates alpha's package");

    // Within one project: build -> package -> lint -> build.
    let failure = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "build",
            "package",
            "dist/rebuilt.tar",
        ))
        .expect_err("an edge that would close a cycle is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");
    assert_eq!(failure.stage(), "project/dependency");
    let path = failure.detail().expect("the refusal names the cycle");
    assert_eq!(
        path,
        "alpha-project/build -> alpha-project/package -> alpha-project/lint -> alpha-project/build"
    );

    // Across projects: alpha/package -> bravo/integrate -> alpha/package.
    let failure = store
        .admit_dependency(&cross_project_dependency(
            "alpha-project",
            "package",
            "bravo-project",
            "integrate",
        ))
        .expect_err("a cross-project cycle is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");
    assert_eq!(
        failure.detail(),
        Some("alpha-project/package -> bravo-project/integrate -> alpha-project/package")
    );

    // A work item depending on itself is the shortest cycle.
    let failure = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "build",
            "build",
            "build/output.bin",
        ))
        .expect_err("a self edge is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");
    assert_eq!(
        failure.detail(),
        Some("alpha-project/build -> alpha-project/build")
    );

    assert_eq!(
        store
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds")
            .len(),
        2,
        "a refused edge leaves no row behind"
    );
}

#[test]
fn a_cross_project_reference_to_an_unauthorized_project_is_refused() {
    let root = TempRoot::new("unauthorized");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    register(&store, "alpha-project", &alpha);

    let failure = store
        .admit_dependency(&cross_project_dependency(
            "alpha-project",
            "test",
            "ghost-project",
            "build",
        ))
        .expect_err("a reference to an unregistered project is refused");
    assert_eq!(failure.code(), "project_artifact_reference_unauthorized");
    assert_eq!(failure.stage(), "project/dependency");
    assert_eq!(failure.detail(), Some("ghost-project"));

    let failure = store
        .admit_dependency(&local_dependency(
            "ghost-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect_err("a dependency declared by an unregistered project is refused");
    assert_eq!(failure.code(), "project_dependency_project_unauthorized");
    assert_eq!(failure.stage(), "project/dependency");

    assert_eq!(
        store
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds"),
        Vec::new()
    );
}

#[test]
fn a_reference_that_escapes_the_authorized_root_is_refused() {
    let root = TempRoot::new("escape");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    register(&store, "alpha-project", &alpha);

    // Two real locations a following implementation would have reached: a
    // sibling beside the authorized root, and an absolute location.
    let sibling = root.beside_authorized_roots("alpha-secret");
    let leak = root.write(&sibling, "leak.txt", "synthetic sibling content");
    let absolute = root.write(root.path(), "absolute.txt", "synthetic absolute content");

    for escaped in [
        "../outside/alpha-secret/leak.txt".to_owned(),
        "dist/../../outside/alpha-secret/leak.txt".to_owned(),
        absolute.to_string_lossy().into_owned(),
    ] {
        let failure = store
            .admit_dependency(&local_dependency(
                "alpha-project",
                "test",
                "build",
                &escaped,
            ))
            .expect_err("a location outside the authorized root is refused");
        assert_eq!(
            failure.code(),
            "project_artifact_reference_escapes_authorized_root"
        );
        assert_eq!(failure.stage(), "project/dependency");
        assert!(
            failure
                .detail()
                .is_some_and(|detail| detail.contains(&escaped)),
            "the refusal names the location it refused"
        );
    }

    assert_eq!(
        store
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds"),
        Vec::new(),
        "a refused reference leaves no row behind"
    );
    assert_eq!(
        std::fs::read_to_string(&leak).expect("the sibling location is untouched"),
        "synthetic sibling content"
    );
}

#[test]
fn admitting_the_same_edge_twice_is_idempotent() {
    let root = TempRoot::new("duplicate");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    register(&store, "alpha-project", &alpha);
    root.write(&alpha, "dist/out.bin", "synthetic build output");

    let first = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("the edge admits");
    let again = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("the same edge admits again without error");
    assert_eq!(again, first);
    assert_eq!(again.dependency_sequence, 1);

    // A different declared result is a different input, not a duplicate.
    let other = store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/other.bin",
        ))
        .expect("a second declared result admits");
    assert_eq!(other.dependency_sequence, 2);
    assert_eq!(
        store
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds")
            .len(),
        2
    );

    // A cross-project duplicate is refused the same way rather than stored
    // twice: the empty local path keeps SQLite's `NULL` distinctness out of it.
    let bravo = root.authorized_root("bravo");
    register(&store, "bravo-project", &bravo);
    let cross = cross_project_dependency("bravo-project", "integrate", "alpha-project", "build");
    let first = store
        .admit_dependency(&cross)
        .expect("the cross-project edge admits");
    let again = store
        .admit_dependency(&cross)
        .expect("the same cross-project edge admits again");
    assert_eq!(again, first);
    assert_eq!(
        store
            .dependencies(&project("bravo-project"))
            .expect("the listing succeeds")
            .len(),
        1
    );
}

#[test]
fn a_blocked_producer_blocks_exactly_its_transitive_consumers() {
    let root = TempRoot::new("consumers");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);

    // alpha: build -> test -> package, with independent lint.
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("test depends on build");
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "package",
            "test",
            "dist/package.tar",
        ))
        .expect("package depends on test");
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "lint",
            "build",
            "dist/lint.log",
        ))
        .expect("lint depends on build");
    // bravo: integrate consumes alpha/build across projects; release follows
    // integrate; docs is an unrelated branch with its own producer.
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "build",
        ))
        .expect("bravo integrates alpha's build");
    store
        .admit_dependency(&local_dependency(
            "bravo-project",
            "release",
            "integrate",
            "dist/release.tar",
        ))
        .expect("release depends on integrate");
    store
        .admit_dependency(&local_dependency(
            "bravo-project",
            "docs",
            "docs-source",
            "docs/site.html",
        ))
        .expect("docs depends on its own source");

    assert_eq!(
        store
            .blocked_consumers(&work_ref("alpha-project", "build"))
            .expect("the consumer set is readable"),
        vec![
            work_ref("alpha-project", "lint"),
            work_ref("alpha-project", "test"),
            work_ref("bravo-project", "integrate"),
            work_ref("alpha-project", "package"),
            work_ref("bravo-project", "release"),
        ],
        "exactly the declared consumers, transitively, and nothing else"
    );
    assert!(
        !store
            .blocked_consumers(&work_ref("alpha-project", "build"))
            .expect("the consumer set is readable")
            .contains(&work_ref("bravo-project", "docs")),
        "an unrelated branch does not wait on a blocked producer"
    );
    assert_eq!(
        store
            .blocked_consumers(&work_ref("bravo-project", "docs-source"))
            .expect("the unrelated branch keeps its own consumer set"),
        vec![work_ref("bravo-project", "docs")]
    );

    // A blocked leaf blocks nothing, and an unrelated producer blocks nothing.
    assert_eq!(
        store
            .blocked_consumers(&work_ref("bravo-project", "release"))
            .expect("the consumer set is readable"),
        Vec::new()
    );
    assert_eq!(
        store
            .blocked_consumers(&work_ref("alpha-project", "missing"))
            .expect("an undeclared producer blocks nothing"),
        Vec::new()
    );

    // The cross-project consumer's input is materialized because the producer's
    // own project declares that work item.
    let integrate = store
        .dependencies(&project("bravo-project"))
        .expect("the listing succeeds")
        .into_iter()
        .find(|declared| declared.consumer() == work_ref("bravo-project", "integrate"))
        .expect("the cross-project edge is stored");
    assert_eq!(integrate.artifact_state, ArtifactState::Materialized);
}

#[test]
fn a_missing_reference_is_reported_explicitly_rather_than_blocking_silently() {
    let root = TempRoot::new("missing");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);

    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/absent.bin",
        ))
        .expect("an absent local location admits as missing");
    // bravo names an unpublished result of a registered project: the reference
    // is authorized, and the answer is still explicit.
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "ghost",
        ))
        .expect("an undeclared referenced work item admits as missing");

    let unresolved = store
        .unresolved_artifacts(&project("bravo-project"))
        .expect("the report succeeds");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].artifact_state, ArtifactState::Missing);
    assert_eq!(
        unresolved[0].dependency.artifact,
        ArtifactReference::cross_project(project("alpha-project"), item("ghost")),
        "the report keeps the declared reference"
    );
    assert_eq!(
        store
            .unresolved_artifacts(&project("alpha-project"))
            .expect("the report succeeds")
            .first()
            .map(|declared| declared.artifact_state),
        Some(ArtifactState::Missing)
    );

    // Once alpha declares the work item, the same reference is materialized.
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "publish",
            "ghost",
            "dist/ghost.bin",
        ))
        .expect("alpha declares the referenced work item");
    assert!(
        store
            .unresolved_artifacts(&project("bravo-project"))
            .expect("the report succeeds")
            .is_empty(),
        "the declared work item is now the reference's materialized result"
    );
}

#[test]
fn dependencies_and_their_states_survive_a_reopen() {
    let root = TempRoot::new("reopen");
    let authorized = root.authorized_root("alpha");
    root.write(&authorized, "dist/out.bin", "synthetic build output");
    let stored = {
        let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
        register(&store, "alpha-project", &authorized);
        store
            .admit_dependency(&local_dependency(
                "alpha-project",
                "test",
                "build",
                "dist/out.bin",
            ))
            .expect("test depends on build");
        store
            .admit_dependency(&local_dependency(
                "alpha-project",
                "package",
                "test",
                "dist/absent.bin",
            ))
            .expect("package depends on test");
        store
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds")
    };
    assert_eq!(stored.len(), 2);

    let reopened = ProjectIdentityStore::open(root.path()).expect("the store reopens");
    assert_eq!(
        reopened
            .dependencies(&project("alpha-project"))
            .expect("the listing succeeds"),
        stored,
        "the declarations and their explicit states survive the reopen"
    );
    assert_eq!(
        reopened
            .blocked_consumers(&work_ref("alpha-project", "build"))
            .expect("the consumer set is readable"),
        vec![
            work_ref("alpha-project", "test"),
            work_ref("alpha-project", "package"),
        ]
    );
    // Admission order keeps counting after a reopen, and the same edge is still
    // the same edge rather than a second one.
    let again = reopened
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("the same edge admits again after the reopen");
    assert_eq!(again.dependency_sequence, 1);
    let next = reopened
        .admit_dependency(&local_dependency(
            "alpha-project",
            "lint",
            "build",
            "dist/lint.log",
        ))
        .expect("a new edge admits after the reopen");
    assert_eq!(next.dependency_sequence, 3);
}

#[cfg(unix)]
#[test]
fn a_symbolically_linked_location_is_unavailable_and_never_followed_outside() {
    let root = TempRoot::new("symlink");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    register(&store, "alpha-project", &alpha);

    let outside = root.beside_authorized_roots("elsewhere");
    let secret = root.write(&outside, "secret.txt", "synthetic outside content");
    let linked_file = alpha.join("linked-out.bin");
    std::os::unix::fs::symlink(&secret, &linked_file).expect("the synthetic link is creatable");
    let linked_dir = alpha.join("linked-dir");
    std::os::unix::fs::symlink(&outside, &linked_dir).expect("the synthetic link is creatable");

    for linked in ["linked-out.bin", "linked-dir/secret.txt"] {
        let declared = store
            .admit_dependency(&local_dependency("alpha-project", "test", "build", linked))
            .expect("a linked location is admitted and reported, not followed");
        assert_eq!(
            declared.artifact_state,
            ArtifactState::Unavailable,
            "{linked} must not be resolved through a symbolic link"
        );
    }
    let unresolved = store
        .unresolved_artifacts(&project("alpha-project"))
        .expect("the report succeeds");
    assert_eq!(unresolved.len(), 2);
    assert!(
        unresolved
            .iter()
            .all(|entry| entry.artifact_state == ArtifactState::Unavailable)
    );
    assert_eq!(
        std::fs::read_to_string(&secret).expect("the outside location is untouched"),
        "synthetic outside content"
    );
}

#[test]
fn the_dependency_index_shares_the_identity_database_without_a_credential_column() {
    let root = TempRoot::new("one-database");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    register(&store, "alpha-project", &alpha);
    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "test",
            "build",
            "dist/out.bin",
        ))
        .expect("the edge admits");

    let connection = Connection::open(store.database_path()).expect("the database is readable");
    let tables = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .expect("the schema is readable")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("the schema query runs")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("the schema rows decode");
    assert!(
        tables.iter().any(|table| table == "project_identities")
            && tables.iter().any(|table| table == "project_dependencies"),
        "identities and dependencies share one database: {tables:?}"
    );
    let version: String = connection
        .query_row(
            "SELECT value FROM project_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .expect("the schema version is readable");
    assert_eq!(version, PROJECT_STORE_SCHEMA_VERSION);

    let mut statement = connection
        .prepare("PRAGMA table_info(project_dependencies)")
        .expect("the schema is readable");
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("the schema query runs")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("the schema rows decode");
    assert_eq!(
        columns.iter().map(String::as_str).collect::<Vec<_>>(),
        PROJECT_DEPENDENCY_COLUMNS
    );
    assert_eq!(
        columns
            .iter()
            .filter(|column| {
                let column = column.to_ascii_lowercase();
                column.contains("secret")
                    || column.contains("credential")
                    || column.contains("token")
                    || column.contains("key")
            })
            .count(),
        0,
        "the record has no column that could hold a credential"
    );
}

/// A database written by an earlier unpublished shape is refused by name rather
/// than opened as a store whose dependency index is missing.
#[test]
fn an_earlier_store_shape_is_refused_by_name() {
    let root = TempRoot::new("old-shape");
    let directory = root.path().join("client-state").join("project-plan");
    std::fs::create_dir_all(&directory).expect("the state directory is creatable");
    let database = directory.join("project-identities.sqlite3");
    let connection = Connection::open(&database).expect("the synthetic database is creatable");
    connection
        .execute_batch(
            "CREATE TABLE project_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO project_meta(key, value) VALUES ('version', '1');
             CREATE TABLE project_identities(
               registration_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
               project_id TEXT NOT NULL UNIQUE,
               display_name TEXT NOT NULL,
               authorized_root TEXT NOT NULL,
               authority_kind TEXT NOT NULL,
               authority_reference TEXT NOT NULL,
               workspace_id TEXT NOT NULL,
               plan_id TEXT NOT NULL,
               UNIQUE(workspace_id, plan_id)
             );",
        )
        .expect("the earlier shape is writable");
    drop(connection);

    let failure = ProjectIdentityStore::open(root.path())
        .expect_err("an earlier shape is not the current store");
    assert_eq!(failure.code(), "project_identity_store_unavailable");
    assert_eq!(failure.stage(), "project/store");
    assert!(
        failure
            .detail()
            .is_some_and(|detail| detail.contains("project_identity_schema_migration_required")),
        "the refusal names the migration: {:?}",
        failure.detail()
    );
    assert!(matches!(failure, ProjectFailure { .. }));
}
