//! The cross-device production entry.
//!
//! Three things already existed in this host with no production caller: the
//! caller-owned port spine [`ProtocolWorkSetup`], the verified-unit mapping in
//! [`endpoint_transport`](crate::domain::mobile_relay::endpoint_transport), and
//! the conversation admission in
//! [`PeerIngress`](crate::domain::client_conversation::peer_ingress::PeerIngress).
//! Each was reachable only from its own module's fixtures. This module is the
//! entry that joins them once, so the native route, the composition root, and
//! the device layer call one owner instead of three loose types.
//!
//! # The two constraints this entry exists to hold
//!
//! **The verification record precedes the conversation row.** One inbound unit
//! is recorded as a [`VerificationRecord`] first; only a unit whose record is
//! already in the ledger can be written into the Canonical Conversation. The
//! order is not bookkeeping: a crash between the two leaves a recorded
//! verification with no row, which is re-drivable, while a row with no recorded
//! verification is a message no verified unit stands behind.
//!
//! **A replay is never admitted again.** The ledger is keyed by the SDK's own
//! replay identity, so a byte-identical replay of one committed protected record
//! is refused before any conversation write. A re-protected resend of the same
//! logical message carries a different replay identity, so it reaches the
//! ingress and merges there as a duplicate instead of becoming a second row.
//!
//! Both constraints are falsifiable from the public surface of this module; the
//! integration suite drives them, and the mutations that invert the order or
//! remove the identity check fail it.
//!
//! # What this entry deliberately does not do
//!
//! It creates no conversation, no principal, no membership, and no
//! administrator; the binding table below only resolves a verified device to a
//! membership that already exists. It executes no effect: a structured command
//! request leaves as an [`PeerEffectIntent`](crate::domain::client_conversation::peer_ingress::PeerEffectIntent)
//! for the single-owner application entry.
//!
//! [`ProtocolWorkSetup`] is admitted through [`admit_protocol_work`], which adds
//! no policy: the pinned SDK decides whether the supplied artifact is the fixed
//! Candidate this build integrates, and a refusal returns every port untouched.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use licoup_conversation::ConversationStore;
use licoup_endpoint_core::{
    AtomicState as AtomicStatePort, Clock as ClockPort, CustodyHandle, CustodyLifecycle,
    CustodyPurpose, KeyCustody as KeyCustodyPort, PortFailure, Revision as PortRevision,
    StateCommit as PortStateCommit, Transport as TransportPort, TransportOutcome,
    Versioned as PortVersioned,
};
use licoup_protocol_bindings::endpoint::EndpointState;
use licoup_protocol_bindings::state::PendingItem as SdkPendingItem;
use licoup_protocol_bindings::{ReplayIdentity, TrustFacts};
use serde_json::Value;

use crate::domain::client_conversation::peer_ingress::{
    AdmissionFact, PeerBinding, PeerBindings, PeerIngress, PeerIngressRefusal, PeerIngressReport,
};
use crate::domain::mobile_relay::endpoint_transport::{
    MAX_STATION_ID_BYTES, MessageForwarder, PeerAuthor, PeerDevice, PeerMessage, PeerMessageBody,
    PeerPart, PeerUnitRefusal, StationHint, StationRef, VerifiedUnit,
};
use crate::domain::mobile_relay::{
    AdmittedProtocolWork, CustodyHandles, ProtocolAdmissionRefusal, ProtocolPorts,
    ProtocolWorkSetup,
};

/// Largest number of verification records one ingress lifecycle remembers.
///
/// The window is bounded and oldest-first, so a long session cannot grow the
/// record set without limit. Exceeding it does not admit a replay silently: the
/// oldest identity leaves the window, and the SDK's own replay refusal remains
/// the authority for a unit whose identity is no longer remembered here.
pub const MAX_VERIFICATION_RECORDS: usize = 4096;

/// One refused step of the cross-device entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrossDeviceRefusal {
    /// No process entry was installed, so nothing may be admitted.
    EntryUnavailable,
    /// The unit's own replay identity is already recorded: a replay.
    ReplayRefused,
    /// The conversation write was reached without a recorded verification.
    VerificationNotRecorded,
    /// The verified unit could not be mapped into a peer message.
    Unit(PeerUnitRefusal),
    /// The host's conversation owner refused the admission.
    Ingress(PeerIngressRefusal),
    /// No device edge is attached, so no packet can be verified here.
    EndpointUnavailable,
    /// The caller's request did not describe a message this entry can carry.
    MalformedRequest(&'static str),
}

impl CrossDeviceRefusal {
    /// Stable class of this refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::EntryUnavailable => "cross_device_entry_unavailable",
            Self::ReplayRefused => "cross_device_replay_refused",
            Self::VerificationNotRecorded => "cross_device_verification_not_recorded",
            Self::Unit(refusal) => refusal.code(),
            Self::Ingress(refusal) => refusal.code(),
            Self::EndpointUnavailable => "cross_device_endpoint_unavailable",
            Self::MalformedRequest(code) => code,
        }
    }
}

impl core::fmt::Display for CrossDeviceRefusal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CrossDeviceRefusal {}

impl From<PeerUnitRefusal> for CrossDeviceRefusal {
    fn from(refusal: PeerUnitRefusal) -> Self {
        Self::Unit(refusal)
    }
}

impl From<PeerIngressRefusal> for CrossDeviceRefusal {
    fn from(refusal: PeerIngressRefusal) -> Self {
        Self::Ingress(refusal)
    }
}

/// One recorded verified unit.
///
/// `ordinal` is this ledger's own strictly increasing position. It is the
/// ordering evidence the entry reports, and it names nothing the protocol
/// decided: the SDK's own identity stays in `identity`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationRecord<RecordKey> {
    identity: RecordKey,
    ordinal: u64,
}

impl<RecordKey> VerificationRecord<RecordKey> {
    /// This ledger's strictly increasing position of the record.
    #[must_use]
    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// The SDK's own identity of the unit this record stands for.
    #[must_use]
    pub const fn identity(&self) -> &RecordKey {
        &self.identity
    }
}

/// The verification records this entry holds, oldest first.
///
/// Classification and recording are separate for the same reason the ingress's
/// intake window separates them: an entry reads the ledger to decide, and only a
/// successfully recorded verification changes it.
#[derive(Debug)]
pub struct VerificationLedger<RecordKey> {
    records: VecDeque<VerificationRecord<RecordKey>>,
    next_ordinal: u64,
}

impl<RecordKey> VerificationLedger<RecordKey> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            next_ordinal: 1,
        }
    }

    /// The ordinal the next recorded verification will carry.
    #[must_use]
    pub const fn next_ordinal(&self) -> u64 {
        self.next_ordinal
    }

    /// Every record still in the window, oldest first.
    #[must_use]
    pub fn records(&self) -> impl ExactSizeIterator<Item = &VerificationRecord<RecordKey>> {
        self.records.iter()
    }

    /// Number of remembered records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl<RecordKey> Default for VerificationLedger<RecordKey> {
    fn default() -> Self {
        Self::new()
    }
}

impl<RecordKey: Clone + Eq> VerificationLedger<RecordKey> {
    /// Whether this exact unit already has a recorded verification.
    #[must_use]
    pub fn contains(&self, identity: &RecordKey) -> bool {
        self.records
            .iter()
            .any(|record| &record.identity == identity)
    }

    /// The record of one already-verified unit, when it is still in the window.
    #[must_use]
    pub fn record_of(&self, identity: &RecordKey) -> Option<&VerificationRecord<RecordKey>> {
        self.records
            .iter()
            .find(|record| &record.identity == identity)
    }

    /// Appends one record and returns it.
    ///
    /// The caller decides whether the unit may be recorded; this method only
    /// appends, so a refusal above leaves the ledger exactly as it was.
    fn append(&mut self, identity: RecordKey) -> VerificationRecord<RecordKey> {
        let ordinal = self.next_ordinal;
        self.next_ordinal += 1;
        if self.records.len() == MAX_VERIFICATION_RECORDS {
            self.records.pop_front();
        }
        let record = VerificationRecord { identity, ordinal };
        self.records.push_back(record.clone());
        record
    }
}

/// One entry of the host's binding table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingEntry {
    author: [u8; 32],
    device: [u8; 32],
    binding: PeerBinding,
}

impl BindingEntry {
    #[must_use]
    pub const fn author(&self) -> [u8; 32] {
        self.author
    }

    #[must_use]
    pub const fn device(&self) -> [u8; 32] {
        self.device
    }

    #[must_use]
    pub const fn binding(&self) -> &PeerBinding {
        &self.binding
    }
}

/// The host's binding table: which pre-existing membership a verified device
/// writes as.
///
/// Revocation is expressed by removing the entry, exactly as the port's own
/// contract requires; nothing here creates a principal, a membership, or an
/// administrator.
#[derive(Debug, Default)]
pub struct HostPeerBindings {
    entries: Mutex<Vec<BindingEntry>>,
}

impl HostPeerBindings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one binding, replacing any earlier binding of the same device.
    pub fn bind(&self, author: [u8; 32], device: [u8; 32], binding: PeerBinding) {
        let mut entries = self.lock();
        entries.retain(|entry| entry.author != author || entry.device != device);
        entries.push(BindingEntry {
            author,
            device,
            binding,
        });
    }

    /// Removes one binding, reporting whether one was there.
    pub fn revoke(&self, author: &[u8; 32], device: &[u8; 32]) -> bool {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|entry| &entry.author != author || &entry.device != device);
        entries.len() != before
    }

    /// Every binding currently trusted, for a client that projects them.
    #[must_use]
    pub fn entries(&self) -> Vec<BindingEntry> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<BindingEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl PeerBindings for HostPeerBindings {
    fn resolve(&self, author: &PeerAuthor, device: &PeerDevice) -> Option<PeerBinding> {
        let author = author.user_authority_state_digest();
        let device = device.identity_state_digest();
        self.lock()
            .iter()
            .find(|entry| entry.author == author && entry.device == device)
            .map(|entry| entry.binding.clone())
    }
}

/// What one admitted unit produced, with the verified/recorded ordering the
/// entry is responsible for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestReceipt<RecordKey> {
    verification: VerificationRecord<RecordKey>,
    admission: AdmissionFact,
    logical_id: [u8; 16],
    effect_intents: usize,
    station: Option<StationHint>,
}

impl<RecordKey> IngestReceipt<RecordKey> {
    /// The recorded verification that authorized the conversation row.
    #[must_use]
    pub const fn verification(&self) -> &VerificationRecord<RecordKey> {
        &self.verification
    }

    /// The local event this unit became, or the duplicate it merged into.
    #[must_use]
    pub const fn admission(&self) -> &AdmissionFact {
        &self.admission
    }

    /// The peer's own logical identity of the message.
    #[must_use]
    pub const fn logical_id(&self) -> [u8; 16] {
        self.logical_id
    }

    /// How many structured command requests the message carried. A duplicate
    /// carries none, because merging a message is not a second execution grant.
    #[must_use]
    pub const fn effect_intents(&self) -> usize {
        self.effect_intents
    }

    /// The station's read-only report, carried through untouched.
    #[must_use]
    pub const fn station(&self) -> Option<StationHint> {
        self.station
    }
}

/// The device edge: the one operator this entry cannot supply for itself.
///
/// Implementing this is the device layer's job, because verifying and durably
/// committing an inbound protected record is the pinned SDK's own entry over
/// caller-owned custody, state, and transport ports. The entry holds the
/// operator rather than the ports so a device that is not attached refuses
/// ([`CrossDeviceRefusal::EndpointUnavailable`]) instead of admitting a unit no
/// verification entry produced.
pub trait CrossDeviceEdge: Send + Sync {
    /// Verifies and durably commits one inbound protected record.
    ///
    /// The returned facts must be the pinned SDK's own `TrustFacts` value of the
    /// committed record (`InboundSession::receive_record`); a value that
    /// describes an accepted handshake carries no record identity and is
    /// refused by the mapping above.
    fn receive_record(&self, packet: &[u8]) -> Result<TrustFacts, CrossDeviceRefusal>;
}

/// The cross-device ingress: bindings, verification records, and the
/// conversation admission they authorize.
///
/// `RecordKey` is the SDK boundary's own replay identity of one committed
/// protected record, carried through unchanged. It is generic for the same
/// reason [`PeerIngress`] and
/// [`PeerMessage`](crate::domain::mobile_relay::endpoint_transport::PeerMessage)
/// are: the production instantiation is the SDK's
/// [`ReplayIdentity`], and a fixture can exercise this exact code with a key it
/// can name without the pinned artifact.
pub struct CrossDeviceEntry<RecordKey> {
    bindings: Arc<HostPeerBindings>,
    ledger: VerificationLedger<RecordKey>,
    ingress: PeerIngress<RecordKey>,
    edge: Option<Arc<dyn CrossDeviceEdge>>,
}

impl<RecordKey: Clone + Eq> CrossDeviceEntry<RecordKey> {
    /// Builds the entry over the host's binding table.
    #[must_use]
    pub fn new(bindings: Arc<HostPeerBindings>) -> Self {
        let port: Arc<dyn PeerBindings> = bindings.clone();
        Self {
            bindings,
            ledger: VerificationLedger::new(),
            ingress: PeerIngress::new(port),
            edge: None,
        }
    }

    /// The host's binding table, so the local client can bind and revoke.
    #[must_use]
    pub const fn bindings(&self) -> &Arc<HostPeerBindings> {
        &self.bindings
    }

    /// The verification records this entry holds.
    #[must_use]
    pub const fn ledger(&self) -> &VerificationLedger<RecordKey> {
        &self.ledger
    }

    /// Attaches the device edge that verifies and commits inbound packets.
    pub fn attach_edge(&mut self, edge: Arc<dyn CrossDeviceEdge>) {
        self.edge = Some(edge);
    }

    /// Whether a device edge is attached.
    #[must_use]
    pub fn edge_attached(&self) -> bool {
        self.edge.is_some()
    }
}

impl<RecordKey: Clone + Eq> CrossDeviceEntry<RecordKey> {
    /// Admits one SDK-verified unit into the conversation it is bound to.
    ///
    /// The sequence is the constraint, in this order:
    ///
    /// 1. the verified unit is mapped into a peer message (which refuses a
    ///    handshake, because a handshake bound no record);
    /// 2. its verification is recorded, and a replay identity that is already
    ///    recorded is refused;
    /// 3. the conversation row is written, and only for a unit whose record is
    ///    already in the ledger.
    pub fn ingest(
        &mut self,
        store: &ConversationStore,
        unit: &impl VerifiedUnit<RecordKey = RecordKey>,
        forwarder: MessageForwarder,
        body: PeerMessageBody,
        station: Option<StationHint>,
    ) -> Result<IngestReceipt<RecordKey>, CrossDeviceRefusal> {
        let message = self.map_unit(unit, forwarder, body, station)?;
        let verification = self.record_verification(message.replay_identity().clone())?;
        let report = self.admit_recorded(store, &message)?;
        Ok(receipt(verification, &report))
    }

    /// Writes the conversation row for a unit whose verification is already
    /// recorded, appending no second record.
    ///
    /// A process that recorded a verification and stopped before the row leaves
    /// the unit recorded and unwritten; re-driving it is how that unit reaches
    /// the conversation without a duplicate record. The gate is the same one
    /// [`CrossDeviceEntry::ingest`] passes through, so a unit that was never
    /// recorded is refused here too.
    pub fn redrive(
        &mut self,
        store: &ConversationStore,
        unit: &impl VerifiedUnit<RecordKey = RecordKey>,
        forwarder: MessageForwarder,
        body: PeerMessageBody,
        station: Option<StationHint>,
    ) -> Result<IngestReceipt<RecordKey>, CrossDeviceRefusal> {
        let message = self.map_unit(unit, forwarder, body, station)?;
        // No second record, and no write before the record is found.
        let verification = self
            .ledger
            .record_of(message.replay_identity())
            .cloned()
            .ok_or(CrossDeviceRefusal::VerificationNotRecorded)?;
        let report = self.admit_recorded(store, &message)?;
        Ok(receipt(verification, &report))
    }

    /// The one mapping every door uses: verified facts and the caller's decoded
    /// body into a peer message, with the station's read-only report attached
    /// when the caller carried one.
    fn map_unit(
        &self,
        unit: &impl VerifiedUnit<RecordKey = RecordKey>,
        forwarder: MessageForwarder,
        body: PeerMessageBody,
        station: Option<StationHint>,
    ) -> Result<PeerMessage<RecordKey>, CrossDeviceRefusal> {
        let mut message = PeerMessage::of_verified(unit, forwarder, body)?;
        if let Some(hint) = station {
            message = message.with_station_hint(hint);
        }
        Ok(message)
    }

    /// Records one verified unit, refusing an identity that is already there.
    ///
    /// The record lands before any conversation write, and a replay never gets a
    /// second record, so it can never reach a second row.
    fn record_verification(
        &mut self,
        identity: RecordKey,
    ) -> Result<VerificationRecord<RecordKey>, CrossDeviceRefusal> {
        if self.ledger.contains(&identity) {
            return Err(CrossDeviceRefusal::ReplayRefused);
        }
        Ok(self.ledger.append(identity))
    }

    /// Writes the conversation row for an already-recorded unit.
    ///
    /// A unit whose verification is not in the ledger is refused rather than
    /// written and reconciled later: the row would name a message no recorded
    /// verification stands behind.
    fn admit_recorded(
        &mut self,
        store: &ConversationStore,
        message: &PeerMessage<RecordKey>,
    ) -> Result<PeerIngressReport, CrossDeviceRefusal> {
        if !self.ledger.contains(message.replay_identity()) {
            return Err(CrossDeviceRefusal::VerificationNotRecorded);
        }
        self.ingress.admit(store, message).map_err(Into::into)
    }
}

/// Builds the receipt one admitted report produced, beside its recorded
/// verification.
fn receipt<RecordKey>(
    verification: VerificationRecord<RecordKey>,
    report: &PeerIngressReport,
) -> IngestReceipt<RecordKey> {
    IngestReceipt {
        verification,
        admission: report.admission().clone(),
        logical_id: report.message_id(),
        effect_intents: report.effect_intents().len(),
        station: report.station(),
    }
}

impl CrossDeviceEntry<ReplayIdentity> {
    /// Verifies one inbound packet through the attached device edge and admits
    /// the unit it committed.
    ///
    /// This is the production packet door: the native route hands it the bytes
    /// the transport carried, and nothing about the resulting message comes from
    /// those bytes except the peer's own decoded body.
    pub fn record_packet(
        &mut self,
        store: &ConversationStore,
        packet: &[u8],
        forwarder: MessageForwarder,
        body: PeerMessageBody,
        station: Option<StationHint>,
    ) -> Result<IngestReceipt<ReplayIdentity>, CrossDeviceRefusal> {
        let edge = self
            .edge
            .clone()
            .ok_or(CrossDeviceRefusal::EndpointUnavailable)?;
        let facts = edge.receive_record(packet)?;
        self.ingest(store, &facts, forwarder, body, station)
    }

    /// The accepted-version freeze this process admitted, once a device edge is
    /// attached and has admitted one.
    ///
    /// Reported as a value rather than a promise: without an attached edge this
    /// entry has admitted nothing and answers `None`.
    #[must_use]
    pub fn edge(&self) -> Option<&Arc<dyn CrossDeviceEdge>> {
        self.edge.as_ref()
    }
}

// ---------------------------------------------------------------------------
// The caller-owned port spine's production entry point
// ---------------------------------------------------------------------------

/// Admits one protocol work set for this process.
///
/// This is the composition root's own call into [`ProtocolWorkSetup`]: the
/// caller supplies its ports and the exact authority bytes, and the pinned SDK
/// decides whether those bytes are the fixed Candidate this build integrates.
/// Constructing the setup performs no side effect, and a refusal hands every
/// port straight back through [`ProtocolWorkSetup::ports`], so a refused
/// artifact produces no write and no network I/O.
pub fn admit_protocol_work<C, H, A, K, T>(
    authority: Vec<u8>,
    custody: C,
    handles: H,
    state: A,
    clock: K,
    transport: T,
) -> Result<AdmittedProtocolWork<C, H, A, K, T>, ProtocolAdmissionRefusal> {
    ProtocolWorkSetup::new(authority, custody, handles, state, clock, transport).admit()
}

/// The same ports with no authority attached, for the branches that need none.
///
/// A caller that has no authority artifact yet still owns its ports; this is the
/// production path that reads them back without admitting anything.
pub fn unadmitted_protocol_ports<C, H, A, K, T>(
    authority: Vec<u8>,
    custody: C,
    handles: H,
    state: A,
    clock: K,
    transport: T,
) -> ProtocolPorts<C, H, A, K, T> {
    ProtocolWorkSetup::new(authority, custody, handles, state, clock, transport).ports()
}

// ---------------------------------------------------------------------------
// The process entry
// ---------------------------------------------------------------------------

static ENTRY: OnceLock<Arc<Mutex<CrossDeviceEntry<ReplayIdentity>>>> = OnceLock::new();

/// Installs the process-wide cross-device entry.
///
/// Called once by [`crate::install_environment_ports`]. Before it runs, every
/// admission refuses ([`CrossDeviceRefusal::EntryUnavailable`]) rather than
/// inventing an entry with no bindings.
pub fn install() -> Result<(), &'static str> {
    ENTRY
        .set(Arc::new(Mutex::new(CrossDeviceEntry::new(Arc::new(
            HostPeerBindings::new(),
        )))))
        .map_err(|_| "cross device entry is already installed")
}

/// Runs one operation against the installed process entry.
///
/// # Errors
///
/// [`CrossDeviceRefusal::EntryUnavailable`] when the composition root has not
/// installed the entry.
pub fn with_entry<Output>(
    operation: impl FnOnce(&mut CrossDeviceEntry<ReplayIdentity>) -> Output,
) -> Result<Output, CrossDeviceRefusal> {
    let entry = ENTRY.get().ok_or(CrossDeviceRefusal::EntryUnavailable)?;
    let mut guard = entry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(operation(&mut guard))
}

// ---------------------------------------------------------------------------
// The unattached device ports
// ---------------------------------------------------------------------------

// The route that admits a protocol line needs concrete ports. Until the device
// layer registers its own, these answer `Unavailable` for every operation: they
// never report a key, a revision, or an outcome the host does not hold. The
// authority admission itself reads none of them, so admitting the fixed
// Candidate stays a real operation while the device half stays honestly absent.

/// Custody that holds no key material.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnattachedCustody;

/// Handles that resolve no token.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnattachedHandles;

/// Durable state that has opened no store.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnattachedState;

/// The process clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

/// A carrier that has no transport attached.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnattachedTransport;

const fn unattached() -> PortFailure {
    PortFailure::Unavailable
}

impl KeyCustodyPort for UnattachedCustody {
    fn ed25519_public(&self, _handle: CustodyHandle) -> Result<[u8; 32], PortFailure> {
        Err(unattached())
    }

    fn ed25519_sign(
        &self,
        _handle: CustodyHandle,
        _message: &[u8],
    ) -> Result<[u8; 64], PortFailure> {
        Err(unattached())
    }

    fn ml_dsa_65_public(&self, _handle: CustodyHandle) -> Result<Vec<u8>, PortFailure> {
        Err(unattached())
    }

    fn ml_dsa_65_sign(
        &self,
        _handle: CustodyHandle,
        _message: &[u8],
    ) -> Result<Vec<u8>, PortFailure> {
        Err(unattached())
    }

    fn x25519_public(&self, _handle: CustodyHandle) -> Result<[u8; 32], PortFailure> {
        Err(unattached())
    }

    fn x25519(&self, _handle: CustodyHandle, _public: &[u8; 32]) -> Result<[u8; 32], PortFailure> {
        Err(unattached())
    }

    fn ml_kem_768_public(&self, _handle: CustodyHandle) -> Result<Vec<u8>, PortFailure> {
        Err(unattached())
    }

    fn ml_kem_768_encapsulate(
        &self,
        _public: &[u8],
        _entropy: CustodyHandle,
    ) -> Result<(Vec<u8>, [u8; 32]), PortFailure> {
        Err(unattached())
    }

    fn ml_kem_768_decapsulate(
        &self,
        _handle: CustodyHandle,
        _ciphertext: &[u8],
    ) -> Result<[u8; 32], PortFailure> {
        Err(unattached())
    }

    fn abort_x25519(&mut self, _staged: CustodyHandle) {}

    fn abort_ml_kem_768(&mut self, _staged: CustodyHandle) {}
}

impl CustodyHandles for UnattachedHandles {
    fn custody_handle(
        &self,
        _token: u128,
        _purpose: CustodyPurpose,
        _lifecycle: CustodyLifecycle,
    ) -> Result<CustodyHandle, PortFailure> {
        Err(unattached())
    }
}

impl AtomicStatePort for UnattachedState {
    type Snapshot = EndpointState;
    type Pending = SdkPendingItem;

    fn load(&self) -> Result<PortVersioned<EndpointState, SdkPendingItem>, PortFailure> {
        Err(unattached())
    }

    fn compare_and_swap(
        &mut self,
        _expected: PortRevision,
        _commit: PortStateCommit<EndpointState, SdkPendingItem>,
    ) -> Result<PortRevision, PortFailure> {
        Err(unattached())
    }

    fn settle(
        &mut self,
        _revision: PortRevision,
        _pending: &SdkPendingItem,
    ) -> Result<PortRevision, PortFailure> {
        Err(unattached())
    }
}

impl ClockPort for SystemClock {
    fn now_unix_seconds(&self) -> Result<u64, PortFailure> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .map_err(|_| PortFailure::Unavailable)
    }
}

impl TransportPort for UnattachedTransport {
    fn submit(&mut self, _packet: &[u8]) -> Result<TransportOutcome, PortFailure> {
        Err(unattached())
    }
}

// ---------------------------------------------------------------------------
// The caller's request shape
// ---------------------------------------------------------------------------

// The native route receives the peer's decoded body as JSON, and these decoders
// are the only place that shape is read. They bound and refuse; they never
// supply an author, a device, a permission, or an order, and every part they
// produce goes through the same bounded constructors the transport mapping uses.

/// Largest accepted logical id, in hex characters (16 bytes).
const LOGICAL_ID_HEX_BYTES: usize = 32;

/// Decodes one peer message body from the caller's request object.
///
/// `logicalId` is the peer's own identity of the logical message and is the only
/// identity this decoder reads; `relatesTo` names the message this one answers
/// when it answers one; `parts` carries the decoded content in order.
///
/// # Errors
///
/// [`CrossDeviceRefusal::MalformedRequest`] for a missing or unbounded identity,
/// and [`CrossDeviceRefusal::Unit`] for a part the transport mapping refuses.
pub fn decode_body(params: &Value) -> Result<PeerMessageBody, CrossDeviceRefusal> {
    let logical_id = decode_logical_id(params.get("logicalId")).ok_or(
        CrossDeviceRefusal::MalformedRequest("peer_logical_id_invalid"),
    )?;
    let relates_to = match params.get("relatesTo") {
        None | Some(Value::Null) => None,
        Some(value) => Some(decode_logical_id(Some(value)).ok_or(
            CrossDeviceRefusal::MalformedRequest("peer_relates_to_invalid"),
        )?),
    };
    let raw_parts = params
        .get("parts")
        .and_then(Value::as_array)
        .ok_or(CrossDeviceRefusal::MalformedRequest("peer_parts_missing"))?;
    let mut parts = Vec::with_capacity(raw_parts.len());
    for raw in raw_parts {
        parts.push(decode_part(raw)?);
    }
    PeerMessageBody::new(logical_id, relates_to, parts).map_err(CrossDeviceRefusal::Unit)
}

/// Decodes the carriage context: direct, or one bounded station label.
///
/// A station label is carriage only. It can never become an author, a device, or
/// trust, so the decoder refuses a label the transport mapping would refuse.
///
/// # Errors
///
/// [`CrossDeviceRefusal::MalformedRequest`] for a forwarder that is neither
/// `"direct"` nor a bounded station object.
pub fn decode_forwarder(params: &Value) -> Result<MessageForwarder, CrossDeviceRefusal> {
    match params.get("forwarder") {
        None | Some(Value::Null) => Ok(MessageForwarder::Direct),
        Some(Value::String(value)) if value == "direct" => Ok(MessageForwarder::Direct),
        Some(Value::Object(forwarder)) => {
            let label = forwarder.get("station").and_then(Value::as_str).ok_or(
                CrossDeviceRefusal::MalformedRequest("peer_forwarder_invalid"),
            )?;
            StationRef::new(label)
                .map(MessageForwarder::Station)
                .map_err(CrossDeviceRefusal::Unit)
        }
        Some(_) => Err(CrossDeviceRefusal::MalformedRequest(
            "peer_forwarder_invalid",
        )),
    }
}

/// Decodes the station's own read-only report, when the caller carried one.
///
/// The two booleans are recorded verbatim. Nothing here can turn them into
/// delivery, admission, read, or acceptance evidence; the returned hint's own
/// `is_endpoint_evidence` is a constant `false`.
///
/// # Errors
///
/// [`CrossDeviceRefusal::MalformedRequest`] when only one of the two booleans is
/// present, because a partial report would be read as a report it is not.
pub fn decode_station_hint(params: &Value) -> Result<Option<StationHint>, CrossDeviceRefusal> {
    let accepted = params.get("stationAccepted");
    let duplicate = params.get("stationDuplicate");
    match (accepted, duplicate) {
        (None, None) => Ok(None),
        (Some(accepted), Some(duplicate)) => {
            let accepted = accepted
                .as_bool()
                .ok_or(CrossDeviceRefusal::MalformedRequest(
                    "peer_station_hint_invalid",
                ))?;
            let duplicate = duplicate
                .as_bool()
                .ok_or(CrossDeviceRefusal::MalformedRequest(
                    "peer_station_hint_invalid",
                ))?;
            Ok(Some(StationHint::reported(accepted, duplicate)))
        }
        _ => Err(CrossDeviceRefusal::MalformedRequest(
            "peer_station_hint_invalid",
        )),
    }
}

fn decode_part(raw: &Value) -> Result<PeerPart, CrossDeviceRefusal> {
    let object = raw
        .as_object()
        .ok_or(CrossDeviceRefusal::MalformedRequest("peer_part_invalid"))?;
    match object.get("kind").and_then(Value::as_str) {
        Some("text") => {
            let content = object
                .get("content")
                .and_then(Value::as_str)
                .ok_or(CrossDeviceRefusal::MalformedRequest("peer_text_missing"))?;
            PeerPart::text(content).map_err(CrossDeviceRefusal::Unit)
        }
        Some("command") => {
            let request = object
                .get("request")
                .cloned()
                .ok_or(CrossDeviceRefusal::MalformedRequest("peer_command_missing"))?;
            PeerPart::command(request).map_err(CrossDeviceRefusal::Unit)
        }
        _ => Err(CrossDeviceRefusal::MalformedRequest("peer_part_invalid")),
    }
}

fn decode_logical_id(value: Option<&Value>) -> Option<[u8; 16]> {
    let raw = value?.as_str()?;
    if raw.len() != LOGICAL_ID_HEX_BYTES {
        return None;
    }
    let mut decoded = [0u8; 16];
    for (index, pair) in raw.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Some(decoded)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The largest station label the request decoder accepts, in bytes.
pub const MAX_REQUEST_STATION_BYTES: usize = MAX_STATION_ID_BYTES;
