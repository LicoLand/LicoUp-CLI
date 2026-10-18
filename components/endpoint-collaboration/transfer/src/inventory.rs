//! The owned transfer inventory.
//!
//! This composes the full-data-root archive owner rather than duplicating it.
//! `licoup_foundation` already walks the logical data root, refuses non-portable
//! paths, applies the extraction limits and reports the credential-custody
//! limitation; what it deliberately does not decide is what a *transfer* must do
//! with each area. This module answers exactly that question:
//!
//! * a **managed payload** travels in the archive and is restored by its owner,
//! * an **external reference** names something the archive cannot carry and that
//!   its own owner must re-establish, and
//! * a **nonportable credential** must be re-entered, re-signed-in or
//!   re-authorized on the new device by the person who owns it.
//!
//! Nothing here reports a credential value, a key, or a secret. A requirement is
//! a domain plus a fixed custody classification, so there is no field a value
//! could be written into.

use anyhow::{Result, ensure};
use licoup_foundation::core::full_data_root_archive::{
    ARCHIVE_LAYOUT, ArchiveManifest, InventoryEntry, InventoryKind, RecoveryCoverage,
    RecoveryLimitation,
};
use serde::{Deserialize, Serialize};

use crate::activation::IdentityActivationRequirement;
use crate::ownership::TransferOwnership;

/// One area of the data root an application owner is responsible for.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedDomain {
    /// The client-state collection store and its preferences.
    ClientState,
    /// Conversation stores, canonical and group, and their projections.
    Conversation,
    /// The selected workspace manifest and its projections.
    Workspace,
    /// The forward-only migration ledger and domain markers.
    MigrationJournal,
    /// The activity journal.
    Activity,
    /// Preserved snapshots the client restored or keeps.
    Snapshot,
    /// Archives the client wrote for its own use.
    Archive,
    /// Caches rebuilt from their own source.
    Cache,
    /// Temporaries that hold no retained content.
    Temp,
    /// Log segments.
    Logs,
    /// Owner-installed resources that are not client state.
    Resource,
    /// An optional package's own data home.
    PackageData,
}

impl ManagedDomain {
    /// Whether an archive member in this area carries content that must be
    /// present after a transfer, as opposed to content an owner rebuilds.
    #[must_use]
    pub const fn is_retained_content(self) -> bool {
        matches!(
            self,
            Self::ClientState
                | Self::Conversation
                | Self::Workspace
                | Self::MigrationJournal
                | Self::Activity
                | Self::Snapshot
                | Self::Archive
                | Self::PackageData
        )
    }
}

/// The kind of owner that holds something the archive cannot carry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalOwner {
    /// History the provider account retains; provider authorization, not this
    /// archive, decides whether it is available on the new device.
    ProviderRetainedHistory,
    /// A project workspace outside the data root. The archive carries the
    /// reference; it never carries the project's files.
    ProjectWorkspace,
    /// A location another application owns.
    OtherApplication,
    /// Operating-system or hardware state outside the application's custody.
    OperatingSystem,
}

/// How the owner of a credential domain re-establishes it on a new device.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialCustody {
    /// The person re-enters the provider key into the new device's own custody.
    ProviderKeyReentry,
    /// The person signs in again, and the new device obtains a fresh token.
    TokenSignIn,
    /// The material is not exportable; its owner must authorize this endpoint.
    NonExportableReauthorization,
}

impl CredentialCustody {
    /// The fixed, non-secret explanation reported to a person.
    ///
    /// It is a constant per classification: an inventory has no free-text
    /// credential field, so no key handle, token or provider value can reach a
    /// report through this type.
    #[must_use]
    pub const fn requirement(self) -> &'static str {
        match self {
            Self::ProviderKeyReentry => {
                "provider keys do not travel: re-enter them on this device"
            }
            Self::TokenSignIn => {
                "the provider account token does not travel: sign in again on this device"
            }
            Self::NonExportableReauthorization => {
                "this material is not exportable: its owner must re-authorize this endpoint"
            }
        }
    }
}

/// One captured path an owner is responsible for.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPayload {
    pub path: String,
    pub kind: InventoryKind,
    pub size: u64,
    pub domain: ManagedDomain,
}

/// Something outside the payload that must be re-established by its own owner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReference {
    pub domain: String,
    pub owner: ExternalOwner,
}

impl ExternalReference {
    /// The fixed, non-secret explanation of this reference.
    #[must_use]
    pub const fn requirement(&self) -> &'static str {
        match self.owner {
            ExternalOwner::ProviderRetainedHistory => {
                "provider-retained history is not in this archive: provider authorization decides availability"
            }
            ExternalOwner::ProjectWorkspace => {
                "the project workspace stays where it is: this device holds only the reference"
            }
            ExternalOwner::OtherApplication => {
                "another application owns this location and it is not transferred"
            }
            ExternalOwner::OperatingSystem => {
                "operating-system state is not transferred and is not reconstructed here"
            }
        }
    }
}

/// A credential domain that must be re-established on the new device.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRequirement {
    pub domain: String,
    pub custody: CredentialCustody,
}

/// A captured path no declaration claims.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnattributedPath {
    pub path: String,
    pub kind: InventoryKind,
    pub size: u64,
}

/// A limitation the archive declares that no declaration explains.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnexplainedLimitation {
    pub domain: String,
    /// The archive owner's own non-secret reason, carried verbatim because the
    /// archive owner wrote it and it is already privacy-safe.
    pub reason: String,
}

/// What a person must be told about one inventory domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferLimitation {
    pub domain: String,
    pub kind: LimitationKind,
}

impl TransferLimitation {
    /// The fixed, non-secret explanation of this limitation.
    #[must_use]
    pub fn requirement(&self) -> &str {
        match &self.kind {
            LimitationKind::External(owner) => ExternalReference {
                domain: self.domain.clone(),
                owner: *owner,
            }
            .requirement(),
            LimitationKind::Credential(custody) => custody.requirement(),
            LimitationKind::Unattributed => {
                "an area of the data root has no declared owner: this inventory cannot say what it needs"
            }
            LimitationKind::Unexplained => {
                "the archive declares a limitation this inventory does not classify"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LimitationKind {
    External(ExternalOwner),
    Credential(CredentialCustody),
    Unattributed,
    Unexplained,
}

/// Whether this inventory describes every captured path and every declared limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InventoryCompleteness {
    /// Every captured path is attributed and every declared limit is classified.
    /// It still says nothing about whether an identity may be activated.
    Complete,
    /// At least one path or limit is unattributed; a person must be told.
    Limited,
}

/// One classified full-data-root inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferInventory {
    layout: String,
    container: String,
    source_home: String,
    created_at_unix: u64,
    /// What the archive owner declared. Preserved verbatim, including its
    /// `Limited` value, so this composition never upgrades the owner's claim.
    declared_coverage: RecoveryCoverage,
    managed: Vec<ManagedPayload>,
    external: Vec<ExternalReference>,
    credentials: Vec<CredentialRequirement>,
    unattributed: Vec<UnattributedPath>,
    unexplained: Vec<UnexplainedLimitation>,
    total_bytes: u64,
}

impl TransferInventory {
    /// Classify one archive manifest against a declared ownership table.
    ///
    /// The manifest's own structure was already validated by the archive owner
    /// before it was published or imported; this refuses a manifest that does
    /// not carry the archive layout it claims, so a caller cannot classify
    /// something that is not a full-data-root inventory.
    pub fn compose(manifest: &ArchiveManifest, ownership: &TransferOwnership) -> Result<Self> {
        ensure!(
            manifest.layout == ARCHIVE_LAYOUT,
            "transfer_inventory_layout_unsupported"
        );
        ensure!(
            !manifest.source_home.is_empty(),
            "transfer_inventory_source_home_missing"
        );

        let (mut managed, mut unattributed) = classify_entries(&manifest.entries, ownership);

        let mut external = Vec::new();
        let mut credentials = Vec::new();
        let mut unexplained = Vec::new();
        for limitation in &manifest.limitations {
            classify_limitation(
                limitation,
                ownership,
                &mut external,
                &mut credentials,
                &mut unexplained,
            );
        }

        // Deterministic order independent of the archive's own member order.
        managed.sort_by(|left, right| left.path.cmp(&right.path));
        unattributed.sort_by(|left, right| left.path.cmp(&right.path));
        external.sort_by(|left, right| left.domain.cmp(&right.domain));
        credentials.sort_by(|left, right| left.domain.cmp(&right.domain));
        unexplained.sort_by(|left, right| left.domain.cmp(&right.domain));
        external.dedup();
        credentials.dedup();
        unexplained.dedup();

        Ok(Self {
            layout: manifest.layout.clone(),
            container: manifest.container.clone(),
            source_home: manifest.source_home.clone(),
            created_at_unix: manifest.created_at_unix,
            declared_coverage: manifest.coverage,
            managed,
            external,
            credentials,
            unattributed,
            unexplained,
            total_bytes: manifest.total_bytes,
        })
    }

    /// What this inventory's own classification covers.
    ///
    /// Deliberately independent of [`Self::declared_coverage`]: an archive that
    /// captured everything still describes areas only its owner can re-establish.
    #[must_use]
    pub fn completeness(&self) -> InventoryCompleteness {
        if self.unattributed.is_empty() && self.unexplained.is_empty() {
            InventoryCompleteness::Complete
        } else {
            InventoryCompleteness::Limited
        }
    }

    /// The coverage the archive owner declared for the capture it produced.
    #[must_use]
    pub const fn declared_coverage(&self) -> RecoveryCoverage {
        self.declared_coverage
    }

    #[must_use]
    pub fn layout(&self) -> &str {
        &self.layout
    }

    #[must_use]
    pub fn container(&self) -> &str {
        &self.container
    }

    /// The logical data home the capture was taken from; provenance, no authority.
    #[must_use]
    pub fn source_home(&self) -> &str {
        &self.source_home
    }

    #[must_use]
    pub const fn created_at_unix(&self) -> u64 {
        self.created_at_unix
    }

    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    #[must_use]
    pub fn managed(&self) -> &[ManagedPayload] {
        &self.managed
    }

    #[must_use]
    pub fn external(&self) -> &[ExternalReference] {
        &self.external
    }

    #[must_use]
    pub fn credentials(&self) -> &[CredentialRequirement] {
        &self.credentials
    }

    #[must_use]
    pub fn unattributed(&self) -> &[UnattributedPath] {
        &self.unattributed
    }

    #[must_use]
    pub fn unexplained(&self) -> &[UnexplainedLimitation] {
        &self.unexplained
    }

    /// Total bytes across the attributed retained-content payloads.
    #[must_use]
    pub fn retained_bytes(&self) -> u64 {
        self.managed
            .iter()
            .filter(|payload| payload.kind == InventoryKind::File)
            .filter(|payload| payload.domain.is_retained_content())
            .fold(0_u64, |total, payload| total.saturating_add(payload.size))
    }

    /// Everything the client must report before a transfer starts.
    #[must_use]
    pub fn limitations(&self) -> Vec<TransferLimitation> {
        let mut limitations = Vec::new();
        for reference in &self.external {
            limitations.push(TransferLimitation {
                domain: reference.domain.clone(),
                kind: LimitationKind::External(reference.owner),
            });
        }
        for requirement in &self.credentials {
            limitations.push(TransferLimitation {
                domain: requirement.domain.clone(),
                kind: LimitationKind::Credential(requirement.custody),
            });
        }
        if !self.unattributed.is_empty() {
            limitations.push(TransferLimitation {
                domain: "unattributed-payload".to_string(),
                kind: LimitationKind::Unattributed,
            });
        }
        for unexplained in &self.unexplained {
            limitations.push(TransferLimitation {
                domain: unexplained.domain.clone(),
                kind: LimitationKind::Unexplained,
            });
        }
        limitations
    }

    /// What may be activated from this inventory — which is a requirement, never
    /// an identity. See [`crate::activation`].
    #[must_use]
    pub fn activation_requirement(&self) -> IdentityActivationRequirement {
        IdentityActivationRequirement::for_inventory(
            self.credentials.iter().map(|requirement| requirement.custody),
        )
    }
}

fn classify_limitation(
    limitation: &RecoveryLimitation,
    ownership: &TransferOwnership,
    external: &mut Vec<ExternalReference>,
    credentials: &mut Vec<CredentialRequirement>,
    unexplained: &mut Vec<UnexplainedLimitation>,
) {
    if let Some(owner) = ownership.external_owner(&limitation.domain) {
        external.push(ExternalReference {
            domain: limitation.domain.clone(),
            owner,
        });
        return;
    }
    if let Some(custody) = ownership.credential_custody(&limitation.domain) {
        credentials.push(CredentialRequirement {
            domain: limitation.domain.clone(),
            custody,
        });
        return;
    }
    unexplained.push(UnexplainedLimitation {
        domain: limitation.domain.clone(),
        reason: limitation.reason.clone(),
    });
}

/// Classify the entries of a manifest without building a full inventory.
///
/// Used by the caller that only needs the managed/unattributed split, for
/// example to decide whether a capture is worth starting.
pub fn classify_entries(
    entries: &[InventoryEntry],
    ownership: &TransferOwnership,
) -> (Vec<ManagedPayload>, Vec<UnattributedPath>) {
    let mut managed = Vec::new();
    let mut unattributed = Vec::new();
    for entry in entries {
        match ownership.managed_domain(&entry.path) {
            Some(domain) => managed.push(ManagedPayload {
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
                domain,
            }),
            None => unattributed.push(UnattributedPath {
                path: entry.path.clone(),
                kind: entry.kind,
                size: entry.size,
            }),
        }
    }
    (managed, unattributed)
}
