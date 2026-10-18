//! Declared changes: what a preview reaches over the declared dependency edges,
//! which references it resolves or orphans, what it refuses exactly as
//! admission would, and what it asks of an already-started or already-accepted
//! contract.
//!
//! Every case here is synthetic: the authorized roots are temporary
//! directories, the authority references name no real deployment grant, the
//! work owner is a table of answers, and nothing outside a declared root is
//! read.

use licoup_project::{
    AffectedWorkItem, ArtifactReference, ArtifactState, AuthorityReference, ChangeHandoff,
    ChangeImpact, ChangePreview, ChangeRequest, DeclaredDependency, NoWorkActivityDirectory,
    ProjectAuthorityDirectory, ProjectId, ProjectIdentitySource, ProjectIdentityStore,
    ProjectRegistration, WorkActivity, WorkActivityDirectory, WorkDependency, WorkItemChange,
    WorkItemId, WorkRef,
};
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

/// The work owner a test composes: named answers, one fallback for the rest.
struct FixedActivity {
    answers: Vec<(WorkRef, WorkActivity)>,
    fallback: WorkActivity,
}

impl FixedActivity {
    fn answering(answers: impl IntoIterator<Item = (WorkRef, WorkActivity)>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            fallback: WorkActivity::NotStarted,
        }
    }

    /// A work owner that answers nothing, like a process without the owner.
    fn silent() -> Self {
        Self {
            answers: Vec::new(),
            fallback: WorkActivity::Unknown,
        }
    }
}

impl WorkActivityDirectory for FixedActivity {
    fn activity(&self, work: &WorkRef) -> WorkActivity {
        self.answers
            .iter()
            .find(|(answered, _)| answered == work)
            .map(|(_, activity)| *activity)
            .unwrap_or(self.fallback)
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
            "licoup-project-preview-{label}-{}-{nanos}-{unique}",
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

/// One declared change of one work item's inputs.
fn change(project_id: &str, work_item_id: &str, inputs: Vec<ArtifactReference>) -> ChangeRequest {
    ChangeRequest {
        project_id: project(project_id),
        changes: vec![WorkItemChange {
            work_item_id: item(work_item_id),
            inputs,
        }],
    }
}

fn local_input(producer: &str, path: &str) -> ArtifactReference {
    ArtifactReference::local(item(producer), path).expect("a declared location")
}

fn paths(preview: &ChangePreview) -> Vec<(WorkRef, Vec<WorkRef>, ChangeImpact)> {
    preview
        .affected
        .iter()
        .map(|entry| (entry.work.clone(), entry.path.clone(), entry.impact))
        .collect()
}

fn inputs(entry: &AffectedWorkItem) -> Vec<(WorkRef, ArtifactState, bool)> {
    entry
        .inputs
        .iter()
        .map(|input| (input.producer.clone(), input.state, input.pending))
        .collect()
}

fn entry(preview: &ChangePreview, work: WorkRef) -> &AffectedWorkItem {
    preview
        .affected
        .iter()
        .find(|entry| entry.work == work)
        .unwrap_or_else(|| panic!("{work} is expected in the preview"))
}

fn stored_states(store: &ProjectIdentityStore, project_id: &str) -> Vec<DeclaredDependency> {
    store
        .dependencies(&project(project_id))
        .expect("the listing succeeds")
}

/// A change reaches exactly the work items that wait on it, follows them across
/// projects, and reports every other registered project as untouched.
#[test]
fn a_change_reaches_exactly_its_declared_consumers_and_leaves_other_projects_alone() {
    let root = TempRoot::new("reach");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    let charlie = root.authorized_root("charlie");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);
    register(&store, "charlie-project", &charlie);

    // alpha: build -> test -> package, with lint on build.
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
    // bravo: integrate consumes alpha/build across projects; release follows;
    // docs is an unrelated branch.
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

    // Two results really exist on disk: the preview must still report them as
    // pending behind the producer it replaces.
    root.write(&alpha, "dist/out.bin", "synthetic build output");
    root.write(&alpha, "dist/lint.log", "synthetic lint log");
    root.write(&alpha, "build/out.bin", "synthetic rebuild output");

    let request = change(
        "alpha-project",
        "build",
        vec![local_input("source", "build/out.bin")],
    );
    let preview = store
        .preview_change(&NoWorkActivityDirectory, &request)
        .expect("the preview answers");

    assert_eq!(
        paths(&preview),
        vec![
            (
                work_ref("alpha-project", "build"),
                vec![work_ref("alpha-project", "build")],
                ChangeImpact::Declared,
            ),
            (
                work_ref("alpha-project", "lint"),
                vec![
                    work_ref("alpha-project", "build"),
                    work_ref("alpha-project", "lint"),
                ],
                ChangeImpact::Consumer,
            ),
            (
                work_ref("alpha-project", "test"),
                vec![
                    work_ref("alpha-project", "build"),
                    work_ref("alpha-project", "test"),
                ],
                ChangeImpact::Consumer,
            ),
            (
                work_ref("bravo-project", "integrate"),
                vec![
                    work_ref("alpha-project", "build"),
                    work_ref("bravo-project", "integrate"),
                ],
                ChangeImpact::Consumer,
            ),
            (
                work_ref("alpha-project", "package"),
                vec![
                    work_ref("alpha-project", "build"),
                    work_ref("alpha-project", "test"),
                    work_ref("alpha-project", "package"),
                ],
                ChangeImpact::Consumer,
            ),
            (
                work_ref("bravo-project", "release"),
                vec![
                    work_ref("alpha-project", "build"),
                    work_ref("bravo-project", "integrate"),
                    work_ref("bravo-project", "release"),
                ],
                ChangeImpact::Consumer,
            ),
        ],
        "the declared consumers of the changed result, transitively, and nothing else"
    );
    assert_eq!(
        preview.untouched_projects,
        vec![project("charlie-project")],
        "a registered project the change reaches no work item of is not restarted"
    );

    let build = entry(&preview, work_ref("alpha-project", "build"));
    assert_eq!(
        inputs(build),
        vec![(
            work_ref("alpha-project", "source"),
            ArtifactState::Materialized,
            false,
        )],
        "the change's own declaration reads its declared location"
    );
    let lint = entry(&preview, work_ref("alpha-project", "lint"));
    assert_eq!(
        inputs(lint),
        vec![(
            work_ref("alpha-project", "build"),
            ArtifactState::Materialized,
            true,
        )],
        "a present file behind a replaced producer is reported as pending"
    );
    let package = entry(&preview, work_ref("alpha-project", "package"));
    assert_eq!(
        inputs(package),
        vec![(
            work_ref("alpha-project", "test"),
            ArtifactState::Missing,
            true
        )],
        "an absent local result stays explicitly missing"
    );
    let integrate = entry(&preview, work_ref("bravo-project", "integrate"));
    assert_eq!(
        inputs(integrate),
        vec![(
            work_ref("alpha-project", "build"),
            ArtifactState::Materialized,
            true,
        )],
        "a cross-project reference stays materialized by the declared index"
    );
    assert_eq!(integrate.activity, WorkActivity::Unknown);
    assert_eq!(
        integrate.handoff,
        ChangeHandoff::Unresolved,
        "an owner that did not answer leaves the work item unresolved, never fresh"
    );

    assert!(
        !preview
            .affected
            .iter()
            .any(|entry| entry.work == work_ref("bravo-project", "docs")),
        "an unrelated branch is not restarted by the change"
    );
}

/// The refused declarations are exactly the ones admission refuses, with the
/// same codes and stage, and nothing is written on the way.
#[test]
fn a_change_is_refused_by_the_same_rules_admission_applies() {
    let root = TempRoot::new("refusals");
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

    // A location outside the authorized root.
    let sibling = root.beside_authorized_roots("alpha-secret");
    let leak = root.write(&sibling, "leak.txt", "synthetic sibling content");
    let escaped = sibling.join("leak.txt").to_string_lossy().into_owned();
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change(
                "alpha-project",
                "lint",
                vec![local_input("build", &escaped)],
            ),
        )
        .expect_err("an escaped location is refused");
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
    assert_eq!(
        std::fs::read_to_string(&leak).expect("the sibling location is untouched"),
        "synthetic sibling content"
    );

    // A cross-project reference to an unregistered project.
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change(
                "alpha-project",
                "lint",
                vec![ArtifactReference::cross_project(
                    project("ghost-project"),
                    item("build"),
                )],
            ),
        )
        .expect_err("an unauthorized cross-project reference is refused");
    assert_eq!(failure.code(), "project_artifact_reference_unauthorized");
    assert_eq!(failure.stage(), "project/dependency");
    assert_eq!(failure.detail(), Some("ghost-project"));

    // A cycle reached through the stored edges.
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change(
                "alpha-project",
                "build",
                vec![local_input("package", "dist/rebuilt.tar")],
            ),
        )
        .expect_err("a cycle through the stored edges is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");
    assert_eq!(
        failure.detail(),
        Some(
            "alpha-project/build -> alpha-project/package -> alpha-project/test -> alpha-project/build"
        )
    );

    // A cycle that only the change's own declarations close.
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "lint",
        ))
        .expect("bravo integrates alpha's lint");
    let request = ChangeRequest {
        project_id: project("alpha-project"),
        changes: vec![
            WorkItemChange {
                work_item_id: item("lint"),
                inputs: vec![local_input("build", "dist/lint.log")],
            },
            WorkItemChange {
                work_item_id: item("build"),
                inputs: vec![ArtifactReference::cross_project(
                    project("bravo-project"),
                    item("integrate"),
                )],
            },
        ],
    };
    let failure = store
        .preview_change(&NoWorkActivityDirectory, &request)
        .expect_err("a cycle among the proposed edges is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");
    assert_eq!(
        failure.detail(),
        Some(
            "alpha-project/build -> bravo-project/integrate -> alpha-project/lint -> alpha-project/build"
        )
    );

    // A work item that would wait on itself.
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change(
                "alpha-project",
                "build",
                vec![local_input("build", "dist/self.bin")],
            ),
        )
        .expect_err("a self edge is refused");
    assert_eq!(failure.code(), "project_dependency_cycle");

    // An unregistered declaring project.
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change("charlie-project", "build", Vec::new()),
        )
        .expect_err("an unregistered declaring project is refused");
    assert_eq!(failure.code(), "project_dependency_project_unauthorized");
    assert_eq!(failure.stage(), "project/dependency");

    // A request that declares nothing, names one work item twice, or exceeds
    // the declared bound.
    let failure = store
        .preview_change(
            &NoWorkActivityDirectory,
            &ChangeRequest {
                project_id: project("alpha-project"),
                changes: Vec::new(),
            },
        )
        .expect_err("an empty change is refused rather than previewed as unaffected");
    assert_eq!(failure.code(), "project_change_required");
    assert_eq!(failure.stage(), "project/change");

    let duplicated = ChangeRequest {
        project_id: project("alpha-project"),
        changes: vec![
            WorkItemChange {
                work_item_id: item("build"),
                inputs: Vec::new(),
            },
            WorkItemChange {
                work_item_id: item("build"),
                inputs: Vec::new(),
            },
        ],
    };
    let failure = store
        .preview_change(&NoWorkActivityDirectory, &duplicated)
        .expect_err("one work item declared twice is refused");
    assert_eq!(failure.code(), "project_change_work_item_duplicate");
    assert_eq!(failure.stage(), "project/change");

    assert_eq!(
        stored_states(&store, "alpha-project").len(),
        2,
        "a refused preview leaves the declarations exactly as they were"
    );
    assert_eq!(
        stored_states(&store, "bravo-project").len(),
        1,
        "and adds nothing to another project"
    );
}

/// What the preview reports for a declaration is what admission stores for it,
/// including the reference the declaration resolves.
#[test]
fn a_previewed_declaration_reports_what_admission_then_stores() {
    let root = TempRoot::new("agreement");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "shared",
        ))
        .expect("bravo references a result alpha has not declared");
    root.write(&alpha, "build/present.bin", "synthetic present output");

    let request = ChangeRequest {
        project_id: project("alpha-project"),
        changes: vec![WorkItemChange {
            work_item_id: item("build"),
            inputs: vec![
                local_input("shared", "build/present.bin"),
                local_input("shared", "build/absent.bin"),
            ],
        }],
    };
    let preview = store
        .preview_change(&NoWorkActivityDirectory, &request)
        .expect("the preview answers");

    let build = entry(&preview, work_ref("alpha-project", "build"));
    assert_eq!(
        inputs(build),
        vec![
            (
                work_ref("alpha-project", "shared"),
                ArtifactState::Materialized,
                false,
            ),
            (
                work_ref("alpha-project", "shared"),
                ArtifactState::Missing,
                false,
            ),
        ]
    );
    // Declaring the producer resolves bravo's reference, and the preview names
    // the declaration that does it.
    let integrate = entry(&preview, work_ref("bravo-project", "integrate"));
    assert_eq!(
        integrate.impact,
        ChangeImpact::Unblocked,
        "a reference to a work item the change declares starts resolving"
    );
    assert_eq!(
        integrate.path,
        vec![
            work_ref("alpha-project", "build"),
            work_ref("alpha-project", "shared"),
            work_ref("bravo-project", "integrate"),
        ]
    );
    assert_eq!(
        inputs(integrate),
        vec![(
            work_ref("alpha-project", "shared"),
            ArtifactState::Materialized,
            false,
        )],
        "the reference resolves under the declaration the change leaves behind"
    );

    // The same declarations, admitted one by one, store exactly those states.
    let reported: Vec<ArtifactState> = build.inputs.iter().map(|input| input.state).collect();
    let admitted: Vec<ArtifactState> = request.changes[0]
        .inputs
        .iter()
        .map(|declared| {
            store
                .admit_dependency(&WorkDependency {
                    project_id: project("alpha-project"),
                    work_item_id: item("build"),
                    artifact: declared.clone(),
                })
                .expect("the previewed declaration admits")
                .artifact_state
        })
        .collect();
    assert_eq!(
        admitted, reported,
        "admission stores the states the preview reported"
    );
    assert!(
        store
            .unresolved_artifacts(&project("bravo-project"))
            .expect("the report succeeds")
            .is_empty(),
        "the reference the preview reported unblocked is materialized after admission"
    );
}

/// A declaration the change removes is reported, and the reference it orphans
/// stays explicitly missing rather than assumed.
#[test]
fn a_change_that_orphans_a_reference_reports_it_as_blocked() {
    let root = TempRoot::new("orphan");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let alpha = root.authorized_root("alpha");
    let bravo = root.authorized_root("bravo");
    register(&store, "alpha-project", &alpha);
    register(&store, "bravo-project", &bravo);
    root.write(&alpha, "build/out.bin", "synthetic build output");

    store
        .admit_dependency(&local_dependency(
            "alpha-project",
            "build",
            "shared",
            "build/out.bin",
        ))
        .expect("alpha's build takes shared's result");
    store
        .admit_dependency(&cross_project_dependency(
            "bravo-project",
            "integrate",
            "alpha-project",
            "shared",
        ))
        .expect("bravo integrates alpha's shared result");
    assert!(
        store
            .unresolved_artifacts(&project("bravo-project"))
            .expect("the report succeeds")
            .is_empty(),
        "the reference resolves while alpha declares shared"
    );

    // The change re-declares build with no input at all: shared is no longer
    // declared by alpha, so bravo's reference stops resolving.
    let preview = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change("alpha-project", "build", Vec::new()),
        )
        .expect("the preview answers");
    let integrate = entry(&preview, work_ref("bravo-project", "integrate"));
    assert_eq!(
        integrate.impact,
        ChangeImpact::Blocked,
        "a reference to a work item the change stops declaring stops resolving"
    );
    assert_eq!(
        integrate.path,
        vec![
            work_ref("alpha-project", "build"),
            work_ref("alpha-project", "shared"),
            work_ref("bravo-project", "integrate"),
        ]
    );
    assert_eq!(
        inputs(integrate),
        vec![(
            work_ref("alpha-project", "shared"),
            ArtifactState::Missing,
            false,
        )],
        "the reference no longer resolves under the declaration the change leaves behind"
    );
    assert!(
        store
            .unresolved_artifacts(&project("bravo-project"))
            .expect("the report succeeds")
            .is_empty(),
        "the preview changed nothing: the stored declaration still resolves"
    );
}

/// A run in flight or an accepted result behind a replaced declaration requires
/// an explicit handoff, and an owner that does not answer is not called fresh.
#[test]
fn an_in_flight_or_accepted_contract_requires_an_explicit_handoff() {
    let root = TempRoot::new("handoff");
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
        .expect("test depends on build");
    root.write(&alpha, "dist/out.bin", "synthetic build output");

    let request = change(
        "alpha-project",
        "build",
        vec![local_input("source", "dist/out.bin")],
    );

    let accepted =
        FixedActivity::answering([(work_ref("alpha-project", "test"), WorkActivity::Accepted)]);
    let preview = store
        .preview_change(&accepted, &request)
        .expect("the preview answers");
    let build = entry(&preview, work_ref("alpha-project", "build"));
    assert_eq!(build.activity, WorkActivity::NotStarted);
    assert_eq!(build.handoff, ChangeHandoff::None);
    let test = entry(&preview, work_ref("alpha-project", "test"));
    assert_eq!(test.activity, WorkActivity::Accepted);
    assert_eq!(
        test.handoff,
        ChangeHandoff::Required,
        "an accepted result never satisfies the changed contract on its own"
    );

    let in_flight =
        FixedActivity::answering([(work_ref("alpha-project", "test"), WorkActivity::InFlight)]);
    assert_eq!(
        entry(
            &store
                .preview_change(&in_flight, &request)
                .expect("the preview answers"),
            work_ref("alpha-project", "test")
        )
        .handoff,
        ChangeHandoff::Required
    );

    // The fail-closed owner answers nothing, so nothing is called fresh.
    let unresolved = store
        .preview_change(&FixedActivity::silent(), &request)
        .expect("the preview answers");
    assert!(
        unresolved
            .affected
            .iter()
            .all(|entry| entry.handoff == ChangeHandoff::Unresolved),
        "an unanswered work item is unresolved rather than fresh"
    );

    // A preview is a query: the declarations and their states are exactly what
    // they were, and a second preview answers identically.
    assert_eq!(
        paths(&unresolved),
        paths(
            &store
                .preview_change(&FixedActivity::silent(), &request)
                .expect("the preview answers again")
        )
    );
    assert_eq!(stored_states(&store, "alpha-project").len(), 1);
    let reopened = ProjectIdentityStore::open(root.path()).expect("the store reopens");
    assert_eq!(
        stored_states(&reopened, "alpha-project").len(),
        1,
        "the store holds the one declaration it held before every preview"
    );
}

/// An independent insertion waits for nothing, still states the declaration it
/// adds, and touches no existing consumer.
#[test]
fn an_independent_insertion_reaches_only_the_declared_work_item() {
    let root = TempRoot::new("insert");
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
        .expect("test depends on build");

    let preview = store
        .preview_change(
            &NoWorkActivityDirectory,
            &change("alpha-project", "publish", Vec::new()),
        )
        .expect("the preview answers");
    assert_eq!(
        paths(&preview),
        vec![(
            work_ref("alpha-project", "publish"),
            vec![work_ref("alpha-project", "publish")],
            ChangeImpact::Declared,
        )],
        "an independent insertion reaches no existing work item"
    );
    assert!(inputs(entry(&preview, work_ref("alpha-project", "publish"))).is_empty());
    assert_eq!(preview.untouched_projects, Vec::new());
}
