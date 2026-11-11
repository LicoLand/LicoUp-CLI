//! The delivery processor: routes one mailbox packet to its pairing or peer
//! session and produces the outbound work it commits.
//!
//! Both consumers of this module drive the same per-delivery path:
//!
//! 1. the delivery is spooled first — a crash never loses a packet the
//!    Station already handed over;
//! 2. `peer` handles route to the established session (receive, release the
//!    plaintext to the command receiver, checkpoint); `pairing` handles route
//!    to the claim/reply exchange; anything else resolves refused;
//! 3. outbound work is split in two: mailbox-level envelopes (pairing
//!    replies) ride in [`DeviceProcessor::outbox`] for the caller's async
//!    send, and session-level packets are committed to the endpoint then
//!    dispatched through [`DeviceProcessor::drain_outbound`] once a transport
//!    is bound.
//!
//! A command's result record is committed exactly once: the receiver stages
//! its bytes into the endpoint directory's `pending-result` file before the
//! plaintext settles, so a crash after settlement re-commits the same result
//! on the next drain instead of losing it.

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeSet;
use std::path::PathBuf;

use licoarc_client::endpoint::IdentityPublic;
use licoarc_client::state::{
    ApplicationEffects, PendingKind, PlaintextReceiver, StagedSecretHandle, X25519Private,
};
use licoarc_client::{
    CustodyPurpose, EndpointConsumer, Error, ErrorCode, HandleToken, PacketCarrierAdapter,
    PacketSink, PairingReply, Spool, Stage, Verdict, VerifiedProtocolLine, drain_pending,
};

use super::command::{
    DeviceCommandExecutor, DeviceCommandLedger, DeviceRecord, execute_device_command,
};
use super::session::{
    DeviceIdentity, EstablishedSession, InitiatorSession, PeerIdentityJson, ResponderSession,
    SessionRole, base64url_decode, base64url_encode,
};
use super::state::{DevicesState, Pairing, Peer};

fn protocol_unavailable() -> Error {
    Error::terminal(ErrorCode::ProviderFailure, Stage::Commit)
}

fn now_rfc3339() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn session_role(peer: &Peer) -> Result<SessionRole> {
    match peer.role.as_str() {
        "responder" => Ok(SessionRole::Responder),
        "initiator" => Ok(SessionRole::Initiator),
        _ => Err(anyhow!("devices_state_invalid")),
    }
}

/// Storage-class processing failures the Station should redeliver: state,
/// spool, custody and ledger I/O. Everything else — record refusals, pairing
/// refusals, corrupt content — is permanent and resolves refused.
fn transient(error: &anyhow::Error) -> bool {
    matches!(
        error.to_string().as_str(),
        "devices_state_unavailable"
            | "devices_spool_unavailable"
            | "devices_session_state_unavailable"
            | "devices_session_custody_unavailable"
            | "devices_session_custody_unsupported"
            | "device_ledger_unavailable"
            | "devices_not_enrolled"
    )
}

/// One mailbox-level envelope the caller sends with `MailboxClient::send`.
/// `pairing_id` names the pairing row whose `reply_pending` clears once the
/// send is accepted.
pub(crate) struct OutEnvelope {
    pub(crate) handle: HandleToken,
    pub(crate) packet: Vec<u8>,
    pub(crate) pairing_id: Option<String>,
}

/// The receiver side of one committed session record: commands execute once
/// through the ledger, results land once, malformed plaintexts are consumed.
struct PeerReceiver<'a, E: DeviceCommandExecutor> {
    peer_id: &'a str,
    peer_paired: bool,
    ledger: &'a DeviceCommandLedger,
    executor: &'a mut E,
    result_path: PathBuf,
    executed: u64,
    results: u64,
    now: String,
}

impl<E: DeviceCommandExecutor> PlaintextReceiver for PeerReceiver<'_, E> {
    fn release(&mut self, plaintext: &[u8]) -> Result<(), Error> {
        match DeviceRecord::decode(plaintext) {
            Ok(DeviceRecord::Command(command)) => {
                let result = execute_device_command(
                    &command,
                    self.peer_id,
                    self.peer_paired,
                    self.ledger,
                    self.executor,
                    &self.now,
                )
                .map_err(|_| protocol_unavailable())?;
                self.executed += 1;
                let bytes = DeviceRecord::Result(result)
                    .encode()
                    .map_err(|_| protocol_unavailable())?;
                // The result is durable before the plaintext settles: a crash
                // after the settle recommits it on the next outbound drain.
                std::fs::write(&self.result_path, &bytes).map_err(|_| protocol_unavailable())?;
            }
            Ok(DeviceRecord::Result(result)) => {
                self.ledger
                    .record_result(self.peer_id, &result)
                    .map_err(|_| protocol_unavailable())?;
                self.results += 1;
            }
            Err(_) => {
                // A committed record whose plaintext is not a device record is
                // noise from a session peer; consuming it keeps the lane open.
            }
        }
        Ok(())
    }
}

struct NullEffects;

impl ApplicationEffects for NullEffects {
    fn apply(&mut self, _effect: &[u8]) -> Result<(), Error> {
        Ok(())
    }
}

/// The shared per-delivery processor for `devices sync` and the host loop.
pub(crate) struct DeviceProcessor<E: DeviceCommandExecutor> {
    state: DevicesState,
    line: VerifiedProtocolLine,
    consumer: EndpointConsumer,
    identity: DeviceIdentity,
    executor: E,
    spool: Spool,
    pub(crate) received: u64,
    pub(crate) executed: u64,
    pub(crate) results: u64,
    outbox: Vec<OutEnvelope>,
    dirty: BTreeSet<String>,
}

impl<E: DeviceCommandExecutor> DeviceProcessor<E> {
    pub(crate) fn open(
        state: &DevicesState,
        line: VerifiedProtocolLine,
        identity: DeviceIdentity,
        executor: E,
    ) -> Result<Self> {
        Ok(Self {
            spool: Spool::open(&state.spool_dir()).context("devices_spool_unavailable")?,
            state: DevicesState::open(state.dir())?,
            line,
            consumer: EndpointConsumer::new(),
            identity,
            executor,
            received: 0,
            executed: 0,
            results: 0,
            outbox: Vec::new(),
            dirty: BTreeSet::new(),
        })
    }

    /// Mailbox-level envelopes waiting for the caller's async send.
    pub(crate) fn take_outbox(&mut self) -> Vec<OutEnvelope> {
        std::mem::take(&mut self.outbox)
    }

    /// Session directory names with committed outbound work to dispatch.
    pub(crate) fn dirty_sessions(&self) -> Vec<String> {
        self.dirty.iter().cloned().collect()
    }

    /// One delivery: spool, route, process, resolve. `Done` ACKs, `Defer`
    /// leaves the packet stored for redelivery. A permanent processing
    /// refusal resolves the spool file as refused and still ACKs — a broken
    /// lane must not stall the queue forever; only storage-class failures
    /// defer to redelivery.
    pub(crate) fn accept(&mut self, sequence: u64, handle: HandleToken, packet: &[u8]) -> Verdict {
        if self.spool.accept(sequence, handle, packet) != Verdict::Done {
            return Verdict::Defer;
        }
        let path = self.spool.path(handle, sequence);
        match self.route(handle, packet) {
            Ok(consumed) => {
                self.received += 1;
                if self.spool.resolve(&path, !consumed).is_err() {
                    return Verdict::Defer;
                }
                Verdict::Done
            }
            Err(error) => {
                if !transient(&error) {
                    self.received += 1;
                    return match self.spool.resolve(&path, true) {
                        Ok(()) => Verdict::Done,
                        Err(_) => Verdict::Defer,
                    };
                }
                Verdict::Defer
            }
        }
    }

    /// Routes a spooled packet; `Ok(false)` means permanently refused.
    fn route(&mut self, handle: HandleToken, packet: &[u8]) -> Result<bool> {
        let text = handle.as_text();
        if let Some(peer) = self.state.peer_by_handle(&text)? {
            self.peer_packet(&peer, packet)?;
            return Ok(true);
        }
        if let Some(pairing) = self.state.pairing_by_handle(&text)? {
            self.pairing_packet(&pairing, packet)?;
            return Ok(true);
        }
        // A handle nothing owns: a stale bootstrap, a replayed claim, or a
        // foreign packet — resolved refused, never ACKed into a session.
        Ok(false)
    }

    /// Session traffic on an established peer lane.
    fn peer_packet(&mut self, peer: &Peer, packet: &[u8]) -> Result<()> {
        let dir = self.state.session_dir(&peer.session_dir);
        let mut session = EstablishedSession::open(&dir, session_role(peer)?, self.line.clone())?;
        let fresh = session
            .custody_mut()
            .stage(CustodyPurpose::X25519Private)
            .map_err(|_| anyhow!("devices_session_custody_unavailable"))?;
        let pending = session.receive_record(
            packet,
            Some(StagedSecretHandle::<X25519Private>::from_custody_token(
                fresh.custody_token(),
            )),
        )?;
        session.persist()?;
        let ledger = self.state.ledger()?;
        let mut receiver = PeerReceiver {
            peer_id: &peer.peer_id,
            peer_paired: self.state.peer(&peer.peer_id)?.is_some(),
            ledger: &ledger,
            executor: &mut self.executor,
            result_path: dir.join("pending-result"),
            executed: 0,
            results: 0,
            now: now_rfc3339(),
        };
        session.release_pending_plaintext(pending, &mut receiver)?;
        session.persist()?;
        self.executed += receiver.executed;
        self.results += receiver.results;
        self.dirty.insert(peer.session_dir.clone());
        Ok(())
    }

    /// A pairing packet: a claim envelope on the offerer's bootstrap handle,
    /// or a reply on the claimer's reply handle.
    fn pairing_packet(&mut self, pairing: &Pairing, packet: &[u8]) -> Result<()> {
        match (pairing.role.as_str(), pairing.state.as_str()) {
            ("offerer", "awaiting-claim") => self.accept_claim(pairing, packet),
            ("offerer", "completed") => {
                // A redelivered claim after acceptance: re-send the reply the
                // pairing already produced — the accept is not replayable.
                let reply = pairing
                    .reply_b64
                    .as_deref()
                    .ok_or_else(|| anyhow!("devices_state_invalid"))?;
                let bytes =
                    base64url_decode(reply).ok_or_else(|| anyhow!("devices_state_invalid"))?;
                PairingReply::decode(&bytes).map_err(|_| anyhow!("devices_state_invalid"))?;
                self.outbox.push(OutEnvelope {
                    handle: HandleToken::parse(
                        pairing
                            .reply_handle
                            .as_deref()
                            .ok_or_else(|| anyhow!("devices_state_invalid"))?,
                    )
                    .map_err(|_| anyhow!("devices_state_invalid"))?,
                    packet: bytes,
                    pairing_id: Some(pairing.pairing_id.clone()),
                });
                Ok(())
            }
            ("claimer", "awaiting-reply") => self.finish_claim(pairing, packet),
            ("claimer", "completed") => Ok(()),
            _ => Ok(()),
        }
    }

    /// The offerer authenticates one claim envelope and answers with the
    /// pairing reply bound to its fresh session handle.
    fn accept_claim(&mut self, pairing: &Pairing, packet: &[u8]) -> Result<()> {
        let dir = self.state.session_dir(&pairing.session_dir);
        let mut responder = ResponderSession::open(&dir, self.line.clone())?;
        let mailbox = self.mailbox_id()?;
        let session_handle = HandleToken::mint(mailbox)?;
        let (reply, claimer, reply_handle) =
            responder.accept(&self.consumer, &self.identity, session_handle, packet)?;
        let reply_bytes = reply
            .encode()
            .map_err(|_| anyhow!("devices_pairing_reply_invalid"))?;
        self.outbox.push(OutEnvelope {
            handle: reply_handle,
            packet: reply_bytes.clone(),
            pairing_id: Some(pairing.pairing_id.clone()),
        });
        // The accepted claim becomes the peer; the pairing row stays
        // `completed` with the reply bytes so a redelivered claim re-answers.
        let peer = Peer {
            peer_id: DeviceIdentity::peer_id_of(&claimer),
            label: pairing.label.clone().unwrap_or_default(),
            paired_at: now_rfc3339(),
            role: "responder".to_owned(),
            local_handle: session_handle.as_text(),
            remote_handle: reply_handle.as_text(),
            session_dir: pairing.session_dir.clone(),
            peer_identity: serde_json::to_value(PeerIdentityJson::of(&claimer))?,
            send_bootstrapped: false,
        };
        self.state.upsert_peer(&peer)?;
        let mut done = pairing.clone();
        done.state = "completed".to_owned();
        done.peer_id = Some(peer.peer_id.clone());
        done.peer_identity = Some(peer.peer_identity.clone());
        done.reply_b64 = Some(base64url_encode(&reply_bytes));
        done.reply_handle = Some(reply_handle.as_text());
        done.reply_pending = true;
        self.state.upsert_pairing(&done)?;
        Ok(())
    }

    /// The claimer verifies the responder's reply and records the peer.
    fn finish_claim(&mut self, pairing: &Pairing, packet: &[u8]) -> Result<()> {
        let dir = self.state.session_dir(&pairing.session_dir);
        let mut initiator = InitiatorSession::open(&dir, self.line.clone())?;
        let (_accept, remote) = initiator.finish(packet)?;
        let offerer_json = pairing
            .peer_identity
            .clone()
            .ok_or_else(|| anyhow!("devices_state_invalid"))?;
        let offerer: IdentityPublic = serde_json::from_value::<PeerIdentityJson>(offerer_json)
            .map_err(|_| anyhow!("devices_state_invalid"))?
            .into_public()?;
        let peer = Peer {
            peer_id: DeviceIdentity::peer_id_of(&offerer),
            label: pairing.label.clone().unwrap_or_default(),
            paired_at: now_rfc3339(),
            role: "initiator".to_owned(),
            local_handle: pairing.handle.clone(),
            remote_handle: remote.as_text(),
            session_dir: pairing.session_dir.clone(),
            peer_identity: serde_json::to_value(PeerIdentityJson::of(&offerer))?,
            send_bootstrapped: false,
        };
        self.state.upsert_peer(&peer)?;
        let mut done = pairing.clone();
        done.state = "completed".to_owned();
        done.peer_id = Some(peer.peer_id.clone());
        self.state.upsert_pairing(&done)?;
        Ok(())
    }

    /// Commits one outbound record to `peer`'s session and marks the lane
    /// dirty for [`Self::drain_outbound`].
    pub(crate) fn send_record(&mut self, peer_id: &str, bytes: &[u8]) -> Result<()> {
        let peer = self
            .state
            .peer(peer_id)?
            .ok_or_else(|| anyhow!("devices_peer_not_found"))?;
        let dir = self.state.session_dir(&peer.session_dir);
        let mut session = EstablishedSession::open(&dir, session_role(&peer)?, self.line.clone())?;
        let mut peer = peer;
        send_peer_record(&mut session, &mut peer, bytes)?;
        session.persist()?;
        self.state.upsert_peer(&peer)?;
        self.dirty.insert(peer.session_dir.clone());
        Ok(())
    }

    /// Pairing replies whose send was never confirmed; the caller re-sends
    /// each and clears the flag with [`Self::confirm_reply`].
    pub(crate) fn pending_replies(&self) -> Result<Vec<OutEnvelope>> {
        Ok(self
            .state
            .pairing_rows()?
            .into_iter()
            .filter(|pairing| pairing.reply_pending)
            .filter_map(|pairing| {
                Some(OutEnvelope {
                    pairing_id: Some(pairing.pairing_id),
                    handle: HandleToken::parse(pairing.reply_handle.as_deref()?).ok()?,
                    packet: base64url_decode(pairing.reply_b64.as_deref()?)?,
                })
            })
            .collect())
    }

    /// Marks a pairing's stored reply as delivered: it still re-answers a
    /// redelivered claim, but no drain start re-sends it unprompted.
    pub(crate) fn confirm_reply(&mut self, pairing_id: &str) -> Result<()> {
        if let Some(mut pairing) = self.state.pairing(pairing_id)?
            && pairing.reply_pending
        {
            pairing.reply_pending = false;
            self.state.upsert_pairing(&pairing)?;
        }
        Ok(())
    }

    /// Sends everything the endpoint committed, in the store's own order:
    /// leftover packets and plaintexts first, then any result the receiver
    /// staged, then dispatches the commit. `carrier` binds the peer's remote
    /// handle (a `SubmitCarrier` for argv, the host's bridged transport).
    pub(crate) fn drain_outbound(
        &mut self,
        session_dir: &str,
        carrier: impl licoarc_client::ports::Transport,
    ) -> Result<()> {
        let Some(peer) = self.state.peer_by_session_dir(session_dir)? else {
            self.dirty.remove(session_dir);
            return Ok(());
        };
        let dir = self.state.session_dir(session_dir);
        let mut session = EstablishedSession::open(&dir, session_role(&peer)?, self.line.clone())?;
        let mut adapter = PacketCarrierAdapter::new(carrier);
        let mut effects = NullEffects;
        let ledger = self.state.ledger()?;
        let mut receiver = PeerReceiver {
            peer_id: &peer.peer_id,
            peer_paired: true,
            ledger: &ledger,
            executor: &mut self.executor,
            result_path: dir.join("pending-result"),
            executed: 0,
            results: 0,
            now: now_rfc3339(),
        };
        loop {
            drain_pending(&mut session, &mut adapter, &mut effects, &mut receiver)
                .map_err(|_| anyhow!("devices_session_unavailable"))?;
            session.persist()?;
            self.executed += receiver.executed;
            self.results += receiver.results;
            receiver.executed = 0;
            receiver.results = 0;
            let result_path = dir.join("pending-result");
            if !result_path.is_file() {
                break;
            }
            let packet_pending = session
                .pending_items()?
                .iter()
                .any(|item| matches!(item.kind(), PendingKind::Packet(_)));
            if packet_pending {
                // The result's packet is committed but not yet dispatched;
                // the next pass retries it, and later drains keep retrying.
                break;
            }
            let bytes = std::fs::read(&result_path)?;
            let mut peer = peer.clone();
            send_peer_record(&mut session, &mut peer, &bytes)?;
            session.persist()?;
            std::fs::remove_file(&result_path)?;
            self.state.upsert_peer(&peer)?;
        }
        self.dirty.remove(session_dir);
        Ok(())
    }

    fn mailbox_id(&self) -> Result<[u8; 16]> {
        let enrollment = self
            .state
            .enrollment()?
            .ok_or_else(|| anyhow!("devices_not_enrolled"))?;
        super::decode_mailbox_id(&enrollment.mailbox_id)
    }
}

impl<E: DeviceCommandExecutor> PacketSink for DeviceProcessor<E> {
    fn accept(&mut self, sequence: u64, handle: HandleToken, packet: &[u8]) -> Verdict {
        DeviceProcessor::accept(self, sequence, handle, packet)
    }
}

/// One outbound record on an established session: responder endpoints need a
/// staged X25519 bootstrap on their first send, which `send_bootstrapped`
/// tracks; a lost flag retries without bootstrap before refusing.
fn send_peer_record(session: &mut EstablishedSession, peer: &mut Peer, bytes: &[u8]) -> Result<()> {
    let staged = if peer.role == "responder" && !peer.send_bootstrapped {
        session
            .custody_mut()
            .stage(CustodyPurpose::X25519Private)
            .ok()
            .map(|handle| {
                StagedSecretHandle::<X25519Private>::from_custody_token(handle.custody_token())
            })
    } else {
        None
    };
    let bootstrapping = staged.is_some();
    match session.send_record(bytes, staged) {
        Ok(_) => {
            if peer.role == "responder" {
                peer.send_bootstrapped = true;
            }
            Ok(())
        }
        Err(first) => {
            if bootstrapping {
                // The send chain already exists (the flag was lost): the SDK
                // aborted the staged key; retry without bootstrap.
                session.send_record(bytes, None).map(|_| {
                    peer.send_bootstrapped = true;
                })
            } else {
                Err(first)
            }
        }
    }
}
