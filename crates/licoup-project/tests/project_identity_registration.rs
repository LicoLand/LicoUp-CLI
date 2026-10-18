//! Registration, refusal and durability of authorized project identities.
//!
//! Every case here is synthetic: the authorized roots are temporary
//! directories, the authority references name no real deployment grant, and no
//! case reads a filesystem to discover an identity.

use licoup_project::{
    AuthorityKind, AuthorityReference, NoAuthorityDirectory, PROJECT_IDENTITY_COLUMNS,
    ProjectAuthorityDirectory, ProjectFailure, ProjectId, ProjectIdentitySource,
    ProjectIdentityStore, ProjectRegistration,
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
            "licoup-project-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the temporary root is creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// An absolute declared root that does not exist: the owner must never need
    /// it to exist, because it never opens it.
    fn declared_root(&self, label: &str) -> String {
        self.path
            .join("authorized")
            .join(label)
            .to_string_lossy()
            .into_owned()
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

fn registration(
    project_id: &str,
    workspace_id: &str,
    plan_id: &str,
    authorized_root: &str,
    authority: Option<AuthorityReference>,
) -> ProjectRegistration {
    ProjectRegistration {
        identity: ProjectIdentitySource::Declared {
            project_id: project_id.to_owned(),
        },
        display_name: format!("Synthetic project {project_id}"),
        authorized_root: authorized_root.to_owned(),
        authority,
        workspace_id: workspace_id.to_owned(),
        plan_id: plan_id.to_owned(),
    }
}

#[test]
fn two_synthetic_authorized_projects_register_and_read_back() {
    let root = TempRoot::new("two-projects");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);

    let alpha = store
        .register(
            &authorities,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                Some(owner_authority()),
            ),
        )
        .expect("alpha registers");
    let bravo = store
        .register(
            &authorities,
            &registration(
                "bravo-project",
                "workspace:shared",
                "plan:bravo",
                &root.declared_root("bravo"),
                Some(owner_authority()),
            ),
        )
        .expect("bravo registers");

    assert_eq!(alpha.project_id.as_str(), "alpha-project");
    assert_eq!(alpha.workspace_id.as_str(), "workspace:shared");
    assert_eq!(alpha.plan_id.as_str(), "plan:alpha");
    assert_eq!(alpha.authority.reference(), "membership:owner");
    assert_eq!(alpha.registration_sequence, 1);
    assert_eq!(bravo.registration_sequence, 2);

    let read = store
        .read(&ProjectId::declare("alpha-project").expect("a bounded identity"))
        .expect("the read succeeds")
        .expect("alpha is registered");
    assert_eq!(read, alpha);
    assert_eq!(
        store
            .read(&ProjectId::declare("missing-project").expect("a bounded identity"))
            .expect("the read succeeds"),
        None
    );
    assert_eq!(
        store.list().expect("the listing succeeds"),
        vec![alpha, bravo]
    );
}

#[test]
fn a_duplicate_project_identity_is_refused_and_leaves_one_record() {
    let root = TempRoot::new("duplicate-project");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);
    let first = registration(
        "alpha-project",
        "workspace:shared",
        "plan:alpha",
        &root.declared_root("alpha"),
        Some(owner_authority()),
    );
    store
        .register(&authorities, &first)
        .expect("alpha registers");

    let duplicate = registration(
        "alpha-project",
        "workspace:shared",
        "plan:other",
        &root.declared_root("elsewhere"),
        Some(owner_authority()),
    );
    let failure = store
        .register(&authorities, &duplicate)
        .expect_err("a duplicate identity is refused");
    assert_eq!(failure.code(), "project_identity_duplicate");
    assert_eq!(failure.stage(), "project/register");
    assert_eq!(store.list().expect("the listing succeeds").len(), 1);
}

#[test]
fn a_duplicate_plan_identity_in_one_workspace_is_refused() {
    let root = TempRoot::new("duplicate-plan");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);
    store
        .register(
            &authorities,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:shared",
                &root.declared_root("alpha"),
                Some(owner_authority()),
            ),
        )
        .expect("alpha registers");

    let failure = store
        .register(
            &authorities,
            &registration(
                "bravo-project",
                "workspace:shared",
                "plan:shared",
                &root.declared_root("bravo"),
                Some(owner_authority()),
            ),
        )
        .expect_err("a duplicate plan identity in one workspace is refused");
    assert_eq!(failure.code(), "project_plan_identity_duplicate");
    assert_eq!(store.list().expect("the listing succeeds").len(), 1);

    // The same plan identity in a different workspace is a different plan, so
    // it is not a duplicate.
    store
        .register(
            &authorities,
            &registration(
                "bravo-project",
                "workspace:other",
                "plan:shared",
                &root.declared_root("bravo"),
                Some(owner_authority()),
            ),
        )
        .expect("the same plan identity in another workspace registers");
}

#[test]
fn an_absent_authority_reference_is_refused() {
    let root = TempRoot::new("absent-authority");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);

    let failure = store
        .register(
            &authorities,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                None,
            ),
        )
        .expect_err("a registration with no authority is refused");
    assert_eq!(failure.code(), "project_authority_reference_required");
    assert_eq!(store.list().expect("the listing succeeds"), Vec::new());
}

#[test]
fn an_unauthorized_authority_reference_is_refused() {
    let root = TempRoot::new("unauthorized-authority");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);
    let foreign =
        AuthorityReference::membership("membership:somebody-else").expect("a bounded reference");

    let failure = store
        .register(
            &authorities,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                Some(foreign),
            ),
        )
        .expect_err("an authority the owner does not admit is refused");
    assert_eq!(failure.code(), "project_authority_unauthorized");

    // The fail-closed owner admits nothing at all.
    let failure = store
        .register(
            &NoAuthorityDirectory,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                Some(owner_authority()),
            ),
        )
        .expect_err("an uncomposed authority owner admits nothing");
    assert_eq!(failure.code(), "project_authority_unauthorized");
    assert_eq!(store.list().expect("the listing succeeds"), Vec::new());
}

#[test]
fn an_identity_that_would_require_scanning_a_root_is_refused() {
    let root = TempRoot::new("scan-refused");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);
    let scanned = root.path().join("authorized").join("scanned");
    std::fs::create_dir_all(&scanned).expect("the synthetic root is creatable");
    let marker = scanned.join("Cargo.toml");
    std::fs::write(&marker, "[package]\nname = \"scanned\"\n").expect("the marker is writable");

    let failure = store
        .register(
            &authorities,
            &ProjectRegistration {
                identity: ProjectIdentitySource::Discovered {
                    authorized_root: scanned.to_string_lossy().into_owned(),
                },
                display_name: "Whatever the directory turns out to be".to_owned(),
                authorized_root: scanned.to_string_lossy().into_owned(),
                authority: Some(owner_authority()),
                workspace_id: "workspace:shared".to_owned(),
                plan_id: "plan:discovered".to_owned(),
            },
        )
        .expect_err("an identity that needs a directory walk is refused");
    assert_eq!(failure.code(), "project_identity_scan_required");
    assert_eq!(failure.stage(), "project/register");
    assert_eq!(store.list().expect("the listing succeeds"), Vec::new());
    assert_eq!(
        std::fs::read_to_string(&marker).expect("the marker is readable"),
        "[package]\nname = \"scanned\"\n",
        "the refused registration must not have touched the root it named"
    );
}

#[test]
fn a_registration_carrying_an_undeclared_field_is_refused() {
    // A payload that smuggles a credential beside the declaration is not a
    // registration this owner can honour.
    let smuggled = serde_json::json!({
        "identity": {"kind": "declared", "projectId": "alpha-project"},
        "displayName": "Synthetic project alpha-project",
        "authorizedRoot": "/synthetic/root",
        "authority": {"kind": "membership", "reference": "membership:owner"},
        "workspaceId": "workspace:shared",
        "planId": "plan:alpha",
        "credential": "synthetic-secret-material",
    });
    let decoded: Result<ProjectRegistration, _> = serde_json::from_value(smuggled);
    assert!(
        decoded.is_err(),
        "an undeclared credential field is refused rather than dropped"
    );

    // The same payload without the smuggled field decodes, so the refusal above
    // is the undeclared field and not a shape mismatch.
    let declared: Result<ProjectRegistration, _> = serde_json::from_value(serde_json::json!({
        "identity": {"kind": "declared", "projectId": "alpha-project"},
        "displayName": "Synthetic project alpha-project",
        "authorizedRoot": "/synthetic/root",
        "authority": {"kind": "membership", "reference": "membership:owner"},
        "workspaceId": "workspace:shared",
        "planId": "plan:alpha",
    }));
    assert!(declared.is_ok(), "{declared:?}");
}

#[test]
fn an_invalid_declared_root_or_display_name_is_refused() {
    let root = TempRoot::new("invalid-declaration");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let authorities = GrantedAuthorities::granting([owner_authority()]);

    let relative = registration(
        "alpha-project",
        "workspace:shared",
        "plan:alpha",
        "relative/root",
        Some(owner_authority()),
    );
    assert_eq!(
        store
            .register(&authorities, &relative)
            .expect_err("a relative root is refused")
            .code(),
        "project_authorized_root_required"
    );

    let mut unnamed = registration(
        "alpha-project",
        "workspace:shared",
        "plan:alpha",
        &root.declared_root("alpha"),
        Some(owner_authority()),
    );
    unnamed.display_name = "   ".to_owned();
    assert_eq!(
        store
            .register(&authorities, &unnamed)
            .expect_err("an empty display name is refused")
            .code(),
        "project_display_name_required"
    );
    assert_eq!(store.list().expect("the listing succeeds"), Vec::new());
}

#[test]
fn registration_survives_a_reopen() {
    let root = TempRoot::new("reopen");
    let authorities = GrantedAuthorities::granting([owner_authority()]);
    let registered = {
        let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
        let registered = store
            .register(
                &authorities,
                &registration(
                    "alpha-project",
                    "workspace:shared",
                    "plan:alpha",
                    &root.declared_root("alpha"),
                    Some(owner_authority()),
                ),
            )
            .expect("alpha registers");
        assert_eq!(registered.registration_sequence, 1);
        registered
    };

    let reopened = ProjectIdentityStore::open(root.path()).expect("the store reopens");
    assert_eq!(
        reopened
            .read(&ProjectId::declare("alpha-project").expect("a bounded identity"))
            .expect("the read succeeds")
            .expect("alpha survived the reopen"),
        registered
    );
    // A reopen must not reset registration order.
    let next = reopened
        .register(
            &authorities,
            &registration(
                "bravo-project",
                "workspace:shared",
                "plan:bravo",
                &root.declared_root("bravo"),
                Some(owner_authority()),
            ),
        )
        .expect("bravo registers after the reopen");
    assert_eq!(next.registration_sequence, 2);
}

#[test]
fn the_persisted_record_keeps_an_authority_reference_and_no_credential_column() {
    let root = TempRoot::new("reference-only");
    let store = ProjectIdentityStore::open(root.path()).expect("the store opens");
    let grant = AuthorityReference::declare(AuthorityKind::Grant, "grant:project-authority")
        .expect("a bounded grant reference");
    let authorities = GrantedAuthorities::granting([grant.clone()]);
    store
        .register(
            &authorities,
            &registration(
                "alpha-project",
                "workspace:shared",
                "plan:alpha",
                &root.declared_root("alpha"),
                Some(grant),
            ),
        )
        .expect("alpha registers");

    let connection = Connection::open(store.database_path()).expect("the database is readable");
    let mut statement = connection
        .prepare("PRAGMA table_info(project_identities)")
        .expect("the schema is readable");
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .expect("the schema query runs")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("the schema rows decode");
    assert_eq!(
        columns.iter().map(String::as_str).collect::<Vec<_>>(),
        PROJECT_IDENTITY_COLUMNS
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

    let stored: String = connection
        .query_row(
            "SELECT authority_reference FROM project_identities WHERE project_id = 'alpha-project'",
            [],
            |row| row.get(0),
        )
        .expect("the stored reference is readable");
    assert_eq!(stored, "grant:project-authority");
}

/// A store that cannot be opened reports its own code rather than panicking, so
/// a caller is told the durable owner is unavailable.
#[test]
fn an_unavailable_store_is_a_typed_failure() {
    let blocked = TempRoot::new("unavailable");
    let file = blocked.path().join("not-a-directory");
    std::fs::write(&file, b"occupied").expect("the obstruction is writable");
    let failure = ProjectIdentityStore::open(&file).expect_err("the store cannot open");
    assert_eq!(failure.code(), "project_identity_store_unavailable");
    assert_eq!(failure.stage(), "project/store");
    assert!(matches!(failure, ProjectFailure { .. }));
}
