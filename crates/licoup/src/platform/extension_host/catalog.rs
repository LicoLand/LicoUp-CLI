//! One typed capability catalog, published one epoch at a time.
//!
//! The catalog is the answer to "what can this host serve right now?". It is a
//! value, not a service: [`CatalogSnapshot`] is immutable, and every surface
//! reads the same `Arc` of it. A commit publishes a whole new snapshot with a
//! new epoch, so no reader can observe a catalog in which half of an update has
//! landed — the mixed-version catalog is unrepresentable rather than merely
//! discouraged.
//!
//! The entries are *facts about instances*, not a vendor list. Identity is a
//! namespaced string, profiles are the published contract's own ids, and an
//! unrecognised profile is preserved with its id and version so a newer
//! extension's declaration is not silently erased by an older host. Nothing
//! here enumerates a vendor, a provider or a model, which is what lets a new
//! extension appear without regenerating the client.
//!
//! The admission *rules* stay with their owner
//! ([`licoup_application::DiscoveredCapabilities`]): [`CatalogSnapshot::discovered`] hands the
//! routable capabilities to that owner's [`DiscoveredCapabilities`], so a
//! required capability this catalog cannot serve refuses that call and only that
//! call, and an unknown optional attribute is preserved rather than acted on.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use licoup_application::{
    ActivationMode, ContractRange, DeclaredAttribute, DiscoveredCapabilities,
};
use licoup_extension_contracts::deployment::InstanceLifecycle;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform::extension_packages::state::{Admission, InstanceIdentity};

use super::journal::ActivePointer;

/// One committed registry epoch.
///
/// The epoch is the version of the *catalog*, not of a package and not of an
/// instance. An invocation records the epoch it was admitted under so its later
/// `observe`, `cancel` and result can be explained against the catalog that
/// accepted it even after the catalog has moved on.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CatalogEpoch(u64);

impl CatalogEpoch {
    /// The epoch before anything has been committed. No instance can carry it:
    /// an instance's epoch is the epoch its activation produced.
    pub const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    /// The epoch a durable record carried, rebased onto this run's catalogue.
    pub(crate) const fn from_recorded(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl std::fmt::Display for CatalogEpoch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Whether one instance still accepts new work, in the catalog's own vocabulary.
///
/// The runtime authority for this flag is
/// [`crate::platform::extension_packages::state::InstanceMachine`]; this is its
/// serializable projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogAdmission {
    Open,
    Withdrawn,
}

impl From<Admission> for CatalogAdmission {
    fn from(admission: Admission) -> Self {
        match admission {
            Admission::Open => Self::Open,
            Admission::Withdrawn => Self::Withdrawn,
        }
    }
}

/// What is known about the process behind one instance.
///
/// [`InstanceLifecycle::Stopped`] is a *catalogue* state: it says this host no
/// longer routes the instance. It is not evidence that the operating-system
/// process exited — a session nobody observed is not a writer that stopped —
/// so the two facts are kept apart, and a caller that would treat a stop as
/// cleanup evidence reads this field rather than the state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionOwner {
    /// This host holds the session and its carrier is answerable for it.
    Held,
    /// The carrier — or the original session owner — confirmed the release.
    StoppedVerified,
    /// No session owner confirmed the process is gone. Never cleanup evidence.
    StoppedUnverified,
}

impl SessionOwner {
    /// Whether a stop may be treated as confirmed by its owner.
    pub const fn is_verified_stop(self) -> bool {
        matches!(self, Self::StoppedVerified)
    }
}

/// What this host decided about one declared profile.
///
/// `Unpublished` is a fact, not an error: a profile id this host does not know
/// is kept with its id and wire major so the declaration survives, while
/// granting nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogProfileStatus {
    Available,
    MissingRequired,
    MissingAlternative,
    Unpublished,
    MajorMismatch,
    RequiresNewerMinor,
}

impl CatalogProfileStatus {
    /// Whether this profile's own operations can be served.
    pub const fn serves(self) -> bool {
        matches!(self, Self::Available)
    }
}

/// One profile an instance declared, as the catalog publishes it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogProfile {
    pub id: String,
    pub major: u32,
    pub status: CatalogProfileStatus,
    /// Capabilities this profile contributes, when the profile is available.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// For a refused profile, the missing method names that caused it. Empty
    /// when the profile is available or unpublished.
    #[serde(default)]
    pub missing: Vec<String>,
}

/// One instance as every surface reads it.
///
/// The four facts stay separate: [`InstanceIdentity`] carries the package
/// version, the instance id, the generation and the registry epoch; the
/// descriptor facts (implementation version, contract range, attributes) are
/// separate again, so "the same package version" can never be read as "the same
/// running thing".
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub identity: InstanceIdentity,
    /// The implementation's own published version, from its descriptor.
    pub implementation_version: String,
    pub supported_contract_range: ContractRange,
    #[serde(default)]
    pub profiles: Vec<CatalogProfile>,
    /// Namespaced capabilities this instance serves: the descriptor's own
    /// capabilities plus those of its available profiles.
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub activation: ActivationMode,
    pub state: InstanceLifecycle,
    pub admission: CatalogAdmission,
    /// Whether a session owner confirmed the process is gone. `Stopped` alone
    /// never means that; see [`SessionOwner`].
    pub session_owner: SessionOwner,
    /// The extension's declared attributes, preserved as declared.
    #[serde(default)]
    pub attributes: Vec<DeclaredAttribute>,
    /// The attributes the host understood and may act on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bound_attributes: BTreeMap<String, Value>,
    /// The unknown optional attributes, kept verbatim and never acted on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub preserved_attributes: BTreeMap<String, Value>,
}

impl CatalogEntry {
    pub fn instance_id(&self) -> &str {
        self.identity.instance_id.as_str()
    }

    pub fn package_id(&self) -> &str {
        self.identity.package_id.as_str()
    }

    pub fn generation(&self) -> u64 {
        self.identity.generation
    }

    pub fn registry_epoch(&self) -> CatalogEpoch {
        CatalogEpoch(self.identity.registry_epoch)
    }

    /// Whether this instance serves one namespaced capability.
    pub fn serves(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|served| served == capability)
    }

    /// Whether the catalog may route *new* work to this instance.
    pub fn routable(&self) -> bool {
        self.state == InstanceLifecycle::Active && self.admission == CatalogAdmission::Open
    }

    /// The published profile that contributes one capability, if one does.
    pub fn profile_serving(&self, capability: &str) -> Option<String> {
        self.profiles
            .iter()
            .find(|profile| {
                profile.status.serves()
                    && profile
                        .capabilities
                        .iter()
                        .any(|served| served == capability)
            })
            .map(|profile| profile.id.clone())
    }
}

/// One routable capability and the instances that serve it, in the document a
/// CLI, MCP or UI surface reads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCapability {
    pub capability: String,
    /// Every serving instance, in instance-id order. The first entry that is
    /// routable is the one [`CatalogSnapshot::route`] selects when it has the
    /// highest generation.
    pub instances: Vec<String>,
}

/// The whole catalog at one epoch, as one serializable document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDocument {
    pub epoch: CatalogEpoch,
    pub extensions: Vec<CatalogEntry>,
    pub capabilities: Vec<CatalogCapability>,
    /// Instances a previous run recorded active whose owner has not been
    /// confirmed stopped. They are visible here and route nothing: a new
    /// instance of the same package and permission scope is refused until one
    /// of them is resolved, so an unobserved session is never silently
    /// replaced.
    #[serde(default)]
    pub predecessors: Vec<ActivePointer>,
}

/// One immutable catalog epoch.
#[derive(Clone, Debug)]
pub struct CatalogSnapshot {
    epoch: CatalogEpoch,
    entries: BTreeMap<String, CatalogEntry>,
    served: BTreeMap<String, BTreeSet<String>>,
    predecessors: BTreeMap<String, ActivePointer>,
}

impl CatalogSnapshot {
    /// The catalog before anything was committed.
    pub(crate) fn empty() -> Self {
        Self {
            epoch: CatalogEpoch::INITIAL,
            entries: BTreeMap::new(),
            served: BTreeMap::new(),
            predecessors: BTreeMap::new(),
        }
    }

    /// Build one epoch from its entries and the unconfirmed predecessors a
    /// previous run recorded.
    pub(crate) fn from_entries(
        epoch: CatalogEpoch,
        entries: Vec<CatalogEntry>,
        predecessors: Vec<ActivePointer>,
    ) -> Self {
        let mut by_instance = BTreeMap::new();
        let mut served: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for mut entry in entries {
            entry.capabilities.sort();
            entry.capabilities.dedup();
            for capability in &entry.capabilities {
                served
                    .entry(capability.clone())
                    .or_default()
                    .insert(entry.identity.instance_id.clone());
            }
            by_instance.insert(entry.identity.instance_id.clone(), entry);
        }
        Self {
            epoch,
            entries: by_instance,
            served,
            predecessors: predecessors
                .into_iter()
                .map(|pointer| (pointer.instance_id.clone(), pointer))
                .collect(),
        }
    }

    pub fn epoch(&self) -> CatalogEpoch {
        self.epoch
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    pub fn entry(&self, instance_id: &str) -> Option<&CatalogEntry> {
        self.entries.get(instance_id)
    }

    /// The active pointers a previous run recorded that no owner has confirmed
    /// stopped, in instance-id order.
    pub fn predecessors(&self) -> Vec<&ActivePointer> {
        self.predecessors.values().collect()
    }

    /// Every instance that names this capability, routable or not.
    pub fn instances_serving(&self, capability: &str) -> Vec<&CatalogEntry> {
        self.served
            .get(capability)
            .map(|instances| {
                instances
                    .iter()
                    .filter_map(|instance_id| self.entries.get(instance_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The instance a new call for `capability` is admitted to, if any.
    ///
    /// The selection is stated rather than incidental: only an `Active`
    /// instance with open admission is eligible, the highest generation wins,
    /// and the instance id breaks a tie, so a draining or revoked generation
    /// cannot answer a call that arrived after the catalog moved on.
    pub fn route(&self, capability: &str) -> Option<&CatalogEntry> {
        self.served
            .get(capability)?
            .iter()
            .filter_map(|instance_id| self.entries.get(instance_id))
            .filter(|entry| entry.routable() && entry.serves(capability))
            .max_by(|left, right| {
                left.generation()
                    .cmp(&right.generation())
                    .then_with(|| right.instance_id().cmp(left.instance_id()))
            })
    }

    /// Whether a new call for this capability can be admitted at all.
    pub fn serves(&self, capability: &str) -> bool {
        self.route(capability).is_some()
    }

    /// The capabilities a caller may name, in the shape the catalog's owner
    /// already admits attributes against.
    ///
    /// This is the join between the runtime registry and the descriptor's own
    /// admission rules: the snapshot supplies what is discovered, and
    /// [`DiscoveredCapabilities::admit`] stays the only place that decides what
    /// an unknown or required attribute means.
    pub fn discovered(&self) -> DiscoveredCapabilities {
        DiscoveredCapabilities::new(
            self.served
                .keys()
                .filter(|capability| self.serves(capability))
                .cloned(),
        )
    }

    /// The whole catalog as one value a surface can serialize.
    pub fn document(&self) -> CatalogDocument {
        let mut capabilities = Vec::with_capacity(self.served.len());
        for (capability, instances) in &self.served {
            capabilities.push(CatalogCapability {
                capability: capability.clone(),
                instances: instances.iter().cloned().collect(),
            });
        }
        CatalogDocument {
            epoch: self.epoch,
            extensions: self.entries.values().cloned().collect(),
            capabilities,
            predecessors: self.predecessors.values().cloned().collect(),
        }
    }
}

/// The single owner of the published snapshot.
///
/// Readers take an `Arc` and see one consistent epoch; only the host publishes,
/// and it publishes whole snapshots, never mutations of a shared one.
pub struct CapabilityCatalog {
    published: RwLock<Arc<CatalogSnapshot>>,
}

impl Default for CapabilityCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityCatalog {
    pub fn new() -> Self {
        Self {
            published: RwLock::new(Arc::new(CatalogSnapshot::empty())),
        }
    }

    /// The catalog as every surface reads it.
    ///
    /// A poisoned lock still returns the last committed snapshot: a reader
    /// asking "what does this host serve" must never be the thing that fails a
    /// call after an unrelated panic.
    pub fn snapshot(&self) -> Arc<CatalogSnapshot> {
        match self.published.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub(crate) fn publish(&self, snapshot: CatalogSnapshot) {
        match self.published.write() {
            Ok(mut guard) => *guard = Arc::new(snapshot),
            Err(poisoned) => *poisoned.into_inner() = Arc::new(snapshot),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::extension_packages::state::InstanceIdentity;

    fn entry(
        instance_id: &str,
        generation: u64,
        state: InstanceLifecycle,
        admission: Admission,
        capabilities: &[&str],
    ) -> CatalogEntry {
        CatalogEntry {
            identity: InstanceIdentity::new(
                instance_id,
                "vendor.example/pack",
                "1.0.0",
                generation,
                1,
                Vec::new(),
            )
            .expect("identity"),
            implementation_version: "1.0.0".to_owned(),
            supported_contract_range: ContractRange {
                major: 1,
                minimum_minor: 0,
            },
            profiles: Vec::new(),
            capabilities: capabilities.iter().map(|name| (*name).to_owned()).collect(),
            activation: ActivationMode::OnDemand,
            state,
            admission: admission.into(),
            session_owner: SessionOwner::Held,
            attributes: Vec::new(),
            bound_attributes: BTreeMap::new(),
            preserved_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn routing_prefers_the_highest_open_generation_and_never_a_draining_one() {
        let snapshot = CatalogSnapshot::from_entries(
            CatalogEpoch(3),
            vec![
                entry(
                    "instance-a",
                    1,
                    InstanceLifecycle::Draining,
                    Admission::Withdrawn,
                    &["acme.tools/render"],
                ),
                entry(
                    "instance-b",
                    2,
                    InstanceLifecycle::Active,
                    Admission::Open,
                    &["acme.tools/render"],
                ),
            ],
            Vec::new(),
        );
        assert_eq!(
            snapshot
                .route("acme.tools/render")
                .map(CatalogEntry::instance_id),
            Some("instance-b")
        );
        assert!(snapshot.serves("acme.tools/render"));
        assert!(!snapshot.serves("acme.tools/absent"));
        // A draining instance is still visible; it just cannot be routed to.
        assert_eq!(snapshot.instances_serving("acme.tools/render").len(), 2);
        assert!(snapshot.discovered().supports("acme.tools/render"));
        assert!(!snapshot.discovered().supports("acme.tools/absent"));
    }

    #[test]
    fn a_withdrawn_or_failed_instance_serves_nothing_new() {
        for (state, admission) in [
            (InstanceLifecycle::Draining, Admission::Withdrawn),
            (InstanceLifecycle::Failed, Admission::Open),
            (InstanceLifecycle::Quarantined, Admission::Open),
            (InstanceLifecycle::Stopped, Admission::Open),
        ] {
            let snapshot = CatalogSnapshot::from_entries(
                CatalogEpoch(1),
                vec![entry(
                    "instance-a",
                    1,
                    state,
                    admission,
                    &["acme.tools/render"],
                )],
                Vec::new(),
            );
            assert!(snapshot.route("acme.tools/render").is_none(), "{state:?}");
            assert!(!snapshot.serves("acme.tools/render"), "{state:?}");
            assert_eq!(snapshot.instances_serving("acme.tools/render").len(), 1);
        }
    }

    #[test]
    fn a_published_document_round_trips_through_json_without_a_vendor_list() {
        let snapshot = CatalogSnapshot::from_entries(
            CatalogEpoch(7),
            vec![entry(
                "instance-a",
                1,
                InstanceLifecycle::Active,
                Admission::Open,
                &["acme.tools/render", "acme.tools/plan"],
            )],
            Vec::new(),
        );
        let document = snapshot.document();
        let encoded = serde_json::to_value(&document).expect("serialize");
        let decoded: CatalogDocument = serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(decoded, document);
        assert_eq!(decoded.epoch, CatalogEpoch(7));
        assert_eq!(decoded.capabilities.len(), 2);
        let entry = snapshot.entry("instance-a").expect("entry");
        assert_eq!(entry.package_id(), "vendor.example/pack");
        assert_eq!(entry.capabilities.len(), 2);
        assert!(entry.serves("acme.tools/render"));
    }
}
