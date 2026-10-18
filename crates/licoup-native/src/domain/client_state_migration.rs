//! Forward-only admission for the shared client data root.
//!
//! The frontier and running identity are compiled into the binary. Callers
//! supply only the raw data root and cannot select migration code or targets.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

use crate::platform::llm_api_key_vault::{
    LegacyCredentialMigrationDisposition, PlatformLlmApiKeyVault,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use fs2::FileExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;

mod appearance;
mod handoff;
mod stores;
mod strategy_store;

pub(crate) use handoff::prepare_update_handoff;
#[cfg(test)]
use handoff::{
    UPDATE_HANDOFF_SCHEMA, UpdateHandoff, pre_claim_backup_path, update_handoff_rejection_path,
};
use handoff::{claim_update_handoff, update_handoff_is_claimed, write_update_handoff_rejection};
#[cfg(test)]
use stores::{apply_authoritative_store, probe_canonical_conversation};
use stores::{
    apply_marker_step, load_domain_marker, migration_handler_target, probe_authority, probe_domain,
    reconcile_current_marker, upgrade_canonical_conversation_schema,
};

use crate::state_machines::update_handoff;

const MAX_MIGRATION_JSON_BYTES: usize = 4 * 1024 * 1024;
const FRONTIER_SCHEMA: &str = "v0.0.1:client-state-migration-frontier-1";
const LEDGER_SCHEMA: &str = "v0.0.1:client-state-migration-ledger-1";
const DOMAIN_MARKER_SCHEMA: &str = "v0.0.1:client-state-domain-marker-1";
const FRONTIER_JSON: &str = include_str!("../../resources/client-state-migration-frontier.json");
const GATEWAY_CUSTODY_DOMAIN: &str = "gateway-credential-custody";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseTrack {
    Nightly,
    Stable,
}

impl ReleaseTrack {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nightly => "nightly",
            Self::Stable => "stable",
        }
    }

    pub fn running() -> Result<Self> {
        match option_env!("LICO_CLIENT_RELEASE_TRACK").unwrap_or("nightly") {
            "nightly" => Ok(Self::Nightly),
            "stable" => Ok(Self::Stable),
            _ => bail!("embedded client release track is invalid"),
        }
    }
}

pub fn running_product_version() -> Result<&'static str> {
    let value = option_env!("LICO_CLIENT_PRODUCT_VERSION").unwrap_or("0.0.1-alpha");
    Version::parse(value).context("embedded client product version is invalid")?;
    Ok(value)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationFrontier {
    schema_version: String,
    /// The last published format this binary converts from. The catalog fixes it:
    /// a root that names any other source is not an input this binary supports.
    pub source_frontier_id: String,
    /// This binary's own target format, and the destination of every conversion.
    pub frontier_id: String,
    pub domains: Vec<DomainFrontier>,
}

/// The two conversion endpoints the catalog declares: the last published format
/// as source and this binary's own target format as destination. There is no
/// third endpoint, so a name outside this pair has no conversion path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionEndpoints {
    pub source_frontier_id: String,
    pub target_frontier_id: String,
}

impl MigrationFrontier {
    /// The declared endpoints in one value, so a reader enumerates them instead
    /// of restating a format name.
    pub fn conversion_endpoints(&self) -> ConversionEndpoints {
        ConversionEndpoints {
            source_frontier_id: self.source_frontier_id.clone(),
            target_frontier_id: self.frontier_id.clone(),
        }
    }

    /// Refuses a root that names a format this catalog does not declare. A root
    /// at the source is converted and a root already at this binary's own target
    /// is a rerun; every other name — an older published format, or a format no
    /// release ever left — is refused as an unsupported source before any
    /// mutation, with the same stable code an unsupported shape gets.
    fn require_declared_format(&self, named_frontier_id: &str) -> Result<()> {
        ensure!(
            named_frontier_id == self.source_frontier_id || named_frontier_id == self.frontier_id,
            "unsupported_state_shape"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainFrontier {
    pub domain_id: String,
    durability: Durability,
    startup_scope: StartupScope,
    pub target_schema_version: u32,
    pub steps: Vec<MigrationEdge>,
}

/// Persistence and startup necessity are independent: optional feature state
/// remains durable even when the current client cannot interpret it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum StartupScope {
    Core,
    Feature,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Durability {
    Durable,
    Derived,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationEdge {
    pub step_id: String,
    pub from_schema_version: u32,
    pub to_schema_version: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    schema_version: String,
    highest_admitted_product_version: String,
    frontier_id: String,
    domains: BTreeMap<String, LedgerDomain>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LedgerDomain {
    schema_version: u32,
    completed_step_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DomainMarker {
    schema_version: String,
    domain_id: String,
    authoritative_schema_version: u32,
}

pub(crate) struct PreparedUpdateHandoff {
    pub handoff_path: PathBuf,
    pub backup_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionResult {
    pub status: &'static str,
    pub running_product_version: String,
    pub running_release_track: &'static str,
    pub frontier_id: String,
    pub applied_domain_ids: Vec<String>,
    pub skipped_domain_ids: Vec<String>,
    pub pending_authorization_domain_ids: Vec<String>,
    pub unavailable_feature_domain_ids: Vec<String>,
}

struct PlannedStep<'a> {
    domain: &'a DomainFrontier,
    edge: &'a MigrationEdge,
}

#[derive(Clone, Copy, Debug)]
struct AuthoritativeProbe {
    version: u32,
    present: bool,
}

/// Admits the root through the immutable embedded frontier. Error text is
/// intentionally a stable privacy-safe code; paths and stored values never
/// cross this boundary.
pub fn admit(data_root: &Path) -> Result<AdmissionResult> {
    running_product_version()
        .and_then(|running_version| admit_inner(data_root, running_version))
        .map_err(|error| anyhow!(safe_error_code(&error)))
}

/// Admission under an explicit running identity, for tests that must run a
/// faithful released-source root under the identity the caller names instead
/// of lowering the root's recorded high-water to the development fallback. The
/// mapping to stable codes is the same one `admit` applies.
#[cfg(test)]
fn admit_as_version(data_root: &Path, running_version: &str) -> Result<AdmissionResult> {
    admit_inner(data_root, running_version).map_err(|error| anyhow!(safe_error_code(&error)))
}

fn admit_inner(data_root: &Path, running_version: &str) -> Result<AdmissionResult> {
    admit_with_credential_migration_disposition(
        data_root,
        running_version,
        PlatformLlmApiKeyVault::legacy_credential_migration_disposition()?,
    )
}

fn admit_with_credential_migration_disposition(
    data_root: &Path,
    running_version: &str,
    custody: LegacyCredentialMigrationDisposition,
) -> Result<AdmissionResult> {
    ensure!(data_root.is_absolute(), "unsupported_state_shape");
    fs::create_dir_all(data_root).context("migration_lock_unavailable")?;
    licoup_foundation::platform::file_security::ensure_private_dir(&data_root.join("client-state"))
        .context("migration_lock_unavailable")?;
    let migration_root = data_root.join("client-state").join("migrations");
    licoup_foundation::platform::file_security::ensure_private_dir(&migration_root)
        .context("migration_lock_unavailable")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(migration_root.join("admission.lock"))
        .context("migration_lock_unavailable")?;
    lock.lock_exclusive()
        .context("migration_lock_unavailable")?;

    let frontier = embedded_frontier()?;
    let handoff_path = migration_root.join("update-handoff.json");
    let handoff_exists = fs::symlink_metadata(&handoff_path).is_ok();
    if let Err(error) = claim_update_handoff(&handoff_path, &frontier) {
        // A claimed handoff is the forward-only ownership boundary. Cleanup
        // failures after that durable write must never tell the installer to
        // restore the old application.
        if handoff_exists && !update_handoff_is_claimed(&handoff_path) {
            write_update_handoff_rejection(&handoff_path)?;
        }
        return Err(error);
    }
    let ledger_path = migration_root.join("ledger.json");
    let mut ledger = load_ledger(&ledger_path, &frontier)?;
    reject_older_binary(&ledger, running_version)?;
    // The root names the format it was last admitted at. Only the declared
    // source is converted and only this binary's own target is a rerun, so a
    // third name is refused here — before the high-water advances or any domain
    // moves — instead of being converted under an undeclared endpoint.
    frontier.require_declared_format(&ledger.frontier_id)?;

    // Probe every authoritative marker and construct every exact path before
    // persisting high-water or changing a domain.
    let marker_root = migration_root.join("domain-state");
    let mut observed = BTreeMap::new();
    let mut plan = Vec::new();
    let mut skipped = Vec::new();
    let mut pending_authorization = Vec::new();
    let mut unavailable_features = BTreeSet::new();
    for domain in &frontier.domains {
        let Some(mut version) = startup_domain_result(
            probe_domain(&marker_root, domain),
            domain,
            &mut unavailable_features,
        )?
        else {
            continue;
        };
        // A data root alone cannot prove that this account has no legacy
        // Keychain items. Only the explicit protected operation can complete
        // this domain; startup never reads secrets or opens a native dialog.
        if domain.domain_id == GATEWAY_CUSTODY_DOMAIN && version == 0 {
            if custody == LegacyCredentialMigrationDisposition::RequiresAuthorization {
                observed.insert(domain.domain_id.clone(), version);
                pending_authorization.push(domain.domain_id.clone());
                continue;
            }
            version = domain.target_schema_version;
        }
        observed.insert(domain.domain_id.clone(), version);
        if version == domain.target_schema_version {
            skipped.push(domain.domain_id.clone());
            continue;
        }
        let mut cursor = version;
        while cursor < domain.target_schema_version {
            let matches = domain
                .steps
                .iter()
                .filter(|edge| edge.from_schema_version == cursor)
                .collect::<Vec<_>>();
            ensure!(matches.len() == 1, "migration_frontier_incomplete");
            let edge = matches[0];
            ensure!(
                edge.to_schema_version > cursor
                    && edge.to_schema_version <= domain.target_schema_version,
                "migration_frontier_incomplete"
            );
            plan.push(PlannedStep { domain, edge });
            cursor = edge.to_schema_version;
        }
        ensure!(
            cursor == domain.target_schema_version,
            "migration_frontier_incomplete"
        );
    }
    validate_ledger_reconciliation(&ledger, &frontier, &observed)?;
    // An authoritative store may have committed before the process crashed
    // while writing its ledger entry. Rebuild the completed prefix from the
    // store probe before admitting the next edge, otherwise a later edge can
    // be recorded without the earlier one and make the next admission fail.
    reconcile_authoritative_ledger_prefix(&mut ledger, &frontier, &observed);

    // Once persisted, an older binary is permanently denied even if a later
    // domain step fails. Recovery is same/newer forward repair only.
    ledger.highest_admitted_product_version = running_version.to_owned();
    ledger.frontier_id = frontier.frontier_id.clone();
    write_json_atomic(&ledger_path, &ledger).context("migration_ledger_invalid")?;

    licoup_foundation::platform::file_security::ensure_private_dir(&marker_root)
        .context("migration_step_failed")?;
    let mut reconciled_current_domain = false;
    for domain in &frontier.domains {
        if observed.get(&domain.domain_id) != Some(&domain.target_schema_version) {
            continue;
        }
        if domain.domain_id == "canonical-conversation" {
            upgrade_canonical_conversation_schema(data_root)?;
        }
        if domain.domain_id == strategy_store::STRATEGY_STORE_DOMAIN {
            // The store is already at its target layout, so this converts
            // nothing: it reconciles the recovery record of a conversion whose
            // physical layout committed before its bookkeeping did. Without
            // this hook a completed store would keep a pending record forever.
            strategy_store::reconcile_completed_strategy_store(
                data_root,
                strategy_store::strategy_format_for_domain_version(domain.target_schema_version)?,
            )?;
        }
        if startup_domain_result(
            reconcile_current_marker(&marker_root, domain),
            domain,
            &mut unavailable_features,
        )?
        .is_none()
        {
            continue;
        }
        for edge in &domain.steps {
            reconciled_current_domain |= reconcile_ledger(&mut ledger, domain, edge);
        }
    }
    if reconciled_current_domain {
        write_json_atomic(&ledger_path, &ledger).context("migration_ledger_invalid")?;
    }
    let mut applied = BTreeSet::new();
    for item in plan {
        if unavailable_features.contains(&item.domain.domain_id) {
            continue;
        }
        let authoritative = observed
            .get_mut(&item.domain.domain_id)
            .ok_or_else(|| anyhow!("migration_step_failed"))?;
        if *authoritative == item.edge.to_schema_version {
            reconcile_ledger(&mut ledger, item.domain, item.edge);
            write_json_atomic(&ledger_path, &ledger).context("migration_ledger_invalid")?;
            continue;
        }
        ensure!(
            *authoritative == item.edge.from_schema_version,
            "migration_step_failed"
        );
        migration_failpoint("before-store")?;
        if startup_domain_result(
            apply_marker_step(&marker_root, item.domain, item.edge),
            item.domain,
            &mut unavailable_features,
        )?
        .is_none()
        {
            continue;
        }
        *authoritative = item.edge.to_schema_version;
        let postcondition = probe_domain(&marker_root, item.domain).and_then(|version| {
            ensure!(version == *authoritative, "migration_postcondition_failed");
            Ok(())
        });
        if startup_domain_result(postcondition, item.domain, &mut unavailable_features)?.is_none() {
            continue;
        }
        migration_failpoint("after-store")?;
        reconcile_ledger(&mut ledger, item.domain, item.edge);
        write_json_atomic(&ledger_path, &ledger).context("migration_ledger_invalid")?;
        migration_failpoint("after-ledger")?;
        applied.insert(item.domain.domain_id.clone());
    }
    licoup_foundation::platform::file_security::remove_private_state_marker(&handoff_path)
        .context("update_handoff_mismatch")?;
    Ok(AdmissionResult {
        status: "ready",
        running_product_version: running_version.to_owned(),
        running_release_track: ReleaseTrack::running()?.as_str(),
        frontier_id: frontier.frontier_id,
        applied_domain_ids: applied.into_iter().collect(),
        skipped_domain_ids: skipped,
        pending_authorization_domain_ids: pending_authorization,
        unavailable_feature_domain_ids: unavailable_features.into_iter().collect(),
    })
}

// Optional state is never replaced by a default document during admission.
// Its owner may use an in-memory default or disable that feature; diagnostics
// continue to report the unreadable document instead of certifying it current.
fn startup_domain_result<T>(
    result: Result<T>,
    domain: &DomainFrontier,
    unavailable: &mut BTreeSet<String>,
) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(_) if domain.startup_scope == StartupScope::Feature => {
            unavailable.insert(domain.domain_id.clone());
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Metadata-only projection of the deferred upgrade. The completion marker
/// is written only after the vault confirms every copied item and cleanup.
pub fn gateway_credential_migration_pending(root: &Path) -> Result<bool> {
    let frontier = embedded_frontier()?;
    let domain = frontier
        .domains
        .iter()
        .find(|domain| domain.domain_id == GATEWAY_CUSTODY_DOMAIN)
        .ok_or_else(|| anyhow!("migration_frontier_incomplete"))?;
    let marker_root = root.join("client-state/migrations/domain-state");
    Ok(
        PlatformLlmApiKeyVault::legacy_credential_migration_disposition()?
            == LegacyCredentialMigrationDisposition::RequiresAuthorization
            && probe_domain(&marker_root, domain)? < domain.target_schema_version,
    )
}

/// Explicit protected continuation of the embedded migration frontier.
/// The long native prompt holds only the custody lock, so normal admission
/// and unrelated client state remain available while the user responds.
pub fn migrate_gateway_credentials(
    root: &Path,
) -> Result<licoup_vault::api_key::LlmApiKeyInventory> {
    migrate_gateway_credentials_with(root, || {
        PlatformLlmApiKeyVault::at_state_root(root)?.migrate_legacy_credentials()
    })
}

fn migrate_gateway_credentials_with(
    root: &Path,
    migrate: impl FnOnce() -> Result<licoup_vault::api_key::LlmApiKeyInventory>,
) -> Result<licoup_vault::api_key::LlmApiKeyInventory> {
    admit(root)?;
    let migration_root = root.join("client-state/migrations");
    let custody_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(migration_root.join("gateway-credential-custody.lock"))
        .context("migration_lock_unavailable")?;
    custody_lock
        .lock_exclusive()
        .context("migration_lock_unavailable")?;
    if !gateway_credential_migration_pending(root)? {
        return PlatformLlmApiKeyVault::at_state_root(root)?.list();
    }
    let inventory = migrate()?;
    complete_gateway_custody_migration(root)?;
    Ok(inventory)
}

fn complete_gateway_custody_migration(root: &Path) -> Result<()> {
    let migration_root = root.join("client-state/migrations");
    let admission_lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(migration_root.join("admission.lock"))
        .context("migration_lock_unavailable")?;
    admission_lock
        .lock_exclusive()
        .context("migration_lock_unavailable")?;
    let frontier = embedded_frontier()?;
    let domain = frontier
        .domains
        .iter()
        .find(|domain| domain.domain_id == GATEWAY_CUSTODY_DOMAIN)
        .ok_or_else(|| anyhow!("migration_frontier_incomplete"))?;
    let ledger_path = migration_root.join("ledger.json");
    let mut ledger = load_ledger(&ledger_path, &frontier)?;
    reject_older_binary(&ledger, running_product_version()?)?;
    reconcile_current_marker(&migration_root.join("domain-state"), domain)?;
    for edge in &domain.steps {
        reconcile_ledger(&mut ledger, domain, edge);
    }
    write_json_atomic(&ledger_path, &ledger).context("migration_ledger_invalid")
}

fn validate_ledger_reconciliation(
    ledger: &Ledger,
    frontier: &MigrationFrontier,
    observed: &BTreeMap<String, u32>,
) -> Result<()> {
    for (domain_id, entry) in &ledger.domains {
        let domain = frontier
            .domains
            .iter()
            .find(|domain| &domain.domain_id == domain_id)
            .ok_or_else(|| anyhow!("migration_ledger_invalid"))?;
        let Some(authoritative) = observed.get(domain_id).copied() else {
            ensure!(
                domain.startup_scope == StartupScope::Feature,
                "migration_ledger_invalid"
            );
            // Preserve this unavailable feature's ledger entry unchanged.
            continue;
        };
        ensure!(
            entry.schema_version <= authoritative
                && entry.schema_version <= domain.target_schema_version,
            "migration_ledger_invalid"
        );
        let expected = domain
            .steps
            .iter()
            .filter(|step| step.to_schema_version <= entry.schema_version)
            .map(|step| step.step_id.as_str())
            .collect::<Vec<_>>();
        ensure!(
            entry
                .completed_step_ids
                .iter()
                .map(String::as_str)
                .eq(expected),
            "migration_ledger_invalid"
        );
    }
    Ok(())
}

fn reconcile_authoritative_ledger_prefix(
    ledger: &mut Ledger,
    frontier: &MigrationFrontier,
    observed: &BTreeMap<String, u32>,
) {
    for domain in &frontier.domains {
        let Some(&authoritative) = observed.get(&domain.domain_id) else {
            continue;
        };
        for edge in domain
            .steps
            .iter()
            .filter(|edge| edge.to_schema_version <= authoritative)
        {
            reconcile_ledger(ledger, domain, edge);
        }
    }
}

#[cfg(not(test))]
fn migration_failpoint(_name: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    static MIGRATION_FAILPOINT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn migration_failpoint(name: &str) -> Result<()> {
    MIGRATION_FAILPOINT.with(|slot| {
        ensure!(
            slot.borrow().as_ref().copied() != Some(name),
            "migration_step_failed"
        );
        Ok(())
    })
}

#[cfg(test)]
struct MigrationFailpointGuard(Option<&'static str>);

#[cfg(test)]
impl MigrationFailpointGuard {
    fn set(name: &'static str) -> Self {
        let previous = MIGRATION_FAILPOINT.with(|slot| slot.replace(Some(name)));
        Self(previous)
    }
}

#[cfg(test)]
impl Drop for MigrationFailpointGuard {
    fn drop(&mut self) {
        MIGRATION_FAILPOINT.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

pub fn embedded_frontier() -> Result<MigrationFrontier> {
    let frontier: MigrationFrontier =
        serde_json::from_str(FRONTIER_JSON).context("migration_frontier_incomplete")?;
    ensure!(
        frontier.schema_version == FRONTIER_SCHEMA,
        "migration_frontier_incomplete"
    );
    ensure!(
        is_frontier_identity(&frontier.source_frontier_id)
            && is_frontier_identity(&frontier.frontier_id)
            && frontier.source_frontier_id != frontier.frontier_id,
        "migration_frontier_incomplete"
    );
    ensure!(
        !frontier.domains.is_empty(),
        "migration_frontier_incomplete"
    );
    let mut domains = BTreeSet::new();
    let mut all_step_ids = BTreeSet::new();
    for domain in &frontier.domains {
        ensure!(
            !domain.domain_id.is_empty()
                && domain
                    .domain_id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && domains.insert(&domain.domain_id),
            "migration_frontier_incomplete"
        );
        ensure!(
            domain.target_schema_version > 0
                && !domain.steps.is_empty()
                && (domain.durability == Durability::Durable || !domain.steps.is_empty()),
            "migration_frontier_incomplete"
        );
        let mut sources = BTreeSet::new();
        let mut cursor = 0;
        for edge in &domain.steps {
            ensure!(
                edge.from_schema_version == cursor
                    && edge.to_schema_version > edge.from_schema_version
                    && edge.to_schema_version <= domain.target_schema_version
                    && sources.insert(edge.from_schema_version)
                    && !edge.step_id.is_empty()
                    && all_step_ids.insert(&edge.step_id)
                    && migration_handler_target(&domain.domain_id, edge.from_schema_version,)
                        == Some(edge.to_schema_version),
                "migration_frontier_incomplete"
            );
            cursor = edge.to_schema_version;
        }
        ensure!(
            cursor == domain.target_schema_version,
            "migration_frontier_incomplete"
        );
    }
    Ok(frontier)
}

fn load_ledger(path: &Path, frontier: &MigrationFrontier) -> Result<Ledger> {
    let Some(raw) = licoup_foundation::platform::file_security::read_existing_private_text_bounded(
        path,
        256 * 1024,
    )
    .context("migration_ledger_invalid")?
    else {
        return Ok(Ledger {
            schema_version: LEDGER_SCHEMA.to_owned(),
            highest_admitted_product_version: "0.0.0".to_owned(),
            frontier_id: frontier.frontier_id.clone(),
            domains: BTreeMap::new(),
        });
    };
    let ledger: Ledger = serde_json::from_str(&raw).context("migration_ledger_invalid")?;
    ensure!(
        ledger.schema_version == LEDGER_SCHEMA,
        "migration_ledger_invalid"
    );
    Version::parse(&ledger.highest_admitted_product_version).context("migration_ledger_invalid")?;
    Ok(ledger)
}

fn reject_older_binary(ledger: &Ledger, running: &str) -> Result<()> {
    let high = Version::parse(&ledger.highest_admitted_product_version)
        .context("migration_ledger_invalid")?;
    let running = Version::parse(running).context("migration_frontier_incomplete")?;
    ensure!(running >= high, "state_newer_than_binary");
    Ok(())
}

fn marker_path(root: &Path, domain_id: &str) -> PathBuf {
    root.join(format!("{domain_id}.json"))
}

fn reconcile_ledger(ledger: &mut Ledger, domain: &DomainFrontier, edge: &MigrationEdge) -> bool {
    let entry = ledger
        .domains
        .entry(domain.domain_id.clone())
        .or_insert_with(|| LedgerDomain {
            schema_version: edge.from_schema_version,
            completed_step_ids: Vec::new(),
        });
    let before = entry.clone();
    entry.schema_version = edge.to_schema_version;
    if !entry.completed_step_ids.contains(&edge.step_id) {
        entry.completed_step_ids.push(edge.step_id.clone());
    }
    let allowed = domain
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect::<BTreeSet<_>>();
    entry
        .completed_step_ids
        .retain(|step| allowed.contains(step.as_str()));
    *entry != before
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut serialized = serde_json::to_string(value)?;
    serialized.push('\n');
    ensure!(
        serialized.len() <= MAX_MIGRATION_JSON_BYTES,
        "migration_step_failed"
    );
    licoup_foundation::platform::file_security::atomic_write_private_text(path, &serialized)
}

fn safe_error_code(error: &anyhow::Error) -> &'static str {
    const CODES: &[&str] = &[
        "migration_lock_unavailable",
        "migration_ledger_invalid",
        "state_newer_than_binary",
        "migration_frontier_incomplete",
        "migration_step_failed",
        "migration_postcondition_failed",
        "update_handoff_mismatch",
        "unsupported_state_shape",
    ];
    for cause in error.chain() {
        if let Some(code) = CODES
            .iter()
            .copied()
            .find(|code| cause.to_string().contains(code))
        {
            return code;
        }
    }
    "migration_step_failed"
}

/// A frontier identity is a lowercase dotted name. The catalog's two endpoints
/// are two such names, and they are distinct: one format cannot be both the
/// source and the destination of the same conversion.
fn is_frontier_identity(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
}

/// The conversion endpoints the embedded catalog declares, for a reader that
/// enumerates the declared pair instead of restating a format name.
pub fn conversion_endpoints() -> Result<ConversionEndpoints> {
    Ok(embedded_frontier()?.conversion_endpoints())
}

/// The authoritative migration frontier in a form the standalone migration tool
/// can read without duplicating the domain catalog.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierProjection {
    pub frontier_id: String,
    pub domains: Vec<FrontierDomainProjection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierDomainProjection {
    pub domain_id: String,
    pub target_schema_version: u32,
    pub steps: Vec<FrontierStepProjection>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierStepProjection {
    pub step_id: String,
    pub from_schema_version: u32,
    pub to_schema_version: u32,
}

/// One domain's observed authority, as the standalone tool must report it.
///
/// The three states are deliberately distinct: `absent` means neither a
/// readable store nor a durable marker records a version, `known` carries the
/// version the admission's own probe resolves (a store that reports one is
/// authoritative, otherwise the marker), and `refused` carries the stable code
/// of a probe or marker refusal. A refused domain is never flattened into
/// version zero, so an unreadable, ahead or corrupt authority stays
/// distinguishable from an absent one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum DomainAuthority {
    Absent,
    Known { version: u32 },
    Refused { code: String },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainStateProjection {
    pub domain_id: String,
    pub authority: DomainAuthority,
    /// The marker's authoritative version when a marker exists.
    pub marker_schema_version: Option<u32>,
    pub target_schema_version: u32,
}

/// Probe the root through the same store owners the client uses, so the tool
/// reports the client's own facts instead of re-deriving them.
///
/// Each domain is projected independently: a domain whose store or marker this
/// binary refuses is reported as `refused` with its stable code while every
/// other domain keeps its own authority. The version in `known` is exactly the
/// one the admission's `probe_domain` resolves, so a store that committed a
/// conversion before its bookkeeping still reports the committed version — the
/// projection never lowers it back to a stale marker.
pub fn domain_state_projection(data_root: &Path) -> Result<Vec<DomainStateProjection>> {
    let frontier = embedded_frontier()?;
    let marker_root = data_root
        .join("client-state")
        .join("migrations")
        .join("domain-state");
    let mut states = Vec::with_capacity(frontier.domains.len());
    for domain in &frontier.domains {
        let marker = match load_domain_marker(&marker_root, domain) {
            Ok(marker) => marker,
            Err(error) => {
                states.push(DomainStateProjection {
                    domain_id: domain.domain_id.clone(),
                    authority: DomainAuthority::Refused {
                        code: safe_error_code(&error).to_owned(),
                    },
                    marker_schema_version: None,
                    target_schema_version: domain.target_schema_version,
                });
                continue;
            }
        };
        let marker_schema_version = marker
            .as_ref()
            .map(|marker| marker.authoritative_schema_version);
        let authority = match probe_authority(&marker_root, domain) {
            Ok((0, false)) => DomainAuthority::Absent,
            Ok((version, _)) => DomainAuthority::Known { version },
            Err(error) => DomainAuthority::Refused {
                code: safe_error_code(&error).to_owned(),
            },
        };
        states.push(DomainStateProjection {
            domain_id: domain.domain_id.clone(),
            authority,
            marker_schema_version,
            target_schema_version: domain.target_schema_version,
        });
    }
    Ok(states)
}

/// Project the immutable embedded frontier. This is the single schema authority:
/// the standalone tool reads it instead of keeping its own domain list.
pub fn frontier_projection_struct() -> Result<FrontierProjection> {
    let frontier = embedded_frontier()?;
    Ok(FrontierProjection {
        frontier_id: frontier.frontier_id.clone(),
        domains: frontier
            .domains
            .iter()
            .map(|domain| FrontierDomainProjection {
                domain_id: domain.domain_id.clone(),
                target_schema_version: domain.target_schema_version,
                steps: domain
                    .steps
                    .iter()
                    .map(|step| FrontierStepProjection {
                        step_id: step.step_id.clone(),
                        from_schema_version: step.from_schema_version,
                        to_schema_version: step.to_schema_version,
                    })
                    .collect(),
            })
            .collect(),
    })
}

pub fn admission_json(data_root: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(admit(data_root)?)?)
}

pub fn frontier_projection() -> Result<serde_json::Value> {
    let frontier = embedded_frontier()?;
    Ok(frontier_projection_for(&frontier))
}

fn frontier_projection_for(frontier: &MigrationFrontier) -> serde_json::Value {
    json!({
        "frontierId": &frontier.frontier_id,
        "domains": frontier.domains.iter().map(|domain| json!({
            "domainId": &domain.domain_id,
            "targetSchemaVersion": domain.target_schema_version,
            "requiredStepIds": domain.steps.iter().map(|step| &step.step_id).collect::<Vec<_>>(),
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests;
