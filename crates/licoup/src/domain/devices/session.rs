//! The LicoArc pairing/session composition for the `devices` group.
//!
//! The client's `LocalIdentity`/`ResponderDrive`/`InitiatorDrive` are bound to
//! the development `FileCustody`/`FileState` pair; this module composes the
//! same public SDK endpoints over [`SessionCustody`] and [`SessionState`], so
//! identity keys, staged secrets and the checkpoint integrity key all stay in
//! the OS secret store.
//!
//! The flow matches `licoarc-client`'s drives: the responder mints a device
//! identity, stages one prekey and publishes a [`PairingOffer`] bound to its
//! mailbox handle; the initiator resolves the offer's identity through a
//! caller-owned trust view, produces a [`PairingSubmission`] bound to its
//! reply handle; the responder authenticates it and answers with a
//! [`PairingReply`] bound to its session handle; the initiator verifies the
//! accept. Every state-advancing step seals the SDK checkpoint before
//! returning, so a reopened drive resumes the last committed snapshot.

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use licoarc_client::endpoint::{
    Endpoint, IdentityPublic, IdentitySigningHandles, Initiator, Responder, SessionAccept,
};
use licoarc_client::inbound::{EndpointConsumer, InboundSession};
use licoarc_client::ports::{CustodyPurpose, KeyCustody as _};
use licoarc_client::provider::RustCryptoProvider;
use licoarc_client::session::{
    PairingOffer, PairingReply, PairingSubmission, StaticTrust, SystemClock,
};
use licoarc_client::state::{
    ApplicationEffects, MlKem768Private, MlKemEncapsulationEntropy, PacketCarrier, PendingId,
    PendingItem, PlaintextReceiver, SecretHandle, StagedSecretHandle, X25519Private,
};
use licoarc_client::{
    AuthorityInput, Error, HandleToken, VerifiedProtocolLine,
    adapters::{AtomicStateAdapter, ClockAdapter, KeyCustodyAdapter},
};

use super::session_custody::SessionCustody;
use super::session_state::SessionState;

/// The env var naming the LicoArc authority bundle artifact the caller
/// supplies (a path); the same variable `licoarc-client`'s own tests use.
pub const AUTHORITY_BUNDLE_ENV: &str = "LICOARC_AUTHORITY_BUNDLE";

/// The domain-separated label this device binds into a handshake as its
/// user-authority state digest: the device's own identity is the authority
/// fact peers see in `TrustFacts`.
const AUTHORITY_DIGEST_LABEL: &[u8] = b"licoup.device-authority-state.v1";

/// The pairing offer's validity window: one hour from issuance.
const OFFER_VALID_SECONDS: u64 = 3_600;
const OFFER_VALID_BACKDATE_SECONDS: u64 = 60;

type CustodyAdapter = KeyCustodyAdapter<SessionCustody, SessionCustody>;
type StoreAdapter = AtomicStateAdapter<SessionState>;
type ResponderEndpoint = Endpoint<Responder, RustCryptoProvider, CustodyAdapter, StoreAdapter>;
type InitiatorEndpoint = Endpoint<Initiator, RustCryptoProvider, CustodyAdapter, StoreAdapter>;
type AcceptedResponderSession = InboundSession<RustCryptoProvider, CustodyAdapter, StoreAdapter>;

/// Admits the caller-supplied LicoArc authority bundle into the verified
/// protocol line every endpoint in this process uses.
pub(crate) fn device_line() -> Result<VerifiedProtocolLine> {
    let path = std::env::var_os(AUTHORITY_BUNDLE_ENV)
        .ok_or_else(|| anyhow!("devices_authority_bundle_missing"))?;
    let bytes = std::fs::read(PathBuf::from(path))
        .map_err(|_| anyhow!("devices_authority_bundle_unreadable"))?;
    AuthorityInput::new(&bytes)
        .admit()
        .map_err(|_| anyhow!("devices_authority_bundle_refused"))
}

/// A locally minted device identity: two custody-held signing keys and their
/// public descriptor; the persisted form keeps only tokens and publics.
pub(crate) struct DeviceIdentity {
    public: IdentityPublic,
    ed25519: u128,
    ml_dsa_65: u128,
}

impl DeviceIdentity {
    /// Mints a fresh identity inside `custody`, or loads the persisted one.
    pub(crate) fn open(dir: &Path) -> Result<Self> {
        let file = dir.join("identity.json");
        if file.is_file() {
            return Self::load(&file);
        }
        let mut custody = SessionCustody::open(dir)?;
        let identity = Self::mint(&mut custody)?;
        identity.persist(&file)?;
        Ok(identity)
    }

    fn mint(custody: &mut SessionCustody) -> Result<Self> {
        let ed25519 = custody.adopt(CustodyPurpose::Ed25519Signing)?;
        let ml_dsa_65 = custody.adopt(CustodyPurpose::MlDsa65Signing)?;
        let ed25519_public = custody
            .ed25519_public(ed25519)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?;
        let ml_dsa_65_public = custody
            .ml_dsa_65_public(ml_dsa_65)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?;
        let ed25519_key_id = key_id(b"ed25519", &ed25519_public);
        let ml_dsa_65_key_id = key_id(b"ml-dsa-65", &ml_dsa_65_public);
        let state_digest = identity_state_digest(
            ed25519_key_id,
            ml_dsa_65_key_id,
            &ed25519_public,
            &ml_dsa_65_public,
        );
        Ok(Self {
            public: IdentityPublic {
                state_digest,
                ed25519_key_id,
                ed25519_public,
                ml_dsa_65_key_id,
                ml_dsa_65_public,
            },
            ed25519: ed25519.custody_token(),
            ml_dsa_65: ml_dsa_65.custody_token(),
        })
    }

    fn persist(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = IdentityFile {
            state_digest: hex(&self.public.state_digest),
            ed25519_key_id: hex(&self.public.ed25519_key_id),
            ed25519_public: hex(&self.public.ed25519_public),
            ml_dsa_65_key_id: hex(&self.public.ml_dsa_65_key_id),
            ml_dsa_65_public: hex(&self.public.ml_dsa_65_public),
            ed25519_token: hex(&self.ed25519.to_le_bytes()),
            ml_dsa_65_token: hex(&self.ml_dsa_65.to_le_bytes()),
        };
        let body = serde_json::to_string_pretty(&file)?;
        licoup_foundation::platform::file_security::atomic_write_private_text(path, &body)
            .context("devices_state_unavailable")
    }

    fn load(path: &Path) -> Result<Self> {
        let body = std::fs::read(path)?;
        let file: IdentityFile =
            serde_json::from_slice(&body).map_err(|_| anyhow!("devices_identity_invalid"))?;
        let identity = Self {
            public: IdentityPublic {
                state_digest: unhex_32(&file.state_digest)?,
                ed25519_key_id: unhex_32(&file.ed25519_key_id)?,
                ed25519_public: unhex_32(&file.ed25519_public)?,
                ml_dsa_65_key_id: unhex_32(&file.ml_dsa_65_key_id)?,
                ml_dsa_65_public: unhex(&file.ml_dsa_65_public)
                    .ok_or_else(|| anyhow!("devices_identity_invalid"))?,
            },
            ed25519: unhex_token(&file.ed25519_token)?,
            ml_dsa_65: unhex_token(&file.ml_dsa_65_token)?,
        };
        Ok(identity)
    }

    /// The public descriptor presented to peers.
    pub(crate) const fn public(&self) -> &IdentityPublic {
        &self.public
    }

    /// The custody handles the SDK uses when this identity must sign.
    pub(crate) fn signing_handles(&self) -> IdentitySigningHandles {
        IdentitySigningHandles {
            ed25519: SecretHandle::from_custody_token(self.ed25519),
            ml_dsa_65: SecretHandle::from_custody_token(self.ml_dsa_65),
        }
    }

    /// This device's authority-state fact: bound to its identity digest so a
    /// peer's `TrustFacts` names exactly this device.
    pub(crate) fn authority_state_digest(&self) -> [u8; 32] {
        let mut preimage = AUTHORITY_DIGEST_LABEL.to_vec();
        preimage.extend_from_slice(&self.public.state_digest);
        licoarc_client::provider::fixed_sha256(&preimage)
    }

    /// The stable `peerId` this device presents as: hex of its identity
    /// state digest.
    pub(crate) fn peer_id_of(identity: &IdentityPublic) -> String {
        hex(&identity.state_digest)
    }
}

/// A caller-owned key label: deterministic and domain-separated per purpose.
fn key_id(purpose: &[u8], public: &[u8]) -> [u8; 32] {
    let mut preimage = b"licoup.device-identity-key.v1\0".to_vec();
    preimage.extend_from_slice(purpose);
    preimage.push(0);
    preimage.extend_from_slice(public);
    licoarc_client::provider::fixed_sha256(&preimage)
}

/// A caller-owned identity label binding all four public facts.
fn identity_state_digest(
    ed25519_key_id: [u8; 32],
    ml_dsa_65_key_id: [u8; 32],
    ed25519_public: &[u8; 32],
    ml_dsa_65_public: &[u8],
) -> [u8; 32] {
    let mut preimage = b"licoup.device-identity-state.v1\0".to_vec();
    preimage.extend_from_slice(&ed25519_key_id);
    preimage.extend_from_slice(&ml_dsa_65_key_id);
    preimage.extend_from_slice(ed25519_public);
    preimage.extend_from_slice(ml_dsa_65_public);
    licoarc_client::provider::fixed_sha256(&preimage)
}

/// The persisted identity form; every field is lowercase hex.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityFile {
    state_digest: String,
    ed25519_key_id: String,
    ed25519_public: String,
    ml_dsa_65_key_id: String,
    ml_dsa_65_public: String,
    ed25519_token: String,
    ml_dsa_65_token: String,
}

/// The peer's `IdentityPublic` in the submission envelope.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PeerIdentityJson {
    state_digest: String,
    ed25519_key_id: String,
    ed25519_public: String,
    ml_dsa_65_key_id: String,
    ml_dsa_65_public: String,
}

impl PeerIdentityJson {
    pub(crate) fn of(public: &IdentityPublic) -> Self {
        Self {
            state_digest: hex(&public.state_digest),
            ed25519_key_id: hex(&public.ed25519_key_id),
            ed25519_public: hex(&public.ed25519_public),
            ml_dsa_65_key_id: hex(&public.ml_dsa_65_key_id),
            ml_dsa_65_public: hex(&public.ml_dsa_65_public),
        }
    }

    pub(crate) fn into_public(self) -> Result<IdentityPublic> {
        Ok(IdentityPublic {
            state_digest: unhex_32(&self.state_digest)?,
            ed25519_key_id: unhex_32(&self.ed25519_key_id)?,
            ed25519_public: unhex_32(&self.ed25519_public)?,
            ml_dsa_65_key_id: unhex_32(&self.ml_dsa_65_key_id)?,
            ml_dsa_65_public: unhex(&self.ml_dsa_65_public)
                .ok_or_else(|| anyhow!("devices_peer_identity_invalid"))?,
        })
    }
}

/// One claim envelope: the `PairingSubmission` bytes plus the claimer's
/// `IdentityPublic`, which the offerer needs to authenticate the packet (the
/// packet itself carries only digests).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimEnvelope {
    schema: String,
    submission: String,
    identity: PeerIdentityJson,
}

const CLAIM_ENVELOPE_SCHEMA: &str = "licoup.device-pair-claim.v1";
const MAX_ENVELOPE_BYTES: usize = 256 * 1024;

/// Encodes the copyable offer `code`: base64url of the canonical offer bytes
/// (which already carry the offerer's `IdentityPublic`).
pub(crate) fn encode_offer_code(offer: &PairingOffer) -> Result<String> {
    let bytes = offer
        .encode()
        .map_err(|_| anyhow!("devices_pairing_offer_invalid"))?;
    Ok(base64url_encode(&bytes))
}

/// Decodes a claim-supplied offer code.
pub(crate) fn decode_offer_code(code: &str) -> Result<PairingOffer> {
    let bytes = base64url_decode(code).ok_or_else(|| anyhow!("devices_pairing_code_invalid"))?;
    PairingOffer::decode(&bytes).map_err(|_| anyhow!("devices_pairing_code_invalid"))
}

/// Wraps one submission with the claimer's identity for the station envelope.
pub(crate) fn encode_claim_envelope(
    submission: &[u8],
    identity: &IdentityPublic,
) -> Result<Vec<u8>> {
    let envelope = ClaimEnvelope {
        schema: CLAIM_ENVELOPE_SCHEMA.to_owned(),
        submission: base64url_encode(submission),
        identity: PeerIdentityJson::of(identity),
    };
    Ok(serde_json::to_vec(&envelope)?)
}

/// Unwraps one claim envelope; refuses malformed or foreign payloads.
pub(crate) fn decode_claim_envelope(packet: &[u8]) -> Result<(Vec<u8>, IdentityPublic)> {
    ensure!(
        packet.len() <= MAX_ENVELOPE_BYTES,
        "devices_pairing_submission_invalid"
    );
    let envelope: ClaimEnvelope = serde_json::from_slice(packet)
        .map_err(|_| anyhow!("devices_pairing_submission_invalid"))?;
    ensure!(
        envelope.schema == CLAIM_ENVELOPE_SCHEMA,
        "devices_pairing_submission_invalid"
    );
    let submission = base64url_decode(&envelope.submission)
        .ok_or_else(|| anyhow!("devices_pairing_submission_invalid"))?;
    Ok((submission, envelope.identity.into_public()?))
}

/// The responder half of one pairing: stage a prekey, publish an offer,
/// authenticate a submission, then carry the established session.
pub(crate) struct ResponderSession {
    line: VerifiedProtocolLine,
    provider: RustCryptoProvider,
    custody: SessionCustody,
    state: SessionState,
    lane: Option<ResponderLane>,
}

enum ResponderLane {
    /// The pre-handshake endpoint, used only to admit prekeys.
    Offer(Box<ResponderEndpoint>),
    /// The established session the accepted handshake produced.
    Session(Box<AcceptedResponderSession>),
}

/// The initiator half of one pairing: build a submission, verify the reply,
/// then carry the established endpoint.
pub(crate) struct InitiatorSession {
    line: VerifiedProtocolLine,
    provider: RustCryptoProvider,
    custody: SessionCustody,
    state: SessionState,
    endpoint: Option<InitiatorEndpoint>,
}

impl ResponderSession {
    /// Opens the durable responder endpoint at `dir`.
    pub(crate) fn open(dir: &Path, line: VerifiedProtocolLine) -> Result<Self> {
        let state = SessionState::responder(dir, &line)?;
        let custody = state.custody().clone();
        Ok(Self {
            line,
            provider: RustCryptoProvider,
            custody,
            state,
            lane: None,
        })
    }

    fn custody_adapter(&self) -> CustodyAdapter {
        KeyCustodyAdapter::new(self.custody.clone(), self.custody.clone())
    }

    fn state_adapter(&self) -> StoreAdapter {
        AtomicStateAdapter::new(self.state.clone())
    }

    fn offer_endpoint(&mut self) -> Result<&mut ResponderEndpoint> {
        if self.lane.is_none() {
            self.lane = Some(ResponderLane::Offer(Box::new(
                Endpoint::<Responder, _, _, _>::resume(
                    self.line.clone(),
                    self.provider,
                    self.custody_adapter(),
                    self.state_adapter(),
                    self.state.minimum_generation(),
                )
                .map_err(|_| anyhow!("devices_session_unavailable"))?,
            )));
        }
        match self.lane.as_mut() {
            Some(ResponderLane::Offer(endpoint)) => Ok(endpoint),
            Some(ResponderLane::Session(_)) => bail!("devices_session_unavailable"),
            None => bail!("devices_session_unavailable"),
        }
    }

    /// Seals the live endpoint's committed snapshot.
    pub(crate) fn persist(&mut self) -> Result<()> {
        if let Some(lane) = self.lane.as_mut() {
            let endpoint = match lane {
                ResponderLane::Offer(endpoint) => &mut **endpoint,
                ResponderLane::Session(session) => session.endpoint_mut(),
            };
            self.state
                .save(endpoint)
                .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
        }
        Ok(())
    }

    /// Stages one X25519 + one ML-KEM private inside custody and admits the
    /// resulting prekey bundle, bound to `bootstrap_handle`.
    pub(crate) fn issue_offer(
        &mut self,
        identity: &DeviceIdentity,
        bootstrap_handle: HandleToken,
        pair_sequence: u64,
        now: u64,
    ) -> Result<PairingOffer> {
        let x25519 = self.custody.stage(CustodyPurpose::X25519Private)?;
        let ml_kem = self.custody.stage(CustodyPurpose::MlKem768Private)?;
        let endpoint = self.offer_endpoint()?;
        let prekey = endpoint
            .admit_prekey(
                identity.public(),
                &identity.signing_handles(),
                pair_sequence,
                StagedSecretHandle::<X25519Private>::from_custody_token(x25519.custody_token()),
                StagedSecretHandle::<MlKem768Private>::from_custody_token(ml_kem.custody_token()),
                now.saturating_sub(OFFER_VALID_BACKDATE_SECONDS),
                now + OFFER_VALID_SECONDS,
            )
            .map_err(|_| anyhow!("devices_pairing_offer_failed"))?;
        self.persist()?;
        Ok(PairingOffer {
            identity: identity.public().clone(),
            prekey,
            handle: bootstrap_handle,
            responder_authority_state_digest: identity.authority_state_digest(),
        })
    }

    /// Authenticates one claim envelope: decodes the submission, resolves the
    /// claimer's identity (the envelope itself is the caller's trust anchor —
    /// the offer code bound this pairing), accepts the handshake, seals the
    /// established snapshot and answers with the reply bound to
    /// `session_handle`. Returns the reply, the claimer's identity and the
    /// handle the reply is submitted to.
    pub(crate) fn accept(
        &mut self,
        consumer: &EndpointConsumer,
        identity: &DeviceIdentity,
        session_handle: HandleToken,
        claim_envelope: &[u8],
    ) -> Result<(PairingReply, IdentityPublic, HandleToken)> {
        let (submission_bytes, initiator_identity) = decode_claim_envelope(claim_envelope)?;
        let submission = PairingSubmission::decode(&submission_bytes)
            .map_err(|_| anyhow!("devices_pairing_submission_invalid"))?;
        ensure!(
            submission.packet.initiator_identity_state_digest == initiator_identity.state_digest,
            "devices_pairing_submission_invalid"
        );
        let trust = StaticTrust::for_identity(&initiator_identity);
        let trusted = consumer
            .trust_peer(&self.line, &trust, &initiator_identity)
            .map_err(|_| anyhow!("devices_pairing_trust_refused"))?;
        let endpoint = match self.lane.take() {
            Some(ResponderLane::Offer(endpoint)) => *endpoint,
            Some(ResponderLane::Session(_)) => bail!("devices_pairing_already_accepted"),
            None => Endpoint::<Responder, _, _, _>::resume(
                self.line.clone(),
                self.provider,
                self.custody_adapter(),
                self.state_adapter(),
                self.state.minimum_generation(),
            )
            .map_err(|_| anyhow!("devices_session_unavailable"))?,
        };
        let clock = SystemClock;
        let (mut session, accept) = consumer
            .accept_handshake(
                endpoint,
                &trusted,
                identity.public(),
                identity.authority_state_digest(),
                &submission.packet,
                &ClockAdapter::new(&clock),
            )
            .map_err(|_| anyhow!("devices_pairing_refused"))?;
        self.state
            .save(session.endpoint_mut())
            .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
        let reply = PairingReply {
            accept,
            session_handle,
        };
        let reply_handle = submission.reply_handle;
        self.lane = Some(ResponderLane::Session(Box::new(session)));
        Ok((reply, initiator_identity, reply_handle))
    }
}

impl InitiatorSession {
    /// Opens the durable initiator endpoint at `dir`.
    pub(crate) fn open(dir: &Path, line: VerifiedProtocolLine) -> Result<Self> {
        let state = SessionState::initiator(dir, &line)?;
        let custody = state.custody().clone();
        Ok(Self {
            line,
            provider: RustCryptoProvider,
            custody,
            state,
            endpoint: None,
        })
    }

    fn custody_adapter(&self) -> CustodyAdapter {
        KeyCustodyAdapter::new(self.custody.clone(), self.custody.clone())
    }

    fn state_adapter(&self) -> StoreAdapter {
        AtomicStateAdapter::new(self.state.clone())
    }

    /// The initiator endpoint for outbound and inbound session work.
    pub(crate) fn endpoint(&mut self) -> Result<&mut InitiatorEndpoint> {
        if self.endpoint.is_none() {
            self.endpoint = Some(
                Endpoint::<Initiator, _, _, _>::resume(
                    self.line.clone(),
                    self.provider,
                    self.custody_adapter(),
                    self.state_adapter(),
                    self.state.minimum_generation(),
                )
                .map_err(|_| anyhow!("devices_session_unavailable"))?,
            );
        }
        self.endpoint
            .as_mut()
            .ok_or_else(|| anyhow!("devices_session_unavailable"))
    }

    /// Seals the live endpoint's committed snapshot.
    pub(crate) fn persist(&mut self) -> Result<()> {
        if let Some(endpoint) = self.endpoint.as_mut() {
            self.state
                .save(endpoint)
                .map_err(|_| anyhow!("devices_session_state_unavailable"))?;
        }
        Ok(())
    }

    /// Builds the claim envelope answering `offer`, bound to `reply_handle`.
    /// The offer code itself is the caller's trust anchor: it names the one
    /// identity this claim admits.
    pub(crate) fn begin(
        &mut self,
        consumer: &EndpointConsumer,
        identity: &DeviceIdentity,
        offer: &PairingOffer,
        reply_handle: HandleToken,
    ) -> Result<Vec<u8>> {
        let trust = StaticTrust::for_identity(&offer.identity);
        let trusted = consumer
            .trust_peer(&self.line, &trust, &offer.identity)
            .map_err(|_| anyhow!("devices_pairing_trust_refused"))?;
        ensure!(
            offer.prekey.responder_identity_state_digest == offer.identity.state_digest,
            "devices_pairing_code_invalid"
        );
        let x25519 = self.custody.stage(CustodyPurpose::X25519Private)?;
        let entropy = self
            .custody
            .adopt(CustodyPurpose::MlKemEncapsulationEntropy)?;
        let clock = SystemClock;
        let packet = self
            .endpoint()?
            .create_first_packet(
                identity.public(),
                &trusted,
                identity.authority_state_digest(),
                offer.responder_authority_state_digest,
                &identity.signing_handles(),
                offer.prekey.clone(),
                &ClockAdapter::new(&clock),
                StagedSecretHandle::<X25519Private>::from_custody_token(x25519.custody_token()),
                SecretHandle::<MlKemEncapsulationEntropy>::from_custody_token(
                    entropy.custody_token(),
                ),
            )
            .map_err(|_| anyhow!("devices_pairing_refused"))?;
        let packet_bytes = licoarc_client::endpoint::encode_first_packet(&packet)
            .map_err(|_| anyhow!("devices_pairing_refused"))?;
        let encoded = PairingSubmission::from_packet_bytes(&packet_bytes, reply_handle)
            .map_err(|_| anyhow!("devices_pairing_submission_invalid"))?;
        self.persist()?;
        encode_claim_envelope(&encoded, identity.public())
    }

    /// Verifies the responder's [`PairingReply`]; on success the session is
    /// established and `session_handle` is where session traffic is submitted.
    pub(crate) fn finish(&mut self, reply_bytes: &[u8]) -> Result<(SessionAccept, HandleToken)> {
        let reply = PairingReply::decode(reply_bytes)
            .map_err(|_| anyhow!("devices_pairing_reply_invalid"))?;
        self.endpoint()?
            .verify_session_accept(&reply.accept)
            .map_err(|_| anyhow!("devices_pairing_refused"))?;
        self.persist()?;
        Ok((reply.accept, reply.session_handle))
    }
}

/// The role one established peer endpoint plays.
#[derive(Clone, Copy)]
pub(crate) enum SessionRole {
    Responder,
    Initiator,
}

/// The long-lived endpoint one paired lane uses for session traffic: record
/// send/receive, durable pending dispatch and plaintext release. The endpoint
/// is resumed eagerly on open and sealed after every state-advancing call.
pub(crate) enum EstablishedSession {
    Responder {
        endpoint: ResponderEndpoint,
        custody: SessionCustody,
        state: SessionState,
    },
    Initiator {
        endpoint: InitiatorEndpoint,
        custody: SessionCustody,
        state: SessionState,
    },
}

impl EstablishedSession {
    /// Opens the established endpoint in `dir` as `role`.
    pub(crate) fn open(dir: &Path, role: SessionRole, line: VerifiedProtocolLine) -> Result<Self> {
        Ok(match role {
            SessionRole::Responder => {
                let state = SessionState::responder(dir, &line)?;
                let custody = state.custody().clone();
                let endpoint = Endpoint::<Responder, _, _, _>::resume(
                    line,
                    RustCryptoProvider,
                    KeyCustodyAdapter::new(custody.clone(), custody.clone()),
                    AtomicStateAdapter::new(state.clone()),
                    state.minimum_generation(),
                )
                .map_err(|_| anyhow!("devices_session_unavailable"))?;
                Self::Responder {
                    endpoint,
                    custody,
                    state,
                }
            }
            SessionRole::Initiator => {
                let state = SessionState::initiator(dir, &line)?;
                let custody = state.custody().clone();
                let endpoint = Endpoint::<Initiator, _, _, _>::resume(
                    line,
                    RustCryptoProvider,
                    KeyCustodyAdapter::new(custody.clone(), custody.clone()),
                    AtomicStateAdapter::new(state.clone()),
                    state.minimum_generation(),
                )
                .map_err(|_| anyhow!("devices_session_unavailable"))?;
                Self::Initiator {
                    endpoint,
                    custody,
                    state,
                }
            }
        })
    }

    /// The coupled custody, for staging receive/send bootstrap keys.
    pub(crate) fn custody_mut(&mut self) -> &mut SessionCustody {
        match self {
            Self::Responder { custody, .. } | Self::Initiator { custody, .. } => custody,
        }
    }

    /// Seals the live endpoint's committed snapshot.
    pub(crate) fn persist(&mut self) -> Result<()> {
        match self {
            Self::Responder {
                endpoint, state, ..
            } => state
                .save(endpoint)
                .map_err(|_| anyhow!("devices_session_state_unavailable")),
            Self::Initiator {
                endpoint, state, ..
            } => state
                .save(endpoint)
                .map_err(|_| anyhow!("devices_session_state_unavailable")),
        }
    }

    /// Commits one outbound record; returns its durable pending id.
    pub(crate) fn send_record(
        &mut self,
        plaintext: &[u8],
        bootstrap: Option<StagedSecretHandle<X25519Private>>,
    ) -> Result<PendingId> {
        match self {
            Self::Responder { endpoint, .. } => endpoint
                .send_record(plaintext, bootstrap)
                .map_err(|_| anyhow!("devices_session_unavailable")),
            Self::Initiator { endpoint, .. } => endpoint
                .send_record(plaintext, bootstrap)
                .map_err(|_| anyhow!("devices_session_unavailable")),
        }
    }

    /// Commits one inbound record; returns its durable pending id.
    pub(crate) fn receive_record(
        &mut self,
        packet: &[u8],
        fresh: Option<StagedSecretHandle<X25519Private>>,
    ) -> Result<PendingId> {
        match self {
            Self::Responder { endpoint, .. } => endpoint
                .receive_record(packet, fresh)
                .map_err(|_| anyhow!("devices_record_refused")),
            Self::Initiator { endpoint, .. } => endpoint
                .receive_record(packet, fresh)
                .map_err(|_| anyhow!("devices_record_refused")),
        }
    }

    /// The durable pending work, in the store's own order.
    pub(crate) fn pending_items(&mut self) -> Result<Vec<PendingItem>> {
        match self {
            Self::Responder { endpoint, .. } => endpoint
                .pending_items()
                .map_err(|_| anyhow!("devices_session_unavailable")),
            Self::Initiator { endpoint, .. } => endpoint
                .pending_items()
                .map_err(|_| anyhow!("devices_session_unavailable")),
        }
    }

    /// Releases one committed plaintext after the receiver succeeds.
    pub(crate) fn release_pending_plaintext<T>(
        &mut self,
        id: PendingId,
        receiver: &mut T,
    ) -> Result<()>
    where
        T: PlaintextReceiver,
    {
        match self {
            Self::Responder { endpoint, .. } => endpoint
                .release_pending_plaintext(id, receiver)
                .map_err(|_| anyhow!("devices_session_unavailable")),
            Self::Initiator { endpoint, .. } => endpoint
                .release_pending_plaintext(id, receiver)
                .map_err(|_| anyhow!("devices_session_unavailable")),
        }
    }
}

/// The drain boundary the mailbox transports serve: the endpoint's durable
/// items in their own order, each settled only after its boundary succeeds.
impl licoarc_client::PendingWork for EstablishedSession {
    fn pending(&self) -> Result<Vec<PendingItem>, Error> {
        match self {
            Self::Responder { endpoint, .. } => endpoint.pending_items(),
            Self::Initiator { endpoint, .. } => endpoint.pending_items(),
        }
    }

    fn dispatch<T: PacketCarrier, E: ApplicationEffects>(
        &mut self,
        id: PendingId,
        carrier: &mut T,
        effects: &mut E,
    ) -> Result<(), Error> {
        match self {
            Self::Responder { endpoint, .. } => endpoint.dispatch_pending(id, carrier, effects),
            Self::Initiator { endpoint, .. } => endpoint.dispatch_pending(id, carrier, effects),
        }
    }

    fn release<L: PlaintextReceiver>(
        &mut self,
        id: PendingId,
        receiver: &mut L,
    ) -> Result<(), Error> {
        match self {
            Self::Responder { endpoint, .. } => endpoint.release_pending_plaintext(id, receiver),
            Self::Initiator { endpoint, .. } => endpoint.release_pending_plaintext(id, receiver),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

fn unhex_32(text: &str) -> Result<[u8; 32]> {
    let bytes = unhex(text).ok_or_else(|| anyhow!("devices_identity_invalid"))?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| anyhow!("devices_identity_invalid"))
}

fn unhex_token(text: &str) -> Result<u128> {
    let bytes = unhex(text).ok_or_else(|| anyhow!("devices_identity_invalid"))?;
    let array =
        <[u8; 16]>::try_from(bytes.as_slice()).map_err(|_| anyhow!("devices_identity_invalid"))?;
    Ok(u128::from_le_bytes(array))
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub(crate) fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64URL[(n >> 18) as usize & 63] as char);
        out.push(BASE64URL[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL[n as usize & 63] as char);
        }
    }
    out
}

pub(crate) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    };
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut acc = 0_u32;
    let mut bits = 0_u32;
    for &byte in bytes {
        acc = (acc << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits > 0 && (acc << (8 - bits)) & 0xff != 0 {
        return None;
    }
    Some(out)
}
