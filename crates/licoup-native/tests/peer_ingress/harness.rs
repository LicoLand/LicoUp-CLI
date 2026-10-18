//! Local two-endpoint harness over the fixed LicoArc SDK.
//!
//! Everything protocol-shaped here is the real pinned SDK: the authority line is
//! admitted through `licoup_protocol_bindings`, the handshake and every protected
//! record are real SDK operations, and every verified fact the ingress consumes
//! is produced by the SDK boundary's own owner.
//!
//! This harness stands in for the platform's caller-owned layer only. The SDK
//! edge itself is `licoup_protocol_bindings`' [`EndpointConsumer`] and
//! [`InboundSession`]: the harness hands them the caller-owned custody, store,
//! clock, and packet bytes, and consumes the [`TrustFacts`] they return. It
//! drives no SDK verification entry of its own and keeps no second facts type,
//! so the facts the transport mapping reads are exactly the client boundary's
//! own.
//!
//! What is synthetic is the caller-owned platform layer: custody material, the
//! atomic store, the clock, and the packet carrier, exactly the layer a real
//! device supplies. Nothing here reaches a network, a user message, or another
//! process.
//!
//! The authority artifact is an explicit environment input
//! (`LICOARC_AUTHORITY_BUNDLE`); with it absent the real-SDK cases report not
//! run instead of passing silently.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use licoarc::VerifiedProtocolLine;
use licoarc::endpoint::{
    Endpoint, EndpointState, IdentityPublic, IdentitySigningHandles, Initiator, PrekeyBundle,
    Responder,
};
use licoarc::error::{Error, ErrorCode, Stage};
use licoarc::provider::{AgreementProvider, RustCryptoProvider, SignatureProvider};
use licoarc::state::{
    ApplicationEffects, AtomicState, Clock, Commit, CustodyRef, Ed25519Signing, KeyCustody,
    KeyMutation, MlDsa65Signing, MlKem768Private, MlKemEncapsulationEntropy, PacketCarrier,
    PendingId, PendingKind, Revision, SecretHandle, StagedSecretHandle,
    TrustFacts as TrustFactsPort, Versioned, X25519Private,
};

use licoup_application::{
    ActorClaim, ActorPort, ApplicationCommand, ApplicationFacade, ApplicationFailure,
    ApplicationPorts, AssistantCommand, AssistantPort, CommandOutcome, ConversationCommand,
    ConversationPort, Operation, OperationReference, OperationState, SubagentCommand, SubagentPort,
};
use licoup_conversation::{ConversationStore, MembershipAccess, Principal, PrincipalKind};
use licoup_protocol_bindings::{
    AuthorityInput, EndpointConsumer, InboundRefusal, InboundSession, ReplayIdentity, TrustFacts,
};

use crate::domain::client_conversation::peer_ingress::{PeerBinding, PeerBindings, PeerIngress};
use crate::domain::mobile_relay::endpoint_transport::{PeerAuthor, PeerDevice};

/// The ingress the fixture composes: keyed by the SDK boundary's own replay
/// identity of a committed protected record.
pub type FixtureIngress = PeerIngress<ReplayIdentity>;

// ---------------------------------------------------------------------------
// Caller-owned platform layer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Eq, PartialEq)]
enum Lifecycle {
    Staged,
    Adopted,
}

#[derive(Clone)]
enum Material {
    Ed25519([u8; 32]),
    MlDsa65([u8; 32]),
    X25519([u8; 32]),
    MlKem768([u8; 64]),
    MlKemEntropy([u8; 32]),
}

#[derive(Clone)]
struct KeyRecord {
    lifecycle: Lifecycle,
    material: Material,
}

#[derive(Clone, Default)]
struct SyntheticLifecycle {
    keys: HashMap<u128, KeyRecord>,
}

type SharedLifecycle = Rc<RefCell<SyntheticLifecycle>>;

/// The caller-owned custody the SDK signs and agrees through. Every operation
/// uses real cryptography; the tokens are the only synthetic part.
#[derive(Clone)]
pub struct TestCustody {
    provider: RustCryptoProvider,
    lifecycle: SharedLifecycle,
}

impl Default for TestCustody {
    fn default() -> Self {
        Self {
            provider: RustCryptoProvider,
            lifecycle: Rc::new(RefCell::new(SyntheticLifecycle::default())),
        }
    }
}

impl TestCustody {
    fn insert(&mut self, token: u128, lifecycle: Lifecycle, material: Material) {
        assert!(
            self.lifecycle
                .borrow_mut()
                .keys
                .insert(
                    token,
                    KeyRecord {
                        lifecycle,
                        material,
                    },
                )
                .is_none(),
            "synthetic custody tokens are unique"
        );
    }

    fn signing_handles(&mut self, marker: u8) -> IdentitySigningHandles {
        let ed25519 = u128::from(marker) << 8;
        let ml_dsa_65 = ed25519 + 1;
        self.insert(ed25519, Lifecycle::Adopted, Material::Ed25519([marker; 32]));
        self.insert(
            ml_dsa_65,
            Lifecycle::Adopted,
            Material::MlDsa65([marker.wrapping_add(1); 32]),
        );
        IdentitySigningHandles {
            ed25519: SecretHandle::from_custody_token(ed25519),
            ml_dsa_65: SecretHandle::from_custody_token(ml_dsa_65),
        }
    }

    fn stage_x25519(
        &mut self,
        token: u128,
        private: [u8; 32],
    ) -> StagedSecretHandle<X25519Private> {
        self.insert(token, Lifecycle::Staged, Material::X25519(private));
        StagedSecretHandle::from_custody_token(token)
    }

    fn stage_ml_kem_768(
        &mut self,
        token: u128,
        private: [u8; 64],
    ) -> StagedSecretHandle<MlKem768Private> {
        self.insert(token, Lifecycle::Staged, Material::MlKem768(private));
        StagedSecretHandle::from_custody_token(token)
    }

    fn ml_kem_entropy(
        &mut self,
        token: u128,
        entropy: [u8; 32],
    ) -> SecretHandle<MlKemEncapsulationEntropy> {
        self.insert(token, Lifecycle::Adopted, Material::MlKemEntropy(entropy));
        SecretHandle::from_custody_token(token)
    }

    fn shared_lifecycle(&self) -> SharedLifecycle {
        Rc::clone(&self.lifecycle)
    }

    fn missing() -> Error {
        Error::terminal(ErrorCode::ProviderFailure, Stage::Provider)
    }

    fn lookup(&self, token: u128, expected: Lifecycle) -> Result<Material, Error> {
        let lifecycle = self.lifecycle.borrow();
        let record = lifecycle.keys.get(&token).ok_or_else(Self::missing)?;
        if record.lifecycle != expected {
            return Err(Self::missing());
        }
        Ok(record.material.clone())
    }

    fn referenced_material<P>(&self, handle: CustodyRef<'_, P>) -> Result<Material, Error> {
        match handle {
            CustodyRef::Adopted(handle) => self.lookup(handle.custody_token(), Lifecycle::Adopted),
            CustodyRef::Staged(handle) => self.lookup(handle.custody_token(), Lifecycle::Staged),
        }
    }

    pub fn ed25519_public(&self, handle: &SecretHandle<Ed25519Signing>) -> Result<[u8; 32], Error> {
        let Material::Ed25519(private) = self.lookup(handle.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        Ok(SignatureProvider::ed25519_public(&self.provider, &private))
    }

    pub fn ml_dsa_65_public(
        &self,
        handle: &SecretHandle<MlDsa65Signing>,
    ) -> Result<Vec<u8>, Error> {
        let Material::MlDsa65(private) = self.lookup(handle.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        Ok(SignatureProvider::ml_dsa_65_public(
            &self.provider,
            &private,
        ))
    }
}

impl KeyCustody for TestCustody {
    fn ed25519_public(&self, handle: &SecretHandle<Ed25519Signing>) -> Result<[u8; 32], Error> {
        TestCustody::ed25519_public(self, handle)
    }

    fn ed25519_sign(
        &self,
        handle: &SecretHandle<Ed25519Signing>,
        message: &[u8],
    ) -> Result<[u8; 64], Error> {
        let Material::Ed25519(private) = self.lookup(handle.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        Ok(SignatureProvider::ed25519_sign(
            &self.provider,
            &private,
            message,
        ))
    }

    fn ml_dsa_65_public(&self, handle: &SecretHandle<MlDsa65Signing>) -> Result<Vec<u8>, Error> {
        TestCustody::ml_dsa_65_public(self, handle)
    }

    fn ml_dsa_65_sign(
        &self,
        handle: &SecretHandle<MlDsa65Signing>,
        message: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let Material::MlDsa65(private) = self.lookup(handle.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        Ok(SignatureProvider::ml_dsa_65_sign(
            &self.provider,
            &private,
            message,
        ))
    }

    fn x25519_public(&self, handle: CustodyRef<'_, X25519Private>) -> Result<[u8; 32], Error> {
        let Material::X25519(private) = self.referenced_material(handle)? else {
            return Err(Self::missing());
        };
        Ok(AgreementProvider::x25519_public(&self.provider, &private))
    }

    fn x25519(
        &self,
        handle: CustodyRef<'_, X25519Private>,
        public: &[u8; 32],
    ) -> Result<[u8; 32], Error> {
        let Material::X25519(private) = self.referenced_material(handle)? else {
            return Err(Self::missing());
        };
        AgreementProvider::x25519(&self.provider, &private, public)
    }

    fn ml_kem_768_public(&self, handle: CustodyRef<'_, MlKem768Private>) -> Result<Vec<u8>, Error> {
        let Material::MlKem768(private) = self.referenced_material(handle)? else {
            return Err(Self::missing());
        };
        Ok(AgreementProvider::ml_kem_768_public(
            &self.provider,
            &private,
        ))
    }

    fn ml_kem_768_encapsulate(
        &self,
        public: &[u8],
        entropy: &SecretHandle<MlKemEncapsulationEntropy>,
    ) -> Result<(Vec<u8>, [u8; 32]), Error> {
        let Material::MlKemEntropy(entropy) =
            self.lookup(entropy.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        AgreementProvider::ml_kem_768_encapsulate(&self.provider, public, &entropy)
    }

    fn ml_kem_768_decapsulate(
        &self,
        handle: &SecretHandle<MlKem768Private>,
        ciphertext: &[u8],
    ) -> Result<[u8; 32], Error> {
        let Material::MlKem768(private) =
            self.lookup(handle.custody_token(), Lifecycle::Adopted)?
        else {
            return Err(Self::missing());
        };
        AgreementProvider::ml_kem_768_decapsulate(&self.provider, &private, ciphertext)
    }

    fn abort_x25519(&mut self, staged: &StagedSecretHandle<X25519Private>) {
        let mut lifecycle = self.lifecycle.borrow_mut();
        let removable = lifecycle
            .keys
            .get(&staged.custody_token())
            .is_some_and(|record| {
                record.lifecycle == Lifecycle::Staged
                    && matches!(record.material, Material::X25519(_))
            });
        if removable {
            lifecycle.keys.remove(&staged.custody_token());
        }
    }

    fn abort_ml_kem_768(&mut self, staged: &StagedSecretHandle<MlKem768Private>) {
        let mut lifecycle = self.lifecycle.borrow_mut();
        let removable = lifecycle
            .keys
            .get(&staged.custody_token())
            .is_some_and(|record| {
                record.lifecycle == Lifecycle::Staged
                    && matches!(record.material, Material::MlKem768(_))
            });
        if removable {
            lifecycle.keys.remove(&staged.custody_token());
        }
    }
}

fn mutation_matches(
    lifecycle: &SyntheticLifecycle,
    token: u128,
    expected: Lifecycle,
    purpose: fn(&Material) -> bool,
) -> bool {
    lifecycle
        .keys
        .get(&token)
        .is_some_and(|record| record.lifecycle == expected && purpose(&record.material))
}

fn validate_mutation(lifecycle: &SyntheticLifecycle, mutation: &KeyMutation) -> Result<(), Error> {
    let valid = match mutation {
        KeyMutation::AdoptX25519(handle) => mutation_matches(
            lifecycle,
            handle.custody_token(),
            Lifecycle::Staged,
            |material| matches!(material, Material::X25519(_)),
        ),
        KeyMutation::AdoptMlKem768(handle) => mutation_matches(
            lifecycle,
            handle.custody_token(),
            Lifecycle::Staged,
            |material| matches!(material, Material::MlKem768(_)),
        ),
        KeyMutation::DeleteX25519(handle) => mutation_matches(
            lifecycle,
            handle.custody_token(),
            Lifecycle::Adopted,
            |material| matches!(material, Material::X25519(_)),
        ),
        KeyMutation::DeleteMlKem768(handle) => mutation_matches(
            lifecycle,
            handle.custody_token(),
            Lifecycle::Adopted,
            |material| matches!(material, Material::MlKem768(_)),
        ),
        KeyMutation::DeleteMlKemEncapsulationEntropy(handle) => mutation_matches(
            lifecycle,
            handle.custody_token(),
            Lifecycle::Adopted,
            |material| matches!(material, Material::MlKemEntropy(_)),
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::terminal(ErrorCode::ProviderFailure, Stage::Commit))
    }
}

fn apply_mutation(lifecycle: &mut SyntheticLifecycle, mutation: &KeyMutation) -> Result<(), Error> {
    validate_mutation(lifecycle, mutation)?;
    match mutation {
        KeyMutation::AdoptX25519(handle) => {
            lifecycle
                .keys
                .get_mut(&handle.custody_token())
                .expect("validated staged X25519")
                .lifecycle = Lifecycle::Adopted;
        }
        KeyMutation::AdoptMlKem768(handle) => {
            lifecycle
                .keys
                .get_mut(&handle.custody_token())
                .expect("validated staged ML-KEM")
                .lifecycle = Lifecycle::Adopted;
        }
        KeyMutation::DeleteX25519(handle) => {
            lifecycle.keys.remove(&handle.custody_token());
        }
        KeyMutation::DeleteMlKem768(handle) => {
            lifecycle.keys.remove(&handle.custody_token());
        }
        KeyMutation::DeleteMlKemEncapsulationEntropy(handle) => {
            lifecycle.keys.remove(&handle.custody_token());
        }
    }
    Ok(())
}

/// The caller-owned durable state root. The handle is shared so the harness can
/// read committed pending work (the protected record's plaintext) while the SDK
/// session stays usable.
#[derive(Clone)]
pub struct TestStore {
    value: Rc<RefCell<Versioned<EndpointState>>>,
    lifecycle: SharedLifecycle,
}

impl TestStore {
    fn new(state: EndpointState, lifecycle: SharedLifecycle) -> Self {
        Self {
            value: Rc::new(RefCell::new(Versioned::initial(state))),
            lifecycle,
        }
    }

    pub fn revision(&self) -> Revision {
        self.value.borrow().revision()
    }

    /// The committed plaintext of one pending record, exactly as the SDK
    /// committed it. Reading it is the caller side of the SDK's
    /// `release_pending_plaintext` boundary; the harness settles the item itself
    /// because the wrapped inbound session exposes no release door.
    pub fn take_plaintext(&self, pending: PendingId) -> Option<Vec<u8>> {
        self.value
            .borrow()
            .pending()
            .iter()
            .find(|item| item.id() == pending)
            .and_then(|item| match item.kind() {
                PendingKind::Plaintext(plaintext) => Some(plaintext.clone()),
                _ => None,
            })
    }

    pub fn settle_pending(&mut self, pending: PendingId) -> Result<Revision, Error> {
        AtomicState::settle(self, self.revision(), pending)
    }
}

impl AtomicState<EndpointState> for TestStore {
    fn load(&self) -> Result<Versioned<EndpointState>, Error> {
        Ok(self.value.borrow().clone())
    }

    fn compare_and_swap(
        &mut self,
        expected: Revision,
        commit: Commit<EndpointState>,
    ) -> Result<Revision, Error> {
        let mut value = self.value.borrow_mut();
        if value.revision() != expected {
            return Err(Error::terminal(ErrorCode::Conflict, Stage::Commit));
        }
        let mutations = commit.key_mutations().to_vec();
        let mut next_lifecycle = {
            let lifecycle = self.lifecycle.borrow();
            for mutation in &mutations {
                validate_mutation(&lifecycle, mutation)?;
            }
            lifecycle.clone()
        };
        let next = commit.into_versioned(expected)?;
        for mutation in &mutations {
            apply_mutation(&mut next_lifecycle, mutation)?;
        }
        *self.lifecycle.borrow_mut() = next_lifecycle;
        *value = next;
        Ok(value.revision())
    }

    fn settle(&mut self, revision: Revision, pending: PendingId) -> Result<Revision, Error> {
        let mut value = self.value.borrow_mut();
        if value.revision() != revision {
            return Err(Error::terminal(ErrorCode::Conflict, Stage::Commit));
        }
        let mut remaining = value.pending().to_vec();
        let Some(index) = remaining.iter().position(|item| item.id() == pending) else {
            return Err(Error::terminal(ErrorCode::InvalidTransition, Stage::Commit));
        };
        remaining.remove(index);
        let commit = Commit::bounded(value.state().clone(), Vec::new(), remaining, 0, 16)?;
        *value = commit.into_versioned(revision)?;
        Ok(value.revision())
    }
}

pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_unix_seconds(&self) -> Result<u64, Error> {
        Ok(self.0)
    }
}

#[derive(Default)]
pub struct CaptureCarrier {
    pub packet: Vec<u8>,
}

impl PacketCarrier for CaptureCarrier {
    fn send(&mut self, packet: &[u8]) -> Result<(), Error> {
        self.packet = packet.to_vec();
        Ok(())
    }
}

#[derive(Default)]
pub struct NoEffects;

impl ApplicationEffects for NoEffects {
    fn apply(&mut self, _: &[u8]) -> Result<(), Error> {
        Ok(())
    }
}

/// The caller-owned trust facts the SDK resolves a peer identity through.
pub struct FixedTrust {
    identity: IdentityPublic,
    profile: [u8; 32],
}

impl TrustFactsPort for FixedTrust {
    fn identity_key(
        &self,
        state_digest: &[u8; 32],
        purpose: &'static str,
        profile: &[u8; 32],
    ) -> Result<Vec<u8>, Error> {
        if state_digest != &self.identity.state_digest || profile != &self.profile {
            return Err(Error::terminal(
                ErrorCode::AuthenticationFailed,
                Stage::Validation,
            ));
        }
        match purpose {
            "ed25519-key-id" => Ok(self.identity.ed25519_key_id.to_vec()),
            "ed25519-public" => Ok(self.identity.ed25519_public.to_vec()),
            "ml-dsa-65-key-id" => Ok(self.identity.ml_dsa_65_key_id.to_vec()),
            "ml-dsa-65-public" => Ok(self.identity.ml_dsa_65_public.clone()),
            _ => Err(Error::terminal(
                ErrorCode::AuthenticationFailed,
                Stage::Validation,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Two endpoints over the fixed Candidate
// ---------------------------------------------------------------------------

/// The fixed identity digests both sides agree on for this component scenario.
pub const PEER_AUTHORITY: [u8; 32] = [0xA1; 32];
pub const HOST_AUTHORITY: [u8; 32] = [0xB2; 32];

/// One received and released peer record.
///
/// `facts` are the SDK boundary's own [`TrustFacts`] of this exact committed
/// record, so the transport mapping reads the production verified-unit value
/// rather than a fixture copy of it. `record` is the SDK's own pending id of the
/// same unit, which the fixture uses to release and settle its plaintext.
#[derive(Debug)]
pub struct ReceivedRecord {
    pub facts: TrustFacts,
    /// The SDK's own identity of the committed protected record.
    pub record: PendingId,
    pub plaintext: Vec<u8>,
}

pub struct PeerSession {
    endpoint: Endpoint<Initiator, RustCryptoProvider, TestCustody, TestStore>,
}

impl PeerSession {
    /// Protects one message and returns the packet the transport would carry.
    pub fn send(&mut self, payload: &[u8]) -> Vec<u8> {
        let pending = self
            .endpoint
            .send_record(payload, None)
            .expect("the established peer protects one record");
        let mut carrier = CaptureCarrier::default();
        let mut effects = NoEffects;
        self.endpoint
            .dispatch_pending(pending, &mut carrier, &mut effects)
            .expect("the committed packet dispatches");
        carrier.packet
    }
}

pub struct HostSession {
    session: InboundSession<RustCryptoProvider, TestCustody, TestStore>,
    store: TestStore,
    custody: TestCustody,
    next_token: u128,
}

impl HostSession {
    /// The facts of the accepted handshake, as the SDK boundary recorded them.
    ///
    /// A handshake is not a committed protected record, so this value carries
    /// no record identity and cannot be mapped into a peer message.
    pub fn facts(&self) -> &TrustFacts {
        self.session.facts()
    }

    /// The caller-owned store's revision.
    ///
    /// A fixture reads it on both sides of one `receive` to show that the SDK
    /// durably recorded a verified unit before any conversation write happened.
    pub fn revision(&self) -> Revision {
        self.store.revision()
    }

    /// Verifies and commits one inbound record through the SDK boundary, then
    /// releases its plaintext from the caller-owned store.
    pub fn receive(&mut self, packet: &[u8]) -> ReceivedRecord {
        self.try_receive(packet)
            .expect("the inbound record verifies and commits")
    }

    /// Verifies and commits one inbound record through the SDK boundary's own
    /// [`InboundSession::receive_record`], then releases its plaintext from the
    /// caller-owned store.
    ///
    /// The commitment lands in the caller-owned store before this returns, so
    /// the caller-owned revision is the fixture's evidence that the recorded
    /// verification happened before any conversation write. The returned facts
    /// are the boundary's own value, not a fixture's re-reading of raw inputs.
    pub fn try_receive(&mut self, packet: &[u8]) -> Result<ReceivedRecord, InboundRefusal> {
        let token = self.next_token;
        self.next_token += 1;
        let fresh = self.custody.stage_x25519(token, [(token % 251) as u8; 32]);
        let (facts, record) = self.session.receive_record(packet, Some(fresh))?;
        let plaintext = self
            .store
            .take_plaintext(record)
            .expect("the committed record carries its plaintext");
        self.store
            .settle_pending(record)
            .expect("the released plaintext settles");
        Ok(ReceivedRecord {
            facts,
            record,
            plaintext,
        })
    }
}

pub struct EndpointPair {
    pub peer: PeerSession,
    pub host: HostSession,
}

/// Runs one real handshake between a peer initiator and the host responder.
///
/// The responder side is the SDK boundary's own
/// [`EndpointConsumer::trust_peer`] and [`EndpointConsumer::accept_handshake`];
/// the initiator side drives the SDK's own first-packet and accept
/// verification entries, which the boundary owns no wrapper for.
pub fn establish(line: &VerifiedProtocolLine, seed: u8) -> EndpointPair {
    let provider = RustCryptoProvider;
    let profile = *line.protection_profile_id();
    let clock = FixedClock(50);
    let consumer = EndpointConsumer::new();

    let mut peer_custody = TestCustody::default();
    let (peer_identity, peer_signing) = identity(&mut peer_custody, seed);
    let peer_store = TestStore::new(EndpointState::initiator(), peer_custody.shared_lifecycle());

    let mut host_custody = TestCustody::default();
    let (host_identity, host_signing) = identity(&mut host_custody, seed.wrapping_add(40));
    let host_store = TestStore::new(EndpointState::responder(), host_custody.shared_lifecycle());

    let trusted_host = consumer
        .trust_peer(
            line,
            &FixedTrust {
                identity: host_identity.clone(),
                profile,
            },
            &host_identity,
        )
        .expect("the host identity resolves through the SDK boundary's trust entry");
    let trusted_peer = consumer
        .trust_peer(
            line,
            &FixedTrust {
                identity: peer_identity.clone(),
                profile,
            },
            &peer_identity,
        )
        .expect("the peer identity resolves through the SDK boundary's trust entry");

    let mut responder =
        Endpoint::<Responder, RustCryptoProvider, TestCustody, TestStore>::responder(
            line.clone(),
            provider,
            host_custody.clone(),
            host_store.clone(),
        )
        .expect("the verified line constructs a responder");
    let prekey_x25519 = host_custody.stage_x25519(0x61, [0x61; 32]);
    let prekey_ml_kem = host_custody.stage_ml_kem_768(0x62, [0x62; 64]);
    let bundle: PrekeyBundle = responder
        .admit_prekey(
            &host_identity,
            &host_signing,
            1,
            prekey_x25519,
            prekey_ml_kem,
            10,
            100,
        )
        .expect("the host admits one prekey");

    let mut initiator =
        Endpoint::<Initiator, RustCryptoProvider, TestCustody, TestStore>::initiator(
            line.clone(),
            provider,
            peer_custody.clone(),
            peer_store,
        )
        .expect("the verified line constructs an initiator");
    let first_x25519 = peer_custody.stage_x25519(0x71, [0x71; 32]);
    let first_entropy = peer_custody.ml_kem_entropy(0x72, [0x72; 32]);
    let first = initiator
        .create_first_packet(
            &peer_identity,
            &trusted_host,
            PEER_AUTHORITY,
            HOST_AUTHORITY,
            &peer_signing,
            bundle,
            &clock,
            first_x25519,
            first_entropy,
        )
        .expect("the peer creates one first packet");

    let (session, accept) = consumer
        .accept_handshake(
            responder,
            &trusted_peer,
            &host_identity,
            HOST_AUTHORITY,
            &first,
            &clock,
        )
        .expect("the SDK boundary accepts the handshake");

    // The accept and the packet it answers must describe one handshake. These
    // are the equalities the SDK itself requires before it produces an accept;
    // the fixture asserts them so it cannot record a verification that never
    // happened. `licoup_protocol_bindings::TrustFacts` enforces the same
    // equalities when the accepted handshake becomes facts, so an accept that
    // did not answer this packet is refused there as well.
    assert_eq!(
        accept.initiator_user_authority_state_digest,
        first.initiator_user_authority_state_digest
    );
    assert_eq!(accept.pair_sequence, first.prekey.pair_sequence);
    assert_eq!(
        accept.responder_identity_state_digest,
        first.responder_identity_state_digest
    );
    assert_eq!(
        accept.responder_user_authority_state_digest,
        first.responder_user_authority_state_digest
    );
    assert_eq!(first.protocol_line_id, *line.protocol_line_id());
    assert_eq!(first.protection_profile_id, *line.protection_profile_id());

    // The facts the boundary produced name the admitted Line, so every later
    // verified unit is bound to the same fixed accepted identity.
    let version = session.facts().protocol_version();
    assert_eq!(version.wire_id(), line.wire_id());
    assert_eq!(version.generation(), line.generation());
    assert_eq!(version.protocol_line_id(), *line.protocol_line_id());
    assert_eq!(
        version.protection_profile_id(),
        *line.protection_profile_id()
    );

    initiator
        .verify_session_accept(&accept)
        .expect("the peer verifies the accept");

    EndpointPair {
        peer: PeerSession {
            endpoint: initiator,
        },
        host: HostSession {
            session,
            store: host_store,
            custody: host_custody,
            next_token: 0x2000,
        },
    }
}

fn identity(custody: &mut TestCustody, marker: u8) -> (IdentityPublic, IdentitySigningHandles) {
    let signing = custody.signing_handles(marker);
    let identity = IdentityPublic {
        state_digest: [marker.wrapping_add(2); 32],
        ed25519_key_id: [marker.wrapping_add(3); 32],
        ed25519_public: custody.ed25519_public(&signing.ed25519).unwrap(),
        ml_dsa_65_key_id: [marker.wrapping_add(4); 32],
        ml_dsa_65_public: custody.ml_dsa_65_public(&signing.ml_dsa_65).unwrap(),
    };
    (identity, signing)
}

/// Admits the explicitly supplied fixed Candidate artifact.
///
/// Admission is the client boundary's own `AuthorityInput::admit`, which the
/// SDK decides: artifact version, wire identity, generation, Candidate
/// lifecycle, complete definition, and content address. The accepted-version
/// freeze that goes with it is the same boundary's
/// [`EndpointConsumer::check_line`], which every entry below applies, so the
/// fixture states no version policy of its own.
pub fn authority_line() -> VerifiedProtocolLine {
    let path = std::env::var_os("LICOARC_AUTHORITY_BUNDLE")
        .expect("LICOARC_AUTHORITY_BUNDLE must name the explicit read-only bundle");
    let bytes = std::fs::read(path).expect("the explicit authority bundle must be readable");
    AuthorityInput::new(&bytes)
        .admit()
        .expect("the fixed Candidate artifact is admitted")
}

// ---------------------------------------------------------------------------
// Host-owned bindings and the conversation fixture
// ---------------------------------------------------------------------------

struct BindingEntry {
    author: [u8; 32],
    device: [u8; 32],
    binding: PeerBinding,
}

/// An in-memory stand-in for the host's durable peer bindings. Revocation is
/// expressed by removing the entry; nothing here creates a membership.
pub struct FixedBindings {
    entries: Mutex<Vec<BindingEntry>>,
}

impl FixedBindings {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    pub fn bind(&self, author: [u8; 32], device: [u8; 32], binding: PeerBinding) {
        self.lock().push(BindingEntry {
            author,
            device,
            binding,
        });
    }

    pub fn revoke(&self, author: [u8; 32], device: [u8; 32]) -> bool {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|entry| entry.author != author || entry.device != device);
        entries.len() != before
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<BindingEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for FixedBindings {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerBindings for FixedBindings {
    fn resolve(&self, author: &PeerAuthor, device: &PeerDevice) -> Option<PeerBinding> {
        let author = author.user_authority_state_digest();
        let device = device.identity_state_digest();
        self.lock()
            .iter()
            .find(|entry| entry.author == author && entry.device == device)
            .map(|entry| entry.binding.clone())
    }
}

/// One Canonical Conversation with an owner, a peer member, and an agent member.
pub struct ConversationFixture {
    pub store: ConversationStore,
    pub conversation_id: String,
    pub owner_membership_id: String,
    pub peer_membership_id: String,
    pub peer_principal_id: String,
}

pub fn conversation_fixture() -> ConversationFixture {
    let store = ConversationStore::open_in_memory().expect("an in-memory conversation store opens");
    let owner = principal("principal:owner", PrincipalKind::Human, "Owner", None);
    let peer = principal("principal:peer", PrincipalKind::Human, "Peer Device", None);
    let assistant = principal(
        "principal:assistant",
        PrincipalKind::Agent,
        "Assistant",
        Some("codex"),
    );
    let conversation = store
        .create_conversation_with_members(
            "Peer sync",
            owner,
            &[
                (peer, MembershipAccess::Member),
                (assistant, MembershipAccess::Member),
            ],
        )
        .expect("the fixture conversation is created");
    let peer_membership_id = membership_for(&conversation, "principal:peer");
    let owner_membership_id = membership_for(&conversation, "principal:owner");
    ConversationFixture {
        conversation_id: conversation.id,
        owner_membership_id,
        peer_membership_id,
        peer_principal_id: "principal:peer".to_owned(),
        store,
    }
}

fn membership_for(conversation: &licoup_conversation::Conversation, principal_id: &str) -> String {
    conversation
        .memberships
        .iter()
        .find(|membership| membership.principal.id == principal_id)
        .map(|membership| membership.id.clone())
        .expect("the fixture membership exists")
}

fn principal(
    id: &str,
    kind: PrincipalKind,
    display_name: &str,
    agent_id: Option<&str>,
) -> Principal {
    Principal {
        id: id.to_owned(),
        kind,
        display_name: display_name.to_owned(),
        agent_id: agent_id.map(str::to_owned),
        created_at_unix_ms: 1,
    }
}

pub fn events(
    store: &ConversationStore,
    conversation_id: &str,
) -> Vec<licoup_conversation::ConversationEvent> {
    store
        .page_events(conversation_id, None, 64)
        .expect("the conversation page reads")
        .events
}

/// Only the Message events: creating the fixture conversation legitimately
/// records membership changes, which are not peer messages.
pub fn messages(
    store: &ConversationStore,
    conversation_id: &str,
) -> Vec<licoup_conversation::ConversationEvent> {
    events(store, conversation_id)
        .into_iter()
        .filter(|event| event.kind == licoup_conversation::EventKind::Message)
        .collect()
}

pub fn conversation_count(store: &ConversationStore) -> usize {
    store.list(true).expect("the conversation list reads").len()
}

// ---------------------------------------------------------------------------
// The single-owner application entry, with recording ports
// ---------------------------------------------------------------------------

pub struct RecordingActor {
    pub verifies: AtomicUsize,
    pub allow: bool,
}

impl RecordingActor {
    pub fn allowing() -> Self {
        Self {
            verifies: AtomicUsize::new(0),
            allow: true,
        }
    }

    pub fn refusing() -> Self {
        Self {
            verifies: AtomicUsize::new(0),
            allow: false,
        }
    }

    pub fn verifies(&self) -> usize {
        self.verifies.load(Ordering::SeqCst)
    }
}

impl ActorPort for RecordingActor {
    fn verify(&self, _claim: &ActorClaim) -> Result<(), ApplicationFailure> {
        self.verifies.fetch_add(1, Ordering::SeqCst);
        if self.allow {
            Ok(())
        } else {
            Err(ApplicationFailure::permanent(
                "actor_refused",
                "actor/validate",
            ))
        }
    }
}

pub struct RecordingConversation {
    pub calls: AtomicUsize,
}

impl RecordingConversation {
    pub fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Default for RecordingConversation {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationPort for RecordingConversation {
    fn execute(
        &self,
        _claim: &ActorClaim,
        _command: &ConversationCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CommandOutcome::new(OperationReference::new(
            Operation::ConversationGet,
            "",
            OperationState::Completed,
        )))
    }
}

pub struct UnusedAssistant;

impl AssistantPort for UnusedAssistant {
    fn execute(
        &self,
        _claim: &ActorClaim,
        _command: &AssistantCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        Err(ApplicationFailure::permanent(
            "assistant_unused",
            "assistant/execute",
        ))
    }
}

pub struct UnusedSubagent;

impl SubagentPort for UnusedSubagent {
    fn execute(
        &self,
        _claim: &ActorClaim,
        _command: &SubagentCommand,
    ) -> Result<CommandOutcome, ApplicationFailure> {
        Err(ApplicationFailure::permanent(
            "subagent_unused",
            "subagent/execute",
        ))
    }
}

/// Builds the one business entry over the recording ports.
pub fn application_facade(
    actor: std::sync::Arc<RecordingActor>,
    conversation: std::sync::Arc<RecordingConversation>,
) -> ApplicationFacade {
    ApplicationFacade::new(ApplicationPorts::new(
        actor,
        std::sync::Arc::new(UnusedAssistant),
        std::sync::Arc::new(UnusedSubagent),
        conversation,
        // This harness never exercises the project port: a fail-closed
        // stand-in keeps the facade honest without reaching into native
        // composition that an integration test cannot name.
        std::sync::Arc::new(UnusedProject),
    ))
}

/// Decodes one peer command request through the application contract.
pub fn decode_command(request: &serde_json::Value) -> ApplicationCommand {
    ApplicationCommand::decode(request).expect("the peer command request is a typed command")
}

/// A project port that answers nothing: this harness has no project work, and
/// a silent success would be a lie about a capability the host does not have.
struct UnusedProject;

impl licoup_application::ProjectPort for UnusedProject {
    fn execute(
        &self,
        _claim: &licoup_application::ActorClaim,
        _command: &licoup_application::ProjectCommand,
    ) -> Result<licoup_application::CommandOutcome, licoup_application::ApplicationFailure> {
        Err(licoup_application::ApplicationFailure::permanent(
            "project_port_unavailable",
            "project",
        )
        .with_field("project"))
    }
}
