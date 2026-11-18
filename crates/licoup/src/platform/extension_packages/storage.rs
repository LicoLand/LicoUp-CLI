//! Honest storage accounting, and a GC that only removes what nothing uses.
//!
//! A "you saved 240 MB" banner that quotes the download size is the failure mode
//! this module exists to prevent. Six categories are accounted separately and
//! always all six are reported, so a surface cannot show one number and call it
//! the account:
//!
//! | category | what lives there |
//! |---|---|
//! | core | the trusted host, which no package manager may remove |
//! | optional code | installed package content, per version |
//! | shared runtime | an interpreter or VM several packages reference |
//! | cache | managed download and expansion cache |
//! | user data | the user's own history and state, outside this root |
//! | in-flight pins | versions still serving admitted work |
//!
//! [`SavingsClaim::from_download_only`] is the refusal that makes the rule
//! enforceable: a claim cannot be constructed from a download number at all. A
//! real claim is `measured`, and it carries what is *still* occupied — old
//! versions kept for in-flight work, and shared dependencies another package
//! still uses.
//!
//! [`reclaim`] removes a managed, unreferenced artifact and nothing else: it
//! never removes the core, user data, a pinned version, or a runtime the user
//! installed. [`plan_gc`] decides which artifacts those are.

use crate::platform::extension_packages::install::{
    InstalledPackage, PackageStore, checked_identity,
};
use crate::platform::extension_packages::journal::JournalOperation;
use crate::platform::extension_packages::{directory_bytes, refusal, remove_managed_tree};
use licoup_application::ApplicationFailure;
use licoup_extension_contracts::manifest::USER_RUNTIME_PREFIX;
use std::collections::BTreeMap;

const STORAGE_STAGE: &str = "extension/package-storage";

/// The six things a storage panel must keep apart.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StorageKind {
    Core,
    OptionalCode,
    SharedRuntime,
    Cache,
    UserData,
    InFlightPin,
}

impl StorageKind {
    pub const ALL: [Self; 6] = [
        Self::Core,
        Self::OptionalCode,
        Self::SharedRuntime,
        Self::Cache,
        Self::UserData,
        Self::InFlightPin,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::OptionalCode => "optional-code",
            Self::SharedRuntime => "shared-runtime",
            Self::Cache => "cache",
            Self::UserData => "user-data",
            Self::InFlightPin => "in-flight-pins",
        }
    }
}

/// One accounted artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageEntry {
    pub id: String,
    pub kind: StorageKind,
    pub bytes: u64,
    /// Whether the host put these bytes here and may therefore reclaim them.
    pub managed: bool,
    /// How many live consumers reference them. Zero means reclaimable *if*
    /// nothing else says otherwise.
    pub references: u32,
    /// A version still serving admitted work: its bytes are not free.
    pub pinned: bool,
    /// Something the user installed themselves (their own Node, Python, agent).
    /// The host reuses it and never deletes it.
    pub user_installed: bool,
}

impl StorageEntry {
    pub fn new(id: impl Into<String>, kind: StorageKind, bytes: u64) -> Self {
        Self {
            id: id.into(),
            kind,
            bytes,
            managed: true,
            references: 0,
            pinned: false,
            user_installed: false,
        }
    }

    pub fn with_references(mut self, references: u32) -> Self {
        self.references = references;
        self
    }

    pub fn managed(mut self, managed: bool) -> Self {
        self.managed = managed;
        self
    }

    pub fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn user_installed(mut self, user_installed: bool) -> Self {
        self.user_installed = user_installed;
        self
    }
}

/// Every byte this installation occupies, by category.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StorageReport {
    categories: BTreeMap<StorageKind, u64>,
    /// The size of the download itself, recorded for the record and never as the
    /// account.
    downloaded_bytes: u64,
    /// Versions whose bytes are still occupied although they are not active.
    retained_bytes: u64,
    entries: usize,
    /// Runtimes the host has not measured. They are named rather than counted as
    /// zero bytes.
    unmeasured: Vec<String>,
}

impl StorageReport {
    pub fn account(entries: &[StorageEntry]) -> Self {
        let mut report = Self {
            entries: entries.len(),
            ..Self::default()
        };
        for entry in entries {
            *report.categories.entry(entry.kind).or_insert(0) += entry.bytes;
            if entry.pinned {
                report.retained_bytes += entry.bytes;
            }
        }
        report
    }

    pub fn with_downloaded_bytes(mut self, downloaded_bytes: u64) -> Self {
        self.downloaded_bytes = downloaded_bytes;
        self
    }

    pub fn with_unmeasured(mut self, mut unmeasured: Vec<String>) -> Self {
        unmeasured.sort();
        unmeasured.dedup();
        self.unmeasured = unmeasured;
        self
    }

    /// Runtimes nobody measured, so a panel can say "unknown" instead of zero.
    pub fn unmeasured(&self) -> &[String] {
        &self.unmeasured
    }

    pub fn category(&self, kind: StorageKind) -> u64 {
        self.categories.get(&kind).copied().unwrap_or(0)
    }

    pub fn total_bytes(&self) -> u64 {
        self.categories.values().sum()
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.downloaded_bytes
    }

    pub fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }

    pub fn entries(&self) -> usize {
        self.entries
    }

    /// All six categories, in order. Zeros are explicit: a category with nothing
    /// in it is a fact, not a missing field.
    pub fn lines(&self) -> [(StorageKind, u64); 6] {
        StorageKind::ALL.map(|kind| (kind, self.category(kind)))
    }

    /// The managed bytes a package manager may consider reclaimable at all.
    pub fn managed_bytes(&self) -> u64 {
        self.category(StorageKind::OptionalCode)
            + self.category(StorageKind::SharedRuntime)
            + self.category(StorageKind::Cache)
    }
}

/// Why an artifact was not reclaimed.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RetainReason {
    /// The trusted host.
    Core,
    /// The user's own data.
    UserData,
    /// Still serving admitted work.
    PinnedInFlight,
    /// Another package still references it.
    SharedRuntimeInUse,
    /// A runtime or tool the user installed.
    UserInstalled,
    /// Not this host's bytes to remove.
    NotManaged,
}

impl RetainReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::UserData => "user-data",
            Self::PinnedInFlight => "pinned-in-flight",
            Self::SharedRuntimeInUse => "shared-runtime-in-use",
            Self::UserInstalled => "user-installed",
            Self::NotManaged => "not-managed",
        }
    }
}

/// What a GC pass would remove, what it would keep, and why.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GcOutcome {
    pub removed: Vec<String>,
    pub reclaimed_bytes: u64,
    pub retained: Vec<(String, RetainReason)>,
}

impl GcOutcome {
    pub fn retained_bytes(&self, entries: &[StorageEntry]) -> u64 {
        entries
            .iter()
            .filter(|entry| self.retained.iter().any(|(id, _)| *id == entry.id))
            .map(|entry| entry.bytes)
            .sum()
    }
}

/// Decide what GC may remove.
pub fn plan_gc(entries: &[StorageEntry]) -> GcOutcome {
    let mut outcome = GcOutcome::default();
    for entry in entries {
        let reason = match entry.kind {
            StorageKind::Core => Some(RetainReason::Core),
            StorageKind::UserData => Some(RetainReason::UserData),
            _ if entry.user_installed => Some(RetainReason::UserInstalled),
            _ if !entry.managed => Some(RetainReason::NotManaged),
            _ if entry.pinned => Some(RetainReason::PinnedInFlight),
            _ if entry.references > 0 => Some(RetainReason::SharedRuntimeInUse),
            _ => None,
        };
        match reason {
            Some(reason) => outcome.retained.push((entry.id.clone(), reason)),
            None => {
                outcome.removed.push(entry.id.clone());
                outcome.reclaimed_bytes += entry.bytes;
            }
        }
    }
    outcome.removed.sort();
    outcome.retained.sort();
    outcome
}

/// The outcome of executing a GC pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GcReport {
    /// Store-owned artifacts actually removed.
    pub removed: Vec<String>,
    /// Bytes actually freed.
    pub reclaimed_bytes: u64,
    /// Planned for reclaim but not claimed here: not this store's bytes, or
    /// nothing left at that path. Named, never silently counted as freed.
    pub not_reclaimed: Vec<String>,
    /// What the plan kept, and why.
    pub retained: Vec<(String, RetainReason)>,
}

/// Execute a GC pass over the bytes this store owns.
///
/// The plan comes from [`plan_gc`], so reference counts, pins and user-installed
/// facts stay the caller's. This function removes only what the store owns —
/// installed versions and the managed cache — and reports anything else the plan
/// marked reclaimable in [`GcReport::not_reclaimed`] rather than guessing at a
/// path that belongs to another component.
pub fn reclaim(
    store: &PackageStore,
    entries: &[StorageEntry],
) -> Result<GcReport, ApplicationFailure> {
    let plan = plan_gc(entries);
    let mut report = GcReport {
        retained: plan.retained,
        ..GcReport::default()
    };
    for id in plan.removed {
        if id == "cache" {
            let bytes = remove_managed_tree(&store.root().join("cache"))?;
            if bytes > 0 {
                report.reclaimed_bytes += bytes;
                report.removed.push(id);
            }
            continue;
        }
        match id.split_once('@') {
            Some((package_id, version)) if checked_identity(package_id, version).is_ok() => {
                let removed = store.remove_version(
                    package_id,
                    version,
                    JournalOperation::Gc,
                    "managed bytes reclaimed by gc",
                )?;
                if removed.reclaimed_bytes() > 0 {
                    report.reclaimed_bytes += removed.reclaimed_bytes();
                    report.removed.push(id);
                } else {
                    report.not_reclaimed.push(id);
                }
            }
            _ => report.not_reclaimed.push(id),
        }
    }
    Ok(report)
}

/// A claim about how much space an operation saved.
///
/// It cannot be built from a download size, and it always states what is still
/// occupied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavingsClaim {
    /// What was actually reclaimed from disk.
    pub reclaimed_bytes: u64,
    /// Packages whose bytes were downloaded and are still installed or cached.
    pub downloaded_but_retained_bytes: u64,
    /// Shared runtime bytes another package still uses.
    pub shared_still_used_bytes: u64,
    /// Bytes pinned by versions still serving admitted work.
    pub pinned_bytes: u64,
    /// Managed cache bytes removed.
    pub cache_reclaimed_bytes: u64,
}

impl SavingsClaim {
    /// A claim made from a download size alone is not a measurement.
    pub fn from_download_only(_downloaded_bytes: u64) -> Result<Self, ApplicationFailure> {
        Err(refusal("storage_savings_unaccounted", STORAGE_STAGE)
            .with_field("reclaimedBytes")
            .with_recovery(licoup_application::RecoveryAction::CorrectRequest))
    }

    /// A claim built from a real reclaim and the report it happened against.
    ///
    /// Refused when the reclaim is larger than everything the account says is
    /// managed; that would mean a category or a dependency went uncounted.
    pub fn measured(
        report: &StorageReport,
        reclaimed_bytes: u64,
        shared_still_used_bytes: u64,
        pinned_bytes: u64,
        cache_reclaimed_bytes: u64,
    ) -> Result<Self, ApplicationFailure> {
        if reclaimed_bytes > report.total_bytes() || reclaimed_bytes > report.managed_bytes() {
            return Err(refusal("storage_savings_unaccounted", STORAGE_STAGE)
                .with_field("reclaimedBytes")
                .with_presentation_arg("reclaimedBytes", &reclaimed_bytes.to_string())
                .with_presentation_arg("managedBytes", &report.managed_bytes().to_string()));
        }
        Ok(Self {
            reclaimed_bytes,
            downloaded_but_retained_bytes: report
                .downloaded_bytes()
                .saturating_sub(cache_reclaimed_bytes),
            shared_still_used_bytes,
            pinned_bytes: pinned_bytes.max(report.retained_bytes()),
            cache_reclaimed_bytes,
        })
    }

    /// What a surface may say was saved. It deducts everything still occupied.
    pub fn reported_savings_bytes(&self) -> u64 {
        self.reclaimed_bytes
            .saturating_sub(self.shared_still_used_bytes)
    }

    /// Still occupied, named: old versions pinned by in-flight work and shared
    /// dependencies that survived the operation.
    pub fn still_occupied_bytes(&self) -> u64 {
        self.downloaded_but_retained_bytes + self.shared_still_used_bytes + self.pinned_bytes
    }
}

/// In-flight pins: a version's bytes stay occupied while it still serves work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InFlightPins {
    pins: BTreeMap<String, u64>,
}

impl InFlightPins {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin `bytes` under a key such as `package@version#generation`.
    pub fn pin(&mut self, key: impl Into<String>, bytes: u64) {
        *self.pins.entry(key.into()).or_insert(0) += bytes;
    }

    /// Release a pin and report the bytes it was holding.
    pub fn release(&mut self, key: &str) -> u64 {
        self.pins.remove(key).unwrap_or(0)
    }

    pub fn pinned_keys(&self) -> impl Iterator<Item = (&str, u64)> {
        self.pins.iter().map(|(key, bytes)| (key.as_str(), *bytes))
    }

    pub fn total_bytes(&self) -> u64 {
        self.pins.values().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }

    /// The pin entries, with a prefix naming the generation they belong to.
    pub fn entries(&self) -> Vec<StorageEntry> {
        self.pins
            .iter()
            .map(|(key, bytes)| {
                StorageEntry::new(format!("pin:{key}"), StorageKind::InFlightPin, *bytes)
                    .pinned(true)
            })
            .collect()
    }
}

/// Account a real managed root: installed versions separately, shared runtimes
/// reference-counted across the packages that name them, cache and user data as
/// their own categories.
///
/// `runtime_bytes` is what the host measured for the interpreters packages
/// reference, keyed by runtime reference. A runtime nobody measured is reported
/// as *unmeasured* rather than as zero bytes, because "we did not measure it" and
/// "it costs nothing" are different answers to the same question.
pub fn account_store(
    store: &PackageStore,
    installed: &[InstalledPackage],
    core_bytes: u64,
    user_data_bytes: u64,
    runtime_bytes: &BTreeMap<String, u64>,
    pins: &InFlightPins,
) -> Result<StorageReport, ApplicationFailure> {
    let mut entries = vec![StorageEntry::new("core", StorageKind::Core, core_bytes)];

    let mut runtimes: BTreeMap<String, (u32, bool)> = BTreeMap::new();
    for package in installed {
        let bytes = store.installed_bytes(&package.package_id, &package.version)?;
        entries.push(
            StorageEntry::new(
                format!("{}@{}", package.package_id, package.version),
                StorageKind::OptionalCode,
                bytes,
            )
            .with_references(1),
        );
        if let Some(reference) = &package.runtime_ref {
            let user_owned = reference.starts_with(USER_RUNTIME_PREFIX);
            let entry = runtimes.entry(reference.clone()).or_insert((0, user_owned));
            entry.0 += 1;
            entry.1 = entry.1 || user_owned;
        }
    }
    let mut unmeasured = Vec::new();
    for (reference, (references, user_owned)) in runtimes {
        let Some(bytes) = runtime_bytes.get(&reference).copied() else {
            unmeasured.push(reference);
            continue;
        };
        entries.push(
            StorageEntry::new(
                format!("runtime:{reference}"),
                StorageKind::SharedRuntime,
                bytes,
            )
            .with_references(references)
            .managed(!user_owned)
            .user_installed(user_owned),
        );
    }

    let cache_bytes = directory_bytes(&store.root().join("cache"))?;
    entries.push(StorageEntry::new("cache", StorageKind::Cache, cache_bytes));
    entries.push(
        StorageEntry::new("user-data", StorageKind::UserData, user_data_bytes).managed(false),
    );
    entries.extend(pins.entries());

    Ok(StorageReport::account(&entries)
        .with_downloaded_bytes(cache_bytes)
        .with_unmeasured(unmeasured))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::{ensure_private_directory, unique_suffix};
    use std::path::PathBuf;

    fn entries() -> Vec<StorageEntry> {
        vec![
            StorageEntry::new("core", StorageKind::Core, 40_000_000),
            StorageEntry::new(
                "example.specialist.echo@1.0.0",
                StorageKind::OptionalCode,
                4_000,
            ),
            StorageEntry::new(
                "example.specialist.echo@0.9.0",
                StorageKind::OptionalCode,
                3_000,
            )
            .pinned(true),
            StorageEntry::new(
                "runtime:runtime.node-22",
                StorageKind::SharedRuntime,
                90_000_000,
            )
            .with_references(2),
            StorageEntry::new("cache", StorageKind::Cache, 12_000),
            StorageEntry::new("user-data", StorageKind::UserData, 500_000).managed(false),
            StorageEntry::new(
                "runtime:user:python3",
                StorageKind::SharedRuntime,
                60_000_000,
            )
            .managed(false)
            .user_installed(true)
            .with_references(1),
        ]
    }

    #[test]
    fn the_account_names_all_six_categories_and_never_only_the_download() {
        let report = StorageReport::account(&entries()).with_downloaded_bytes(4_096);
        let lines = report.lines();
        assert_eq!(lines.len(), 6);
        assert_eq!(report.category(StorageKind::Core), 40_000_000);
        assert_eq!(report.category(StorageKind::OptionalCode), 7_000);
        assert_eq!(report.category(StorageKind::SharedRuntime), 150_000_000);
        assert_eq!(report.category(StorageKind::Cache), 12_000);
        assert_eq!(report.category(StorageKind::UserData), 500_000);
        assert_eq!(report.category(StorageKind::InFlightPin), 0);
        assert_eq!(report.downloaded_bytes(), 4_096);
        assert_eq!(report.retained_bytes(), 3_000, "the pinned old version");
        assert_ne!(report.total_bytes(), report.downloaded_bytes());
    }

    #[test]
    fn gc_removes_only_managed_unreferenced_artifacts() {
        let outcome = plan_gc(&entries());
        assert_eq!(
            outcome.removed,
            vec![
                "cache".to_owned(),
                "example.specialist.echo@1.0.0".to_owned()
            ]
        );
        assert_eq!(outcome.reclaimed_bytes, 16_000);
        let reasons: BTreeMap<&str, RetainReason> = outcome
            .retained
            .iter()
            .map(|(id, reason)| (id.as_str(), *reason))
            .collect();
        assert_eq!(reasons["core"], RetainReason::Core);
        assert_eq!(reasons["user-data"], RetainReason::UserData);
        assert_eq!(
            reasons["example.specialist.echo@0.9.0"],
            RetainReason::PinnedInFlight
        );
        assert_eq!(
            reasons["runtime:runtime.node-22"],
            RetainReason::SharedRuntimeInUse,
            "a shared runtime another package uses is retained"
        );
        assert_eq!(
            reasons["runtime:user:python3"],
            RetainReason::UserInstalled,
            "the user's own interpreter is never removed"
        );
    }

    #[test]
    fn no_savings_claim_can_be_made_from_a_download_size() {
        assert_eq!(
            SavingsClaim::from_download_only(240_000_000)
                .expect_err("a download is not a reclaim")
                .code,
            "storage_savings_unaccounted"
        );
    }

    #[test]
    fn a_measured_claim_deducts_what_is_still_used() {
        let report = StorageReport::account(&entries()).with_downloaded_bytes(4_000);
        let claim =
            SavingsClaim::measured(&report, 16_000, 90_000_000, 3_000, 12_000).expect("measured");
        assert_eq!(claim.reclaimed_bytes, 16_000);
        assert_eq!(
            claim.reported_savings_bytes(),
            0,
            "nothing is saved while it is used"
        );
        assert_eq!(claim.still_occupied_bytes(), 90_000_000 + 3_000);

        let modest = SavingsClaim::measured(&report, 4_000, 0, 3_000, 0).expect("measured");
        assert_eq!(modest.reported_savings_bytes(), 4_000);
        assert_eq!(
            modest.still_occupied_bytes(),
            4_000 + 3_000,
            "the download is still on disk and the old version is still pinned"
        );

        assert_eq!(
            SavingsClaim::measured(&report, 400_000_000, 0, 0, 0)
                .expect_err("more than the account holds")
                .code,
            "storage_savings_unaccounted"
        );
    }

    #[test]
    fn a_pin_holds_bytes_until_its_work_settles() {
        let mut pins = InFlightPins::new();
        pins.pin("example.specialist.echo@1.0.0#1", 4_000);
        pins.pin("example.specialist.echo@1.0.0#1", 1_000);
        assert_eq!(pins.total_bytes(), 5_000);
        assert_eq!(pins.entries().len(), 1);
        assert_eq!(pins.release("example.specialist.echo@1.0.0#1"), 5_000);
        assert!(pins.is_empty());

        let mut entries = entries();
        entries.extend(pins.entries());
        let report = StorageReport::account(&entries);
        assert_eq!(report.category(StorageKind::InFlightPin), 0);
    }

    #[test]
    fn the_account_measures_a_real_root_and_counts_shared_runtimes_once() {
        let root: PathBuf =
            std::env::temp_dir().join(format!("licoup-pkg-storage-{}", unique_suffix()));
        let store = PackageStore::open(&root).expect("store");
        ensure_private_directory(&root.join("cache")).expect("cache");
        std::fs::write(root.join("cache").join("blob"), vec![3u8; 256]).expect("write");

        let mut pins = InFlightPins::new();
        pins.pin("example.specialist.echo@0.9.0#1", 512);
        let report =
            account_store(&store, &[], 4_000, 128, &BTreeMap::new(), &pins).expect("account");
        assert_eq!(report.category(StorageKind::Core), 4_000);
        assert_eq!(report.category(StorageKind::Cache), 256);
        assert_eq!(report.category(StorageKind::UserData), 128);
        assert_eq!(report.category(StorageKind::InFlightPin), 512);
        assert!(report.unmeasured().is_empty());
        assert_eq!(
            plan_gc(&[
                StorageEntry::new("cache", StorageKind::Cache, 256),
                StorageEntry::new(
                    "pin:example.specialist.echo@0.9.0#1",
                    StorageKind::InFlightPin,
                    512
                )
                .pinned(true),
            ])
            .removed,
            vec!["cache".to_owned()],
            "in-flight pins are never collected"
        );
        crate::platform::extension_packages::remove_managed_tree(&root).expect("cleanup");
    }
}
