//! What this client knows about its peers' capabilities, and what that knowledge
//! is allowed to do.
//!
//! Installing or removing a capability changes *what a peer offers*. It does not
//! change who the peer is, does not rebuild a session, and does not grant this
//! client any authority: the pinned SDK owns capability flows, governance and the
//! group/device identities they are bound to, and this module owns only the
//! client-side catalogue those flows are announced into.
//!
//! Six rules, each falsifiable from this module's public surface:
//!
//! * **An announcement is not authority.** [`CapabilityCatalogue::announce`]
//!   records what a peer says it offers. [`CapabilityCatalogue::authority`] answers
//!   what this client may do, and it answers `Authorized` only after an explicit
//!   local [`CapabilityCatalogue::grant_authority`]. Recording a peer's own claim
//!   never grants execution, and no announcement can revoke a grant either.
//! * **A device identity and a group identity are different facts.** A peer is
//!   keyed by both ([`PeerKey`]), so the same device in two groups is two peers and
//!   a group is never collapsed onto whichever device announced it first. A group
//!   that is re-announced by a different device is refused rather than re-owned.
//! * **A rotation keeps the identity and refuses a rollback.** A device's identity
//!   rotation epoch may only advance, mirroring the directory's own
//!   `identityRotationEpoch` rule. A lower epoch is refused as
//!   [`CatalogueRefusal::RotationEpochRollback`]; a higher one keeps the device
//!   identity, its local grants and its protected material exactly where they were,
//!   so material committed before a rotation stays recoverable afterwards and a
//!   catalogue change alone never rebuilds a peer.
//! * **A stale announcement is an explicit answer.** An announcement older than
//!   what the catalogue holds is refused as [`CatalogueRefusal::StaleAnnouncement`]
//!   with both revisions, never silently dropped: offline catch-up has to be able
//!   to tell "nothing new" from "I am behind".
//! * **A required capability cannot be downgraded away.** Removing a capability the
//!   peer marks required — or one this client holds protected material for — is
//!   refused while that condition holds, so neither a peer nor a stale replica can
//!   quietly turn a protected conversation into an unprotected one.
//! * **Pending protected material stays recoverable.** Material recorded against a
//!   peer survives every catalogue change and leaves only through an explicit
//!   settlement of that material's own identity.
//!
//! Unknown *optional* capabilities are preserved as unknown and refuse nothing;
//! unknown *required* ones refuse the announcement, because a peer that requires
//! something this client cannot name is not a peer this client can serve.

use std::collections::{BTreeMap, BTreeSet};

/// One device identity. It is the peer's own device, not a session and not a group.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DeviceIdentity(String);

impl DeviceIdentity {
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One group identity. A group outlives any single device and is never re-owned by
/// whoever announces it last.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GroupIdentity(String);

impl GroupIdentity {
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One peer: a device *in* a group. The pair is the identity, and neither half
/// substitutes for the other.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PeerKey {
    device: DeviceIdentity,
    group: GroupIdentity,
}

impl PeerKey {
    #[must_use]
    pub const fn new(device: DeviceIdentity, group: GroupIdentity) -> Self {
        Self { device, group }
    }

    #[must_use]
    pub const fn device(&self) -> &DeviceIdentity {
        &self.device
    }

    #[must_use]
    pub const fn group(&self) -> &GroupIdentity {
        &self.group
    }
}

/// Whether a peer requires one capability or merely offers it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityRequirement {
    /// The conversation works without it. An unknown optional capability is kept
    /// and refuses nothing.
    Optional,
    /// The peer requires it. A client that cannot name it cannot serve the peer,
    /// so the announcement is refused rather than half-applied.
    Required,
}

/// One capability announcement from one peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityAnnouncement {
    pub peer: PeerKey,
    /// The peer's own monotonic revision for this peer entry.
    pub revision: u64,
    /// The device's identity rotation epoch, mirroring the directory's own
    /// `identityRotationEpoch`. It belongs to the device, not to one group:
    /// rotating a key does not make a new device, and the epoch never rewinds.
    pub rotation_epoch: u64,
    /// The capabilities, with the requirement the peer attaches to each.
    pub capabilities: BTreeMap<String, CapabilityRequirement>,
}

impl CapabilityAnnouncement {
    #[must_use]
    pub fn new(peer: PeerKey, revision: u64) -> Self {
        Self {
            peer,
            revision,
            rotation_epoch: 0,
            capabilities: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_rotation_epoch(mut self, rotation_epoch: u64) -> Self {
        self.rotation_epoch = rotation_epoch;
        self
    }

    #[must_use]
    pub fn with_capability(
        mut self,
        capability: impl Into<String>,
        requirement: CapabilityRequirement,
    ) -> Self {
        self.capabilities.insert(capability.into(), requirement);
        self
    }
}

/// What one announcement did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnnouncementOutcome {
    /// The catalogue now holds this revision.
    Recorded { revision: u64 },
    /// The revision was already held and the same capabilities were announced:
    /// offline catch-up re-delivers announcements, and a re-delivery is not news.
    Idempotent { revision: u64 },
}

/// Why the catalogue refused an announcement or a settlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogueRefusal {
    /// The announcement is older than what the catalogue holds. Both revisions are
    /// reported so a caller can re-synchronize instead of guessing.
    StaleAnnouncement { announced: u64, held: u64 },
    /// One group was announced by a different device than the one that owns it.
    GroupOwnerMismatch {
        group: String,
        owner: String,
        announced: String,
    },
    /// The device announced an identity rotation epoch older than the one the
    /// catalogue holds. Both epochs are reported so a caller can re-synchronize.
    RotationEpochRollback { announced: u64, held: u64 },
    /// The peer requires a capability this client cannot name.
    RequiredCapabilityUnknown { capability: String },
    /// The announcement would remove a capability the peer marks required.
    RequiredCapabilityRemoved { capability: String },
    /// The announcement would remove a capability this client still holds
    /// protected material for.
    ProtectedMaterialOutstanding { capability: String, material: usize },
    /// The granted capability was never announced, so there is nothing to grant.
    CapabilityNotAnnounced { capability: String },
}

impl CatalogueRefusal {
    /// The stable reason a caller publishes.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::StaleAnnouncement { .. } => "endpoint_capability_stale_announcement",
            Self::GroupOwnerMismatch { .. } => "endpoint_capability_group_owner_mismatch",
            Self::RotationEpochRollback { .. } => "endpoint_capability_rotation_epoch_rollback",
            Self::RequiredCapabilityUnknown { .. } => "endpoint_capability_required_unknown",
            Self::RequiredCapabilityRemoved { .. } => "endpoint_capability_required_removed",
            Self::ProtectedMaterialOutstanding { .. } => {
                "endpoint_capability_protected_material_outstanding"
            }
            Self::CapabilityNotAnnounced { .. } => "endpoint_capability_not_announced",
        }
    }
}

/// What this client may do with one capability of one peer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityAuthority {
    /// The peer never announced it.
    NotAnnounced,
    /// The peer announced it and this client has granted nothing. An announcement
    /// is what a peer says, and saying it grants nothing.
    AnnouncedNotAuthorized,
    /// This client granted it explicitly.
    Authorized,
}

/// One protected material reference outstanding against a peer.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProtectedMaterialRef {
    id: String,
    capability: String,
}

impl ProtectedMaterialRef {
    #[must_use]
    pub fn new(id: impl Into<String>, capability: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            capability: capability.into(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn capability(&self) -> &str {
        &self.capability
    }
}

#[derive(Clone, Debug, Default)]
struct PeerEntry {
    revision: u64,
    capabilities: BTreeMap<String, CapabilityRequirement>,
    authorized: BTreeSet<String>,
    protected_material: BTreeSet<ProtectedMaterialRef>,
}

/// The client's own catalogue of what its peers offer, and what it may do.
///
/// The capabilities this client can *name* are the ones installed in this build;
/// an announcement naming anything else is unknown, and whether that refuses
/// depends on the requirement the peer attached to it.
#[derive(Clone, Debug, Default)]
pub struct CapabilityCatalogue {
    known: BTreeSet<String>,
    groups: BTreeMap<String, DeviceIdentity>,
    /// The highest identity rotation epoch this client has accepted per device.
    /// It is a device-level fact and survives every group-level change.
    rotations: BTreeMap<DeviceIdentity, u64>,
    peers: BTreeMap<PeerKey, PeerEntry>,
}

impl CapabilityCatalogue {
    /// A catalogue of the capabilities this build can name.
    #[must_use]
    pub fn with_known_capabilities<I, S>(capabilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            known: capabilities.into_iter().map(Into::into).collect(),
            groups: BTreeMap::new(),
            rotations: BTreeMap::new(),
            peers: BTreeMap::new(),
        }
    }

    /// Record one announcement.
    ///
    /// Rotation rollback, staleness, required downgrades and outstanding protected
    /// material are all decided here, and a refusal leaves the catalogue exactly as
    /// it was.
    pub fn announce(
        &mut self,
        announcement: CapabilityAnnouncement,
    ) -> Result<AnnouncementOutcome, CatalogueRefusal> {
        if let Some(owner) = self.groups.get(announcement.peer.group().as_str())
            && owner != announcement.peer.device()
        {
            return Err(CatalogueRefusal::GroupOwnerMismatch {
                group: announcement.peer.group().as_str().to_owned(),
                owner: owner.as_str().to_owned(),
                announced: announcement.peer.device().as_str().to_owned(),
            });
        }
        // Rotation is a device-level fact: a device may present a newer epoch than
        // the catalogue holds, and never an older one, because the device it names
        // is the same device either way and its protected material must stay
        // reachable. The epoch is checked before the entry's own revision so a
        // rolled-back device is reported as a rollback rather than as staleness.
        let held_rotation = self
            .rotations
            .get(announcement.peer.device())
            .copied()
            .unwrap_or(0);
        if announcement.rotation_epoch < held_rotation {
            return Err(CatalogueRefusal::RotationEpochRollback {
                announced: announcement.rotation_epoch,
                held: held_rotation,
            });
        }
        let held = self
            .peers
            .get(&announcement.peer)
            .cloned()
            .unwrap_or_default();
        if announcement.revision < held.revision {
            return Err(CatalogueRefusal::StaleAnnouncement {
                announced: announcement.revision,
                held: held.revision,
            });
        }
        for (capability, requirement) in &announcement.capabilities {
            if *requirement == CapabilityRequirement::Required && !self.known.contains(capability) {
                return Err(CatalogueRefusal::RequiredCapabilityUnknown {
                    capability: capability.clone(),
                });
            }
        }
        let removed = held
            .capabilities
            .keys()
            .filter(|capability| !announcement.capabilities.contains_key(*capability))
            .cloned()
            .collect::<Vec<_>>();
        for capability in &removed {
            if held.capabilities.get(capability) == Some(&CapabilityRequirement::Required) {
                return Err(CatalogueRefusal::RequiredCapabilityRemoved {
                    capability: capability.clone(),
                });
            }
            let material = held
                .protected_material
                .iter()
                .filter(|reference| reference.capability() == capability)
                .count();
            if material > 0 {
                return Err(CatalogueRefusal::ProtectedMaterialOutstanding {
                    capability: capability.clone(),
                    material,
                });
            }
        }
        if announcement.revision == held.revision
            && held.capabilities == announcement.capabilities
            && announcement.rotation_epoch == held_rotation
        {
            return Ok(AnnouncementOutcome::Idempotent {
                revision: announcement.revision,
            });
        }

        self.groups
            .entry(announcement.peer.group().as_str().to_owned())
            .or_insert_with(|| announcement.peer.device().clone());
        // A rotation advances the device's epoch without touching the device
        // identity, so the peer keeps its entry, its grants and its material.
        self.rotations.insert(
            announcement.peer.device().clone(),
            announcement.rotation_epoch,
        );
        let entry = self.peers.entry(announcement.peer).or_default();
        entry.revision = announcement.revision;
        entry.capabilities = announcement.capabilities;
        // A grant for a capability the peer no longer offers is dropped with the
        // capability: this client does not keep authority over something the peer
        // stopped offering, and it never re-grants it by itself.
        let offered = entry.capabilities.keys().cloned().collect::<BTreeSet<_>>();
        entry
            .authorized
            .retain(|capability| offered.contains(capability));
        Ok(AnnouncementOutcome::Recorded {
            revision: entry.revision,
        })
    }

    /// Apply a batch of announcements, in the order given.
    ///
    /// This is the offline catch-up entry: every answer is returned, so a caller
    /// reports one stale or refused announcement instead of losing the batch.
    pub fn reconcile(
        &mut self,
        announcements: impl IntoIterator<Item = CapabilityAnnouncement>,
    ) -> Vec<Result<AnnouncementOutcome, CatalogueRefusal>> {
        announcements
            .into_iter()
            .map(|announcement| self.announce(announcement))
            .collect()
    }

    /// What this client may do with one capability of one peer.
    #[must_use]
    pub fn authority(&self, peer: &PeerKey, capability: &str) -> CapabilityAuthority {
        let Some(entry) = self.peers.get(peer) else {
            return CapabilityAuthority::NotAnnounced;
        };
        if !entry.capabilities.contains_key(capability) {
            return CapabilityAuthority::NotAnnounced;
        }
        if entry.authorized.contains(capability) {
            CapabilityAuthority::Authorized
        } else {
            CapabilityAuthority::AnnouncedNotAuthorized
        }
    }

    /// Grant this client authority over one announced capability.
    ///
    /// The grant is local and explicit: it is the only way [`Self::authority`]
    /// answers `Authorized`, and no announcement produces one.
    pub fn grant_authority(
        &mut self,
        peer: &PeerKey,
        capability: &str,
    ) -> Result<(), CatalogueRefusal> {
        let Some(entry) = self.peers.get_mut(peer) else {
            return Err(CatalogueRefusal::CapabilityNotAnnounced {
                capability: capability.to_owned(),
            });
        };
        if !entry.capabilities.contains_key(capability) {
            return Err(CatalogueRefusal::CapabilityNotAnnounced {
                capability: capability.to_owned(),
            });
        }
        entry.authorized.insert(capability.to_owned());
        Ok(())
    }

    /// Withdraw one local grant. The peer's announcement is untouched.
    pub fn withdraw_authority(&mut self, peer: &PeerKey, capability: &str) {
        if let Some(entry) = self.peers.get_mut(peer) {
            entry.authorized.remove(capability);
        }
    }

    /// Record protected material this client owes a peer a boundary for.
    pub fn record_protected_material(
        &mut self,
        peer: &PeerKey,
        reference: ProtectedMaterialRef,
    ) -> bool {
        self.peers
            .entry(peer.clone())
            .or_default()
            .protected_material
            .insert(reference)
    }

    /// The protected material outstanding against one peer.
    #[must_use]
    pub fn protected_material(&self, peer: &PeerKey) -> Vec<&ProtectedMaterialRef> {
        self.peers
            .get(peer)
            .map(|entry| entry.protected_material.iter().collect())
            .unwrap_or_default()
    }

    /// Settle exactly one protected material reference.
    ///
    /// Nothing else removes material: not an announcement, not a revision bump and
    /// not a lost session, which is what keeps it recoverable across every
    /// catalogue change.
    pub fn settle_protected_material(&mut self, peer: &PeerKey, id: &str) -> bool {
        let Some(entry) = self.peers.get_mut(peer) else {
            return false;
        };
        let before = entry.protected_material.len();
        entry
            .protected_material
            .retain(|reference| reference.id() != id);
        entry.protected_material.len() != before
    }

    /// The revision the catalogue holds for one peer.
    #[must_use]
    pub fn revision(&self, peer: &PeerKey) -> Option<u64> {
        self.peers.get(peer).map(|entry| entry.revision)
    }

    /// The identity rotation epoch the catalogue holds for one device, once the
    /// device has announced one.
    #[must_use]
    pub fn rotation_epoch(&self, device: &DeviceIdentity) -> Option<u64> {
        self.rotations.get(device).copied()
    }

    /// The device that owns one group, once one is known.
    #[must_use]
    pub fn group_owner(&self, group: &GroupIdentity) -> Option<&DeviceIdentity> {
        self.groups.get(group.as_str())
    }

    /// The capabilities one peer announced, with their requirements.
    #[must_use]
    pub fn announced(&self, peer: &PeerKey) -> Option<&BTreeMap<String, CapabilityRequirement>> {
        self.peers.get(peer).map(|entry| &entry.capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AnnouncementOutcome, CapabilityAnnouncement, CapabilityAuthority, CapabilityCatalogue,
        CapabilityRequirement, CatalogueRefusal, DeviceIdentity, GroupIdentity, PeerKey,
        ProtectedMaterialRef,
    };

    const RELAY: &str = "endpoint.collaboration.v1";
    const TRANSFER: &str = "endpoint.transfer.v1";

    fn catalogue() -> CapabilityCatalogue {
        CapabilityCatalogue::with_known_capabilities([RELAY, TRANSFER])
    }

    fn peer(device: &str, group: &str) -> PeerKey {
        PeerKey::new(DeviceIdentity::new(device), GroupIdentity::new(group))
    }

    fn announcement(
        peer: PeerKey,
        revision: u64,
        revision_capabilities: &[&str],
    ) -> CapabilityAnnouncement {
        let mut announcement = CapabilityAnnouncement::new(peer, revision);
        for capability in revision_capabilities {
            announcement =
                announcement.with_capability(*capability, CapabilityRequirement::Optional);
        }
        announcement
    }

    #[test]
    fn an_announcement_records_what_a_peer_offers_and_grants_nothing() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 1, &[RELAY])),
            Ok(AnnouncementOutcome::Recorded { revision: 1 })
        );
        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::AnnouncedNotAuthorized,
            "a peer's own claim is not this client's authority"
        );
        assert_eq!(
            catalogue.authority(&peer, TRANSFER),
            CapabilityAuthority::NotAnnounced
        );
    }

    #[test]
    fn only_an_explicit_local_grant_produces_authority() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY]))
            .expect("recorded");

        assert_eq!(
            catalogue.grant_authority(&peer, TRANSFER),
            Err(CatalogueRefusal::CapabilityNotAnnounced {
                capability: TRANSFER.to_owned()
            })
        );
        assert_eq!(catalogue.grant_authority(&peer, RELAY), Ok(()));
        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::Authorized
        );

        // A later announcement does not re-grant, and it does not revoke either.
        catalogue
            .announce(announcement(peer.clone(), 2, &[RELAY, TRANSFER]))
            .expect("recorded");
        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::Authorized
        );
        assert_eq!(
            catalogue.authority(&peer, TRANSFER),
            CapabilityAuthority::AnnouncedNotAuthorized
        );
    }

    #[test]
    fn a_stale_announcement_is_an_explicit_answer_not_a_silent_drop() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 5, &[RELAY]))
            .expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 4, &[RELAY])),
            Err(CatalogueRefusal::StaleAnnouncement {
                announced: 4,
                held: 5
            })
        );
        assert_eq!(catalogue.revision(&peer), Some(5));
        let refusal = catalogue
            .announce(announcement(peer.clone(), 4, &[RELAY]))
            .expect_err("still stale")
            .reason();
        assert_eq!(refusal, "endpoint_capability_stale_announcement");
    }

    #[test]
    fn re_delivering_the_same_revision_is_idempotent() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 3, &[RELAY]))
            .expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 3, &[RELAY])),
            Ok(AnnouncementOutcome::Idempotent { revision: 3 })
        );
    }

    #[test]
    fn a_device_and_a_group_are_separate_identities() {
        let mut catalogue = catalogue();
        let first = peer("device-a", "group-1");
        let second = peer("device-a", "group-2");
        catalogue
            .announce(announcement(first.clone(), 1, &[RELAY]))
            .expect("recorded");
        catalogue
            .announce(announcement(second.clone(), 1, &[TRANSFER]))
            .expect("recorded");

        assert_eq!(catalogue.revision(&first), Some(1));
        assert_eq!(catalogue.revision(&second), Some(1));
        assert_eq!(
            catalogue.authority(&first, TRANSFER),
            CapabilityAuthority::NotAnnounced,
            "the same device in another group is another peer"
        );
        assert_eq!(
            catalogue
                .group_owner(&GroupIdentity::new("group-1"))
                .map(DeviceIdentity::as_str),
            Some("device-a")
        );
    }

    #[test]
    fn a_group_is_not_re_owned_by_another_device() {
        let mut catalogue = catalogue();
        catalogue
            .announce(announcement(peer("device-a", "group-1"), 1, &[RELAY]))
            .expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(peer("device-b", "group-1"), 1, &[RELAY])),
            Err(CatalogueRefusal::GroupOwnerMismatch {
                group: "group-1".to_owned(),
                owner: "device-a".to_owned(),
                announced: "device-b".to_owned()
            })
        );
    }

    #[test]
    fn a_required_capability_cannot_be_downgraded_away() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        let required = CapabilityAnnouncement::new(peer.clone(), 1)
            .with_capability(RELAY, CapabilityRequirement::Required)
            .with_capability(TRANSFER, CapabilityRequirement::Optional);
        catalogue.announce(required).expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 2, &[TRANSFER])),
            Err(CatalogueRefusal::RequiredCapabilityRemoved {
                capability: RELAY.to_owned()
            })
        );
        assert_eq!(
            catalogue.announced(&peer).map(|announced| announced.len()),
            Some(2),
            "the refused downgrade changed nothing"
        );
    }

    #[test]
    fn unknown_optional_is_preserved_and_unknown_required_refuses() {
        let mut catalogue = catalogue();
        let known_peer = peer("device-a", "group-1");
        let optional = CapabilityAnnouncement::new(known_peer.clone(), 1)
            .with_capability("endpoint.future.v9", CapabilityRequirement::Optional);
        catalogue.announce(optional).expect("optional is preserved");

        assert_eq!(
            catalogue.authority(&known_peer, "endpoint.future.v9"),
            CapabilityAuthority::AnnouncedNotAuthorized,
            "an unknown optional capability is kept as an announcement"
        );

        let other = peer("device-b", "group-2");
        let required = CapabilityAnnouncement::new(other.clone(), 1)
            .with_capability("endpoint.future.v9", CapabilityRequirement::Required);
        assert_eq!(
            catalogue.announce(required),
            Err(CatalogueRefusal::RequiredCapabilityUnknown {
                capability: "endpoint.future.v9".to_owned()
            })
        );
        assert_eq!(catalogue.revision(&other), None);
    }

    #[test]
    fn pending_protected_material_survives_catalogue_changes_and_refuses_removal() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY, TRANSFER]))
            .expect("recorded");
        assert!(
            catalogue
                .record_protected_material(&peer, ProtectedMaterialRef::new("material-1", RELAY))
        );

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 2, &[TRANSFER])),
            Err(CatalogueRefusal::ProtectedMaterialOutstanding {
                capability: RELAY.to_owned(),
                material: 1
            }),
            "material protected with a capability blocks removing that capability"
        );
        assert_eq!(
            catalogue.protected_material(&peer).len(),
            1,
            "the refused announcement changed nothing"
        );

        // An unrelated update keeps the material, and only its own settlement
        // removes it.
        catalogue
            .announce(announcement(peer.clone(), 3, &[RELAY, TRANSFER]))
            .expect("recorded");
        assert_eq!(catalogue.protected_material(&peer).len(), 1);
        assert!(!catalogue.settle_protected_material(&peer, "material-2"));
        assert!(catalogue.settle_protected_material(&peer, "material-1"));
        assert!(catalogue.protected_material(&peer).is_empty());

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 4, &[TRANSFER])),
            Ok(AnnouncementOutcome::Recorded { revision: 4 }),
            "with the material settled the capability may be removed"
        );
    }

    #[test]
    fn offline_catch_up_reports_every_answer_including_the_refusals() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 5, &[RELAY]))
            .expect("recorded");

        let answers = catalogue.reconcile([
            announcement(peer.clone(), 4, &[RELAY]),
            announcement(peer.clone(), 6, &[RELAY, TRANSFER]),
            announcement(peer.clone(), 6, &[RELAY, TRANSFER]),
        ]);

        assert_eq!(answers.len(), 3);
        assert!(matches!(
            answers[0],
            Err(CatalogueRefusal::StaleAnnouncement { .. })
        ));
        assert_eq!(
            answers[1],
            Ok(AnnouncementOutcome::Recorded { revision: 6 })
        );
        assert_eq!(
            answers[2],
            Ok(AnnouncementOutcome::Idempotent { revision: 6 })
        );
        assert_eq!(catalogue.revision(&peer), Some(6));
    }

    #[test]
    fn withdrawing_a_local_grant_leaves_the_peers_announcement_alone() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY]))
            .expect("recorded");
        catalogue.grant_authority(&peer, RELAY).expect("announced");

        catalogue.withdraw_authority(&peer, RELAY);

        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::AnnouncedNotAuthorized
        );
        assert_eq!(catalogue.revision(&peer), Some(1));
    }

    #[test]
    fn removing_a_capability_drops_the_grant_it_carried() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY, TRANSFER]))
            .expect("recorded");
        catalogue.grant_authority(&peer, RELAY).expect("announced");

        catalogue
            .announce(announcement(peer.clone(), 2, &[TRANSFER]))
            .expect("recorded");

        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::NotAnnounced,
            "authority over a capability the peer stopped offering is not retained"
        );
        assert_eq!(
            catalogue.authority(&peer, TRANSFER),
            CapabilityAuthority::AnnouncedNotAuthorized
        );
    }

    #[test]
    fn a_rotation_keeps_the_device_its_grants_and_its_material() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY, TRANSFER]))
            .expect("recorded");
        catalogue.grant_authority(&peer, RELAY).expect("announced");
        assert!(
            catalogue
                .record_protected_material(&peer, ProtectedMaterialRef::new("material-1", RELAY))
        );

        catalogue
            .announce(announcement(peer.clone(), 2, &[RELAY, TRANSFER]).with_rotation_epoch(1))
            .expect("a newer epoch is a rotation, not a new peer");

        assert_eq!(
            catalogue.rotation_epoch(&DeviceIdentity::new("device-a")),
            Some(1),
            "the device's epoch moved"
        );
        assert_eq!(
            catalogue.revision(&peer),
            Some(2),
            "the peer entry is the same device, not a rebuilt one"
        );
        assert_eq!(
            catalogue.authority(&peer, RELAY),
            CapabilityAuthority::Authorized,
            "the rotation did not re-grant and did not revoke"
        );
        assert_eq!(
            catalogue.protected_material(&peer).len(),
            1,
            "material committed before the rotation stays recoverable"
        );
        assert_eq!(
            catalogue
                .group_owner(&GroupIdentity::new("group-1"))
                .map(DeviceIdentity::as_str),
            Some("device-a")
        );
    }

    #[test]
    fn a_rotation_epoch_rollback_is_refused_with_both_epochs() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY]).with_rotation_epoch(2))
            .expect("recorded");

        let refusal = catalogue
            .announce(announcement(peer.clone(), 2, &[RELAY]).with_rotation_epoch(1))
            .expect_err("a rolled-back device is refused, not silently accepted");

        assert_eq!(
            refusal,
            CatalogueRefusal::RotationEpochRollback {
                announced: 1,
                held: 2
            }
        );
        assert_eq!(
            refusal.reason(),
            "endpoint_capability_rotation_epoch_rollback"
        );
        assert_eq!(
            catalogue.revision(&peer),
            Some(1),
            "the refused rollback changed nothing"
        );
        assert_eq!(
            catalogue.rotation_epoch(&DeviceIdentity::new("device-a")),
            Some(2)
        );
    }

    #[test]
    fn the_rotation_epoch_belongs_to_the_device_and_not_to_one_group() {
        let mut catalogue = catalogue();
        let first = peer("device-a", "group-1");
        let second = peer("device-a", "group-2");
        catalogue
            .announce(announcement(first.clone(), 1, &[RELAY]).with_rotation_epoch(3))
            .expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(second.clone(), 1, &[TRANSFER]).with_rotation_epoch(2)),
            Err(CatalogueRefusal::RotationEpochRollback {
                announced: 2,
                held: 3
            }),
            "the same device in another group cannot rewind its own epoch"
        );
        assert_eq!(catalogue.revision(&second), None);

        catalogue
            .announce(announcement(second, 1, &[TRANSFER]).with_rotation_epoch(4))
            .expect("an advancing epoch is accepted in the other group too");
        assert_eq!(
            catalogue.rotation_epoch(&DeviceIdentity::new("device-a")),
            Some(4)
        );
    }

    #[test]
    fn an_unchanged_announcement_with_a_newer_epoch_is_recorded_not_ignored() {
        let mut catalogue = catalogue();
        let peer = peer("device-a", "group-1");
        catalogue
            .announce(announcement(peer.clone(), 1, &[RELAY]))
            .expect("recorded");

        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 1, &[RELAY]).with_rotation_epoch(1)),
            Ok(AnnouncementOutcome::Recorded { revision: 1 }),
            "the same capabilities at a newer epoch are a rotation, not a re-delivery"
        );
        assert_eq!(
            catalogue.announce(announcement(peer.clone(), 1, &[RELAY]).with_rotation_epoch(1)),
            Ok(AnnouncementOutcome::Idempotent { revision: 1 }),
            "re-delivering that same rotation is not news"
        );
    }
}
