//! Stage, prepare, activate: the three steps between a descriptor and a routed
//! capability, and the receipt that records what was committed.
//!
//! The split exists so that nothing a descriptor claims can touch the live
//! catalog on its way in:
//!
//! - **Stage** is pure. The descriptor is validated, its contract range is
//!   negotiated against the host's, each declared profile gets its published
//!   status, and every namespaced attribute is decided against what the catalog
//!   currently serves. A staged value holds no session, no identity and no
//!   lock, and staging cannot change what the host answers to anybody.
//! - **Prepare** allocates the instance's generation and starts the carrier.
//!   The handshake's own answer — the methods the runtime really implements —
//!   replaces the manifest's claim for the live profile decision, and the
//!   carrier must reach `extension.ready` before anything may be committed. A
//!   failed preparation releases its session and leaves the catalog exactly as
//!   it was.
//! - **Activate** is the compare-and-swap. It commits against the epoch the
//!   preparation observed; an unrelated concurrent commit is re-based onto, and
//!   an instance that a newer generation has already replaced is refused and
//!   shut down instead of committed. Only this step may publish a new epoch.
//!
//! A [`StagedExtension`] and a [`PreparedExtension`] are values, and a prepared
//! value is bound to the host that prepared it: committing it elsewhere is
//! refused rather than silently treated as an activation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use licoup_application::{
    ActivationMode, AdoptedAttributes, ApplicationFailure, CapabilityDescriptor, ContractRange,
    DeclaredAttribute,
};
use licoup_extension_contracts::profile::{DeclaredMethods, ProfileDeclaration, ProfileStatus};
use serde_json::Value;

use super::carrier::ExtensionCarrier;
use super::identity::HostIncarnation;

use super::carrier::{CarrierSession, InitializeRequest, InitializedProfileSet};
use super::catalog::{CatalogEpoch, CatalogProfile, CatalogProfileStatus};
use super::{COMPONENT, refusal};

/// One extension being offered to the host.
#[derive(Clone, Debug)]
pub struct StageRequest {
    pub descriptor: CapabilityDescriptor,
    /// The methods the package claims. This is a claim; the handshake's answer
    /// decides what the *live* instance serves.
    pub methods: DeclaredMethods,
    pub profiles: Vec<ProfileDeclaration>,
    /// How this instance may be activated. The descriptor's own lifecycle
    /// declaration is the default the caller may narrow, never broaden.
    pub activation: ActivationMode,
    /// The approved permission scope this instance runs with. Two instances of
    /// one package version may differ here, which is why it is part of identity.
    pub permission_scope: Vec<String>,
}

/// A validated extension that has not touched the catalog.
#[derive(Clone, Debug)]
pub struct StagedExtension {
    pub(crate) package_id: String,
    pub(crate) package_version: String,
    pub(crate) implementation_version: String,
    pub(crate) descriptor: CapabilityDescriptor,
    pub(crate) profiles: Vec<ProfileDeclaration>,
    pub(crate) activation: ActivationMode,
    pub(crate) permission_scope: Vec<String>,
    pub(crate) host_contract_range: ContractRange,
    pub(crate) staged_epoch: CatalogEpoch,
    pub(crate) resolved_profiles: Vec<CatalogProfile>,
    pub(crate) staged_capabilities: Vec<String>,
    pub(crate) declared_attributes: Vec<DeclaredAttribute>,
    pub(crate) adopted_attributes: AdoptedAttributes,
}

impl StagedExtension {
    pub fn package_id(&self) -> &str {
        self.package_id.as_str()
    }

    pub fn package_version(&self) -> &str {
        self.package_version.as_str()
    }

    /// The capabilities this declaration would serve once committed.
    pub fn staged_capabilities(&self) -> &[String] {
        &self.staged_capabilities
    }

    /// The profiles as the host decided them at staging time.
    pub fn profiles(&self) -> &[CatalogProfile] {
        &self.resolved_profiles
    }

    /// The epoch this staging decision was made against. An activation of this
    /// value is compared against it.
    pub fn staged_epoch(&self) -> CatalogEpoch {
        self.staged_epoch
    }

    /// The attributes the host understood, and the ones it preserved unknown.
    pub fn adopted_attributes(&self) -> &AdoptedAttributes {
        &self.adopted_attributes
    }
}

/// A started, handshaken, ready instance that has not been committed.
///
/// The value belongs to the host run that prepared it and to the carrier that
/// started its session: activation elsewhere is refused, and the session is
/// released through the carrier that owns it.
pub struct PreparedExtension {
    pub(crate) staged: StagedExtension,
    pub(crate) host: HostIncarnation,
    pub(crate) carrier: Arc<dyn ExtensionCarrier>,
    pub(crate) instance_id: String,
    pub(crate) generation: u64,
    pub(crate) expected_epoch: CatalogEpoch,
    pub(crate) session: CarrierSession,
    pub(crate) live_profiles: Vec<CatalogProfile>,
    pub(crate) live_capabilities: Vec<String>,
    pub(crate) live_methods: DeclaredMethods,
    pub(crate) accepted_profiles: Vec<String>,
}

impl PreparedExtension {
    pub fn package_id(&self) -> &str {
        self.staged.package_id.as_str()
    }

    pub fn instance_id(&self) -> &str {
        self.instance_id.as_str()
    }

    /// The generation this preparation allocated. It is already fixed, so a
    /// re-based commit cannot make two preparations share one generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The epoch this preparation read; activation compares against it.
    pub fn expected_epoch(&self) -> CatalogEpoch {
        self.expected_epoch
    }

    /// The profiles as the runtime's own handshake answered them.
    pub fn live_profiles(&self) -> &[CatalogProfile] {
        &self.live_profiles
    }

    /// The capabilities the handshake leaves this instance serving.
    pub fn live_capabilities(&self) -> &[String] {
        &self.live_capabilities
    }

    pub fn accepted_profiles(&self) -> &[String] {
        &self.accepted_profiles
    }

    /// The methods the runtime answered with, replacing the manifest's claim.
    pub fn live_methods(&self) -> &DeclaredMethods {
        &self.live_methods
    }
}

/// What one committed activation produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationReceipt {
    pub instance_id: String,
    pub package_id: String,
    pub generation: u64,
    pub registry_epoch: CatalogEpoch,
    /// The instances this commit withdrew admission from because the new
    /// generation replaces them (same package, same permission scope).
    pub drained: Vec<String>,
    pub profiles: Vec<CatalogProfile>,
    pub capabilities: Vec<String>,
}

/// Resolve one declaration against the host's contract range and the methods
/// the runtime answered with.
///
/// The version decision comes first, exactly as it does in the contract: a
/// major mismatch makes the method catalog meaningless.
pub(crate) fn resolve_profiles(
    profiles: &[ProfileDeclaration],
    host_range: ContractRange,
    methods: &DeclaredMethods,
    accepted: Option<&[String]>,
) -> Vec<CatalogProfile> {
    profiles
        .iter()
        .map(|declaration| {
            let mut status = declaration.status(host_range, methods);
            if let Some(accepted) = accepted {
                // The handshake is the live fact: a profile the runtime did not
                // accept is not available even if the manifest listed its
                // methods, and a profile it accepted is decided by its methods.
                if status.is_available() && !accepted.iter().any(|id| id == &declaration.id) {
                    status = ProfileStatus::MissingMethods {
                        missing: Vec::new(),
                    };
                }
            }
            let (status, missing) = match status {
                ProfileStatus::Available => (CatalogProfileStatus::Available, Vec::new()),
                ProfileStatus::Unpublished => (CatalogProfileStatus::Unpublished, Vec::new()),
                ProfileStatus::MajorMismatch => (CatalogProfileStatus::MajorMismatch, Vec::new()),
                ProfileStatus::RequiresNewerMinor => {
                    (CatalogProfileStatus::RequiresNewerMinor, Vec::new())
                }
                ProfileStatus::MissingMethods { missing } => (
                    CatalogProfileStatus::MissingRequired,
                    missing.into_iter().map(str::to_owned).collect(),
                ),
                ProfileStatus::MissingAlternative { groups } => (
                    CatalogProfileStatus::MissingAlternative,
                    groups
                        .into_iter()
                        .filter_map(|group| group.first().copied())
                        .map(str::to_owned)
                        .collect(),
                ),
            };
            CatalogProfile {
                id: declaration.id.clone(),
                major: declaration.major,
                status,
                capabilities: if status.serves() {
                    declaration.capabilities.clone()
                } else {
                    Vec::new()
                },
                missing,
            }
        })
        .collect()
}

/// The capabilities one instance serves.
///
/// The rule is stated once so staging and the live decision cannot drift:
/// profile capabilities count only from profiles that are available, and the
/// descriptor's own capabilities count only when the extension implements at
/// least one published call contract. An extension whose every profile is
/// unpublished is kept in the catalog with its declaration intact and serves
/// nothing, which is the difference between "preserved" and "granted".
pub(crate) fn served_capabilities(
    descriptor: &CapabilityDescriptor,
    profiles: &[CatalogProfile],
) -> Vec<String> {
    let mut served: BTreeSet<String> = profiles
        .iter()
        .filter(|profile| profile.status.serves())
        .flat_map(|profile| profile.capabilities.iter().cloned())
        .collect();
    if profiles.iter().any(|profile| profile.status.serves()) {
        served.extend(descriptor.capabilities.iter().cloned());
    }
    served.into_iter().collect()
}

/// The `extension.initialize` request for one staged declaration.
pub(crate) fn initialize_request(staged: &StagedExtension) -> InitializeRequest {
    InitializeRequest {
        host_contract_range: staged.host_contract_range,
        profiles: staged.profiles.clone(),
    }
}

/// Rebuild the live profile decision from the handshake's answer.
pub(crate) fn live_profiles(
    staged: &StagedExtension,
    initialized: &InitializedProfileSet,
) -> Vec<CatalogProfile> {
    resolve_profiles(
        &staged.profiles,
        staged.host_contract_range,
        &initialized.methods,
        Some(&initialized.accepted_profiles),
    )
}

/// The refusal for an activation whose prepared instance was replaced while it
/// was being prepared.
pub(crate) fn superseded(package_id: &str, generation: u64) -> ApplicationFailure {
    refusal("extension_activation_superseded", "extension/activate")
        .with_field("generation")
        .with_presentation_arg("packageId", package_id)
        .with_presentation_arg("generation", &generation.to_string())
        .with_recovery(licoup_application::RecoveryAction::RetryOrReviewRequest)
}

/// The refusal for a prepared value that belongs to a different host.
pub(crate) fn foreign_prepared(instance_id: &str) -> ApplicationFailure {
    refusal("extension_prepared_foreign_host", "extension/activate")
        .with_field("instanceId")
        .with_presentation_arg("instanceId", instance_id)
}

impl std::fmt::Debug for PreparedExtension {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedExtension")
            .field("package_id", &self.staged.package_id)
            .field("instance_id", &self.instance_id)
            .field("generation", &self.generation)
            .field("expected_epoch", &self.expected_epoch)
            .finish_non_exhaustive()
    }
}

/// The bound and preserved attribute maps of one admission, in the catalog's
/// own serializable shape.
pub(crate) fn attribute_buckets(
    admitted: &AdoptedAttributes,
) -> (BTreeMap<String, Value>, BTreeMap<String, Value>) {
    (
        admitted
            .bound()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        admitted
            .preserved()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

/// The component name, re-exported for tests that assert an error chain.
pub const fn host_component() -> &'static str {
    COMPONENT
}
