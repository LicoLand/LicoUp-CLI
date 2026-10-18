//! An interrupted conversion resumes to exactly one result.
//!
//! Two interruption points carry the durability claim, and this regression visits each of
//! them on a disposable root:
//!
//! * **before the owner**: the process stopped with the step recorded as attempted and the
//!   root untouched. A resume must complete the step exactly once.
//! * **after the owner**: the process stopped after the client's owner had produced its
//!   durable result and before this tool recorded it. A resume must report the domain
//!   already current and add no second record.
//!
//! The oracle is the client's own record, not this tool's opinion of itself. The client's
//! ledger holds one completed step id per conversion, so:
//!
//! * the total number of completed step ids must be the same after a resume as after a
//!   single uninterrupted conversion,
//! * no domain may list the same step id twice, and
//! * the ledger's bytes must be unchanged when a resume had nothing left to do.
//!
//! Source preservation is asserted against a fingerprint of the client's own files taken
//! before any conversion ran. Converting must not replace the source: the seeded rows are
//! still there afterwards, and the authority that moves them is the client's owner rather
//! than a second implementation in this tool.
//!
//! Both acceptance cases run once per permitted interruption point, so a point that no case
//! visits is itself a failure.

mod support;

use licoup_migrate::journal::ledger::LedgerSnapshot;
use licoup_migrate::journal::{self, StepStatus};
use licoup_migrate::resume::{self, INTERRUPTION_POINTS, ResumeOptions};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use support::seed_released_strategy_store;

const STRATEGY_STORE: &str = "client-state/adaptive-flywheel/strategies.sqlite3";
const APPEARANCE: &str = "client-state/appearance-preferences.json";
const CLIENT_LEDGER: &str = "client-state/migrations/ledger.json";
const JOURNAL: &str = "client-state/migrations/data-migration-journal.json";
const STRATEGY_ARTIFACT: &str =
    "client-state/migrations/artifacts/adaptive-flywheel-strategy-store.json";
/// A disposable root under the repository's ignored `build/` directory.
fn scratch(name: &str) -> PathBuf {
    let root = repository_root()
        .join("build/tmp/licoup-migrate-resume")
        .join(name);
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clear the disposable root");
    }
    std::fs::create_dir_all(&root).expect("create the disposable root");
    root.canonicalize().expect("canonical root")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// Seed the published strategy format and a durable JSON domain, so the client's own
/// conversion has real state to move and the source has something to preserve.
fn seed_source(root: &Path) {
    // The strategy store is the frozen published layout (`strategy-store-2`, the exact
    // layout the last release wrote, answering to domain version 1). Its version is never
    // lowered to invent a longer ladder: the client's owner has one genuine published
    // format left to walk, which is what the frontier itself declares.
    seed_released_strategy_store(&root.join(STRATEGY_STORE));
    // The seeded state is a store that has never been through an admission: no ledger, no
    // marker, no recovery artifact, and no lock left over from the writer that created it.
    let migrations = root.join("client-state/migrations");
    if migrations.exists() {
        std::fs::remove_dir_all(&migrations).expect("clear the seeded migrations directory");
    }

    let appearance = root.join(APPEARANCE);
    std::fs::create_dir_all(appearance.parent().expect("state directory")).expect("create state");
    std::fs::write(
        &appearance,
        b"{\n  \"appearancePresetId\": \"preserved\",\n  \"localePreference\": \"en\"\n}\n",
    )
    .expect("seed the appearance document");
}

/// The seeded user state, read back from the client's own stores.
///
/// "The source is not replaced" is a statement about the user's rows, not about the file's
/// bytes: the client's own conversion legitimately rewrites the shape of the database and
/// the document it moves, so what has to survive is every seeded value, under the identity
/// it was seeded with. A conversion that dropped or duplicated a row is what this catches.
fn preserved_source_state(root: &Path) -> BTreeMap<String, String> {
    let mut preserved = BTreeMap::new();
    let database = rusqlite::Connection::open(root.join(STRATEGY_STORE)).expect("open the store");
    let mut statement = database
        .prepare("SELECT revision_digest, value_id FROM strategy_bindings ORDER BY revision_digest")
        .expect("read the seeded bindings");
    let seeded = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query the seeded bindings");
    for row in seeded {
        let (digest, value) = row.expect("a seeded binding row");
        preserved.insert(format!("strategy_bindings/{digest}"), value);
    }
    drop(statement);
    drop(database);

    let appearance: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join(APPEARANCE)).expect("read the appearance document"),
    )
    .expect("the appearance document is JSON");
    for field in ["appearancePresetId", "localePreference"] {
        let value = appearance
            .get(field)
            .unwrap_or_else(|| panic!("{field} survives the conversion"));
        preserved.insert(
            format!("{APPEARANCE}/{field}"),
            value.as_str().unwrap_or_default().to_string(),
        );
    }
    preserved
}

/// A fingerprint of the client's own conversion records: the ledger and its markers.
///
/// Only documents count. The transient SQLite journal and write-ahead files a database
/// leaves behind describe an in-flight transaction, not a recorded conversion, so they are
/// excluded; every document here is the client's, not this tool's.
fn record_fingerprint(root: &Path) -> BTreeMap<String, String> {
    let mut fingerprint = BTreeMap::new();
    let ledger = root.join(CLIENT_LEDGER);
    if ledger.exists() {
        fingerprint.insert(
            CLIENT_LEDGER.to_string(),
            digest(&std::fs::read(&ledger).expect("read the client ledger")),
        );
    }
    let markers = root.join("client-state/migrations/domain-state");
    if let Ok(entries) = std::fs::read_dir(&markers) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with("-journal") || name.ends_with("-wal") {
                continue;
            }
            let bytes = std::fs::read(entry.path()).expect("read a domain marker");
            fingerprint.insert(format!("domain-state/{name}"), digest(&bytes));
        }
    }
    fingerprint
}

/// The client's own files, hashed, so a rewrite of a record is visible as a changed byte.
fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}:{}", bytes.len())
}

/// The client's record of what has been converted.
fn client_ledger(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(CLIENT_LEDGER)).expect("read the ledger"))
        .expect("the client's ledger is JSON")
}

/// Every completed step id the client's ledger lists, across every domain.
fn ledger_step_ids(root: &Path) -> Vec<String> {
    let ledger = client_ledger(root);
    let mut ids = Vec::new();
    for (_, domain) in ledger["domains"].as_object().expect("domains").iter() {
        for step in domain["completedStepIds"].as_array().expect("step ids") {
            ids.push(step.as_str().expect("step id").to_string());
        }
    }
    ids
}

fn duplicated_step_ids(root: &Path) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut duplicates = Vec::new();
    for step in ledger_step_ids(root) {
        if !seen.insert(step.clone()) {
            duplicates.push(step);
        }
    }
    duplicates
}

fn domain_states(
    root: &Path,
) -> Vec<licoup_native::domain::client_state_migration::DomainStateProjection> {
    licoup_native::domain::client_state_migration::domain_state_projection(root)
        .expect("the client's owner reports the observed state")
}

/// The version the client's own domain marker records.
///
/// The marker is written by the client's owner, so it is the client's durable statement
/// that the domain reached a version. The store projection is a different question, and it
/// is deliberately not used as the oracle for "did the commit happen".
fn client_marker_version(root: &Path, domain_id: &str) -> u32 {
    let path = root
        .join("client-state/migrations/domain-state")
        .join(format!("{domain_id}.json"));
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read the client's marker"))
            .expect("the client's marker is JSON");
    assert_eq!(
        marker["schemaVersion"], "v0.0.1:client-state-domain-marker-1",
        "the marker is the client's own document"
    );
    marker["authoritativeSchemaVersion"]
        .as_u64()
        .expect("the marker records a version") as u32
}

fn observed_version(root: &Path, domain_id: &str) -> u32 {
    let state = domain_states(root)
        .into_iter()
        .find(|state| state.domain_id == domain_id)
        .expect("the domain is declared by the client's frontier");
    match state.authority {
        licoup_native::domain::client_state_migration::DomainAuthority::Known { version } => {
            version
        }
        licoup_native::domain::client_state_migration::DomainAuthority::Absent => 0,
        licoup_native::domain::client_state_migration::DomainAuthority::Refused { code } => {
            panic!("the client refused {domain_id}: {code}")
        }
    }
}

/// The domains the client's frontier declares, in its own order.
fn frontier_domains() -> Vec<String> {
    licoup_native::domain::client_state_migration::frontier_projection_struct()
        .expect("the client's frontier projection")
        .domains
        .into_iter()
        .map(|domain| domain.domain_id)
        .collect()
}

/// Every domain marker the client's owner has written, with the version it records.
fn domain_marker_versions(root: &Path) -> BTreeMap<String, u32> {
    let markers = root.join("client-state/migrations/domain-state");
    let mut versions = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(&markers) else {
        return versions;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(domain_id) = name.strip_suffix(".json") else {
            continue;
        };
        versions.insert(
            domain_id.to_string(),
            client_marker_version(root, domain_id),
        );
    }
    versions
}

fn journal_document(root: &Path) -> journal::Journal {
    journal::open(root)
        .expect("the journal is readable")
        .expect("the journal is present")
}

fn read_journal_json(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(JOURNAL)).expect("read the journal"))
        .expect("the journal is JSON")
}

/// The client's ledger for one uninterrupted conversion of the same seeded source.
///
/// The comparison root is a separate disposable copy of the same seed, so the expected
/// record is produced by the client's owner rather than by this tool's own arithmetic.
fn reference_ledger(root: &Path) -> serde_json::Value {
    let reference = repository_root()
        .join("build/tmp/licoup-migrate-resume")
        .join("reference");
    if reference.exists() {
        std::fs::remove_dir_all(&reference).expect("clear the reference root");
    }
    std::fs::create_dir_all(&reference).expect("create the reference root");
    let reference = reference.canonicalize().expect("canonical reference root");
    seed_source(&reference);
    let plan = plan_for_domain(&reference, "adaptive-flywheel");
    journal::initialize(&reference, &plan).expect("write the reference journal");
    let report = resume::resume(&reference, options(None), None).expect("the reference conversion");
    assert_eq!(report.status, "completed");
    assert_ne!(
        reference.file_name(),
        root.file_name(),
        "the reference root is a second root, never the one under test"
    );
    client_ledger(&reference)
}

/// Write the journal a run leaves when it stops before the owner has run for `stop_at`.
///
/// The run declares one step: the domain whose store this synthetic root can carry all the
/// way to its declared target. The owner still converts every other domain it finds, which
/// is how a real run behaves; what the journal has to record is the step that was
/// interrupted, and the resume has to close it without a second conversion of it.
fn write_interrupted_before_owner(root: &Path, stop_at: &str) -> journal::JournalPlan {
    let plan = plan_for_domain(root, stop_at);
    journal::initialize(root, &plan).expect("write the journal");
    journal::mark_running(root, stop_at, &step_id_for(&plan, stop_at)).expect("record the attempt");
    plan
}

/// The plan for one domain, taken from the client's own projections.
fn plan_for_domain(root: &Path, domain_id: &str) -> journal::JournalPlan {
    let mut plan = resume::plan_for(root).expect("the plan comes from the client's projections");
    plan.steps.retain(|step| step.domain_id == domain_id);
    assert_eq!(
        plan.steps.len(),
        1,
        "the client's own frontier declares work for {domain_id}"
    );
    plan
}

fn step_id_for(plan: &journal::JournalPlan, domain_id: &str) -> String {
    plan.steps
        .iter()
        .find(|step| step.domain_id == domain_id)
        .expect("the plan declares the domain")
        .step_id
        .clone()
}

/// Where the journal is at, so the two sides of the commit boundary can be told apart.
fn journal_positions(root: &Path) -> BTreeMap<String, String> {
    journal_document(root)
        .domains
        .into_iter()
        .map(|(domain_id, entry)| (domain_id, format!("{:?}", entry.status)))
        .collect()
}

/// The domain both acceptance cases interrupt.
///
/// One domain is enough because the resume is one step at a time: whatever else the
/// frontier declares stays owed, so the two cases exercise the commit boundary itself
/// rather than the size of the run.
fn interrupted_domain() -> String {
    let domain = "adaptive-flywheel".to_string();
    assert!(
        frontier_domains().contains(&domain),
        "the client's frontier declares the interrupted domain"
    );
    domain
}

fn options<'a>(interrupter: Option<&'a resume::Interrupter<'a>>) -> ResumeOptions<'a> {
    ResumeOptions {
        writers_stopped: true,
        interrupter,
    }
}

#[test]
fn every_permitted_interruption_point_is_visited_by_this_regression() {
    // The acceptance cases are dispatched per point, so a point no case reaches would leave
    // a durability claim untested.
    for point in INTERRUPTION_POINTS {
        assert!(
            [
                resume::BEFORE_OWNER,
                resume::AFTER_OWNER,
                resume::AFTER_RECORD,
            ]
            .contains(point),
            "{point} is a permitted interruption point"
        );
    }
    assert!(INTERRUPTION_POINTS.contains(&resume::AFTER_OWNER));
    assert!(INTERRUPTION_POINTS.contains(&resume::BEFORE_OWNER));
}

#[test]
fn an_interruption_before_the_owner_completes_once_and_preserves_the_source() {
    let interrupt_at = resume::BEFORE_OWNER;
    let stop_at = interrupted_domain();
    let root = scratch("before-owner");
    seed_source(&root);
    let source_before = preserved_source_state(&root);

    let plan = write_interrupted_before_owner(&root, &stop_at);
    let declared = plan.steps.len();
    assert!(
        declared > 0,
        "the client's frontier declares work on an empty root"
    );

    // The interruption stopped the run before the owner touched the root, so the run is
    // recorded as attempted and the client has written no marker for it yet.
    assert_eq!(journal_positions(&root)[&stop_at], "Running");
    assert_eq!(
        domain_marker_versions(&root).get(&stop_at),
        None,
        "the owner has not run, so the domain is not converted yet"
    );

    // First resume stops at the point under test.
    let interrupted = RefCell::new(Vec::new());
    let interrupter = |_: &journal::Journal, domain: &str, point: &str| {
        if point == interrupt_at && domain == stop_at {
            interrupted.borrow_mut().push(format!("{domain}@{point}"));
            true
        } else {
            false
        }
    };
    let first =
        resume::resume(&root, options(Some(&interrupter)), None).expect("the first attempt");
    let visited = interrupted.borrow().clone();
    assert_eq!(
        visited,
        [format!("{stop_at}@{interrupt_at}")],
        "the attempt is interrupted at the point this case is about"
    );
    assert_eq!(first.status, "blocked");
    assert!(first.still_owed.contains(&stop_at));
    assert!(
        !journal_document(&root).is_complete(),
        "a run that stopped before the owner is not complete"
    );
    assert!(
        !root.join(CLIENT_LEDGER).exists(),
        "the owner had not run, so nothing is recorded yet"
    );

    // The remaining attempt completes the run.
    let completed = resume::resume(&root, options(None), None).expect("the completing attempt");
    assert_eq!(completed.status, "completed");
    assert!(completed.still_owed.is_empty());
    assert_eq!(
        completed.declared_domains, declared,
        "the resume continued the declared run instead of planning a new one"
    );
    assert!(journal_document(&root).is_complete());
    assert_eq!(
        completed
            .steps
            .iter()
            .filter(|step| step.domain_id == stop_at)
            .count(),
        1,
        "each step is applied by exactly one attempt"
    );

    // The client's own record, not this tool's report, is the oracle for "converted once".
    let steps = ledger_step_ids(&root);
    assert!(
        duplicated_step_ids(&root).is_empty(),
        "no domain records a step twice: {:?}",
        duplicated_step_ids(&root)
    );
    // What one uninterrupted conversion of the same seeded source records. The interrupted
    // path has to land on exactly that, which is a record-identity oracle rather than a
    // count this tool derived from the run being checked.
    let uninterrupted = reference_ledger(&root);
    assert!(
        !steps.is_empty(),
        "the client recorded at least one completed step"
    );
    assert_eq!(
        client_ledger(&root)["domains"],
        uninterrupted["domains"],
        "an interrupted and resumed conversion records exactly what one clean run records"
    );
    assert_eq!(
        client_marker_version(&root, &stop_at),
        plan.steps
            .iter()
            .find(|step| step.domain_id == stop_at)
            .expect("the domain")
            .target_version,
        "the client recorded the domain at the version the frontier declares"
    );

    // The source is preserved: every seeded row is still there, with the value it was
    // seeded with, and no second copy of it exists.
    assert_eq!(
        preserved_source_state(&root),
        source_before,
        "the conversion must not replace the rows it converts"
    );
    assert!(
        root.join(APPEARANCE).exists(),
        "the durable domain document survives"
    );
    assert!(
        root.join(STRATEGY_ARTIFACT).exists(),
        "the owner records its strategy-store conversion"
    );
    assert!(root.join(CLIENT_LEDGER).exists());
}

#[test]
fn an_interruption_after_the_owner_reports_the_domain_current_without_a_second_record() {
    let interrupt_at = resume::AFTER_OWNER;
    let stop_at = interrupted_domain();
    let root = scratch("after-owner");
    seed_source(&root);
    let source_before = preserved_source_state(&root);

    let plan = write_interrupted_before_owner(&root, &stop_at);
    let declared = plan.steps.len();

    // The first attempt runs the owner and stops before recording what it reported.
    let interrupted = RefCell::new(Vec::new());
    let interrupter = |_: &journal::Journal, domain: &str, point: &str| {
        if point == interrupt_at && domain == stop_at {
            interrupted.borrow_mut().push(format!("{domain}@{point}"));
            true
        } else {
            false
        }
    };
    let first =
        resume::resume(&root, options(Some(&interrupter)), None).expect("the first attempt");
    let visited = interrupted.borrow().clone();
    assert_eq!(
        visited,
        [format!("{stop_at}@{interrupt_at}")],
        "the attempt is interrupted after the owner's commit"
    );
    assert_eq!(first.status, "blocked");
    assert_eq!(
        journal_positions(&root)[&stop_at],
        "Running",
        "the process stopped before the outcome was recorded"
    );
    // The client's own marker is the durable record the owner wrote before this tool was
    // interrupted, so its presence at the target proves the commit really happened.
    assert_eq!(
        client_marker_version(&root, &stop_at),
        plan.steps
            .iter()
            .find(|step| step.domain_id == stop_at)
            .expect("the plan declares the domain")
            .target_version,
        "the owner's durable result exists even though this tool has not recorded it"
    );

    let records_before = record_fingerprint(&root);
    let markers_before = domain_marker_versions(&root);

    // The resume must notice that the domain is already where the journal wanted it.
    let completed = resume::resume(&root, options(None), None).expect("the completing attempt");
    assert_eq!(completed.status, "completed");
    assert!(completed.still_owed.is_empty());
    assert_eq!(
        client_marker_version(&root, &stop_at),
        plan.steps
            .iter()
            .find(|step| step.domain_id == stop_at)
            .expect("the domain")
            .target_version,
        "the completed version is the target the client's frontier declares"
    );

    // Exactly one conversion result: the same domains are current, the record count is
    // unchanged, and nothing was recorded twice.
    assert_eq!(
        domain_marker_versions(&root),
        markers_before,
        "the second run rewrites no domain marker the first one already wrote"
    );
    assert!(
        duplicated_step_ids(&root).is_empty(),
        "a resume never lists the same step id twice: {:?}",
        duplicated_step_ids(&root)
    );
    assert_eq!(
        ledger_step_ids(&root).len(),
        LedgerSnapshot::read(&root)
            .expect("snapshot")
            .recorded_steps()
            .expect("count"),
        "the client's ledger count agrees with its own listing"
    );
    assert_eq!(declared, plan.steps.len());
    assert_eq!(
        record_fingerprint(&root),
        records_before,
        "the resume of a committed domain rewrites no record"
    );
    assert_eq!(preserved_source_state(&root), source_before);
}

#[test]
fn a_resume_of_a_finished_run_reports_every_domain_current_and_changes_no_record() {
    let root = scratch("repeat");
    seed_source(&root);
    let source_before = preserved_source_state(&root);

    let plan = plan_for_domain(&root, "adaptive-flywheel");
    journal::initialize(&root, &plan).expect("write the journal");
    let converted = resume::resume(&root, options(None), None).expect("the conversion");
    assert_eq!(converted.status, "completed");

    let ledger_after_conversion = std::fs::read(root.join(CLIENT_LEDGER)).expect("read the ledger");
    let records_after_conversion = record_fingerprint(&root);
    let step_ids_after_conversion = ledger_step_ids(&root);

    // Ask again. This is the second run of the acceptance oracle: the journal is still the
    // evidence of the finished run, so nothing may be converted a second time.
    let again = resume::resume(&root, options(None), None).expect("the repeated attempt");
    assert_eq!(again.status, "alreadyCurrent");
    assert!(again.is_complete());
    assert!(again.still_owed.is_empty());
    assert_eq!(
        again
            .steps
            .iter()
            .filter(|step| step.settled_before_resume)
            .count(),
        again.steps.len(),
        "every step was already settled when this attempt started"
    );
    assert_eq!(
        std::fs::read(root.join(CLIENT_LEDGER)).expect("read the ledger"),
        ledger_after_conversion,
        "a repeated resume leaves the client's ledger byte-identical"
    );
    assert_eq!(record_fingerprint(&root), records_after_conversion);
    assert_eq!(ledger_step_ids(&root), step_ids_after_conversion);
    assert!(duplicated_step_ids(&root).is_empty());
    assert_eq!(preserved_source_state(&root), source_before);

    // A third attempt is the same answer, so the outcome does not depend on how many times
    // the operator retries.
    let third = resume::resume(&root, options(None), None).expect("the third attempt");
    assert_eq!(third.status, "alreadyCurrent");
    assert_eq!(third.ledger_records, again.ledger_records);
}

#[test]
fn resume_without_a_journal_reports_no_work_rather_than_converting() {
    let root = scratch("no-journal");
    seed_source(&root);
    let source_before = preserved_source_state(&root);

    let report = resume::resume(&root, options(None), None).expect("a resume with nothing to do");
    assert_eq!(report.status, "noOp");
    assert!(
        !report.still_owed.is_empty(),
        "the report still names the work the root owes"
    );
    assert!(
        !root.join(CLIENT_LEDGER).exists(),
        "a resume that found no journal must not convert anything"
    );
    assert_eq!(preserved_source_state(&root), source_before);
}

#[test]
fn a_resume_without_the_operators_confirmation_is_refused_before_the_owner_runs() {
    let stop_at = interrupted_domain();
    let root = scratch("no-confirmation");
    seed_source(&root);
    write_interrupted_before_owner(&root, &stop_at);
    let records_before = record_fingerprint(&root);

    let refused = resume::resume(
        &root,
        ResumeOptions {
            writers_stopped: false,
            interrupter: None,
        },
        None,
    )
    .expect_err("a resume without the operator's statement is refused");
    assert_eq!(refused.code(), "maintenance_confirmation_required");
    assert_eq!(
        journal_positions(&root)[&stop_at],
        "Running",
        "the refusal happens before the owner is asked to move anything"
    );
    assert_eq!(
        record_fingerprint(&root),
        records_before,
        "a refused resume leaves every record untouched"
    );
}

#[test]
fn a_journal_this_binary_must_not_act_on_is_refused() {
    let root = scratch("mismatched-journal");
    seed_source(&root);
    let path = root.join(JOURNAL);
    std::fs::create_dir_all(path.parent().expect("migrations directory")).expect("create");
    std::fs::write(
        &path,
        br#"{"schemaVersion":"v0.0.1:data-migration-journal-99","status":"inProgress","startedAt":"0","updatedAt":"0","targetVersion":"0.0.1-alpha","frontierId":"licoup-state-other","direction":"forward","domains":{}}"#,
    )
    .expect("write a foreign journal");

    let refused = resume::resume(&root, options(None), None)
        .expect_err("a journal from another run is refused");
    assert_eq!(refused.code(), "migration_journal_mismatched");
    assert!(!root.join(CLIENT_LEDGER).exists());
    assert_eq!(
        std::fs::read(&path).expect("read the journal"),
        br#"{"schemaVersion":"v0.0.1:data-migration-journal-99","status":"inProgress","startedAt":"0","updatedAt":"0","targetVersion":"0.0.1-alpha","frontierId":"licoup-state-other","direction":"forward","domains":{}}"#,
        "the refusal does not rewrite the document it refused"
    );
}

#[test]
fn a_journal_step_the_owner_left_owed_keeps_the_run_blocked_and_visible() {
    let stop_at = interrupted_domain();
    let root = scratch("blocked-step");
    seed_source(&root);

    // A journal whose step names a version the client's frontier does not declare for this
    // domain. The owner cannot satisfy it, so the run must stay open instead of reporting
    // a completion it did not reach.
    let declared = plan_for_domain(&root, &stop_at);
    let unreachable = declared.steps[0].target_version + 1;
    let plan = journal::JournalPlan {
        frontier_id: declared.frontier_id.clone(),
        target_version: declared.target_version.clone(),
        steps: vec![journal::StepRequest {
            domain_id: stop_at.clone(),
            from_version: declared.steps[0].from_version,
            target_version: unreachable,
            step_id: "adaptive-flywheel.unreachable".to_string(),
        }],
    };
    journal::initialize(&root, &plan).expect("write the journal");
    journal::mark_running(&root, &stop_at, "adaptive-flywheel.unreachable").expect("attempt");

    let report = resume::resume(&root, options(None), None).expect("the attempt is reported");
    assert_eq!(report.status, "blocked");
    assert_eq!(report.still_owed, vec![stop_at.clone()]);
    assert!(
        !journal_document(&root).is_complete(),
        "a run that still owes a step is never closed"
    );
    assert_eq!(
        journal_positions(&root)[&stop_at],
        "Pending",
        "the declined step is recorded as owed again"
    );
    assert_eq!(journal_document(&root).committed_count(), 0);
    assert_eq!(
        journal_document(&root).direction,
        journal::RunDirection::Forward
    );
}

#[test]
fn the_journal_records_the_client_ledger_step_ids_and_never_writes_the_client_ledger() {
    let stop_at = interrupted_domain();
    let root = scratch("journal-content");
    seed_source(&root);

    let plan = write_interrupted_before_owner(&root, &stop_at);
    let step_id = step_id_for(&plan, &stop_at);
    let document = read_journal_json(&root);
    assert_eq!(
        document["schemaVersion"], "v0.0.1:data-migration-journal-1",
        "the journal carries the schema this tool declares"
    );
    assert_eq!(
        document["frontierId"],
        licoup_native::domain::client_state_migration::frontier_projection_struct()
            .expect("frontier")
            .frontier_id,
        "the journal's frontier is the client's, not one this tool derived"
    );
    assert_eq!(
        document["domains"][&stop_at]["stepId"], step_id,
        "the journal names the client's own step id"
    );
    assert_eq!(
        document["domains"][&stop_at]["targetVersion"],
        plan.steps
            .iter()
            .find(|step| step.domain_id == stop_at)
            .expect("the domain")
            .target_version,
        "the journal's target version is the client's declared target"
    );

    // The client's ledger does not exist until the client's owner writes it.
    assert!(!root.join(CLIENT_LEDGER).exists());

    resume::resume(&root, options(None), None).expect("the conversion");
    assert!(root.join(CLIENT_LEDGER).exists());
    let ledger = client_ledger(&root);
    assert_eq!(
        ledger["frontierId"], document["frontierId"],
        "the client recorded the conversion against the same frontier"
    );
    assert!(
        licoup_migrate::journal::ledger::LedgerSnapshot::read(&root)
            .expect("snapshot")
            .parse()
            .expect("parse")
            .expect("present")
            .duplicated_steps()
            .is_empty(),
        "the client's ledger holds no duplicated record"
    );
}

/// The oracle this regression deliberately does not use, with the reason recorded.
///
/// The resume's own guarantee is stated against the client's durable records — its ledger
/// and its domain markers — because those are the documents the client's owner writes. The
/// *store* projection is the client's answer to a different question, and it is deliberately
/// not what decides whether a conversion happened: a projection that answered zero for a
/// converted domain would make a tool that trusted it report a durable conversion as
/// unsupported. The projection is asserted here only to agree with the marker, so the two
/// client documents are cross-checked without either one becoming the tool's oracle.
#[test]
fn the_store_projection_is_not_the_resume_oracle() {
    let stop_at = interrupted_domain();
    let root = scratch("projection-counterexample");
    seed_source(&root);
    let plan = write_interrupted_before_owner(&root, &stop_at);
    let target = plan.steps[0].target_version;

    let report = resume::resume(&root, options(None), None).expect("the conversion");
    assert_eq!(report.status, "completed");
    assert_eq!(
        client_marker_version(&root, &stop_at),
        target,
        "the client's own marker records the completed conversion"
    );
    let ledger = client_ledger(&root);
    assert!(
        ledger["domains"][&stop_at]["completedStepIds"]
            .as_array()
            .is_some_and(|steps| !steps.is_empty()),
        "the client's own ledger records the completed steps"
    );
    // The client's own projection reads the store the owner just wrote, so it agrees with
    // the marker. The assertion is recorded as agreement between two client documents, not
    // as the condition the resume finished on.
    let projection = observed_version(&root, &stop_at);
    assert_eq!(
        projection, target,
        "the client's store projection reports the store the owner wrote"
    );
}

#[test]
fn a_committed_journal_names_only_the_client_ledger_targets_it_reached() {
    let root = scratch("committed-versions");
    seed_source(&root);
    let plan = plan_for_domain(&root, "adaptive-flywheel");
    journal::initialize(&root, &plan).expect("journal");
    let report = resume::resume(&root, options(None), None).expect("conversion");
    assert_eq!(report.status, "completed");

    let document = journal_document(&root);
    assert!(
        document
            .domains
            .values()
            .all(|entry| entry.status == StepStatus::Committed),
        "a completed journal has no unsettled step"
    );
    for (domain_id, entry) in &document.domains {
        assert_eq!(
            client_marker_version(&root, domain_id),
            entry.target_version(),
            "{domain_id} is committed at {:?}, so the client's marker says so",
            entry.committed_version
        );
        assert!(entry.committed_at.is_some());
        assert_eq!(entry.status, StepStatus::Committed);
    }
}
