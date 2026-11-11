//! The `devices` command group: LicoArc mailbox enrollment, device pairing and
//! agent messages to a paired device.
//!
//! Everything lives under `<data home>/client-state/devices/`, owner-only:
//! the enrollment record (Station URL, admission credential, mailbox id), the
//! Station CA, the peer and pairing tables, the command ledger, the packet
//! spool and one session-endpoint directory per lane. The device's mailbox
//! key and every session secret never touch that directory: they are held by
//! the OS secret store ([`custody`], [`session_custody`]).
//!
//! The pre-LicoArc relay's state under `client-state/mobile-relay/` is never
//! read, migrated or deleted; its presence is reported once as
//! `legacyPairingsDetected` so the user is asked to pair again.

pub mod command;
mod custody;
pub mod host;
mod processor;
mod session;
mod session_custody;
mod session_state;
mod state;

#[cfg(test)]
mod tests;

use anyhow::{Context, Result, anyhow, bail, ensure};
use licoarc_client::{
    Credentials, EndpointConsumer, Event, HandleToken, MailboxClient, SubmitCarrier, Verdict,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub use custody::DEVICE_KEY_USER_PRESENCE_ENV;
use host::AgentMessageExecutor;
pub(crate) use processor::DeviceProcessor;
pub(crate) use session::DeviceIdentity;
use session::{
    InitiatorSession, ResponderSession, decode_offer_code, device_line, encode_offer_code,
};
use session_custody::SessionCustody;
use state::{DevicesState, Enrollment, Pairing};

const DEVICES_DIR: &str = "client-state/devices";
const LEGACY_RELAY_DIR: &str = "client-state/mobile-relay";
const MAX_LABEL_BYTES: usize = 128;
const MAX_CA_BYTES: usize = 64 * 1024;

/// The directory every `devices` record lives in, for one data home.
pub fn devices_dir(data_home: &Path) -> PathBuf {
    data_home.join(DEVICES_DIR)
}

fn data_home() -> Result<PathBuf> {
    licoup_foundation::platform::paths::portable_data_dir()
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fresh_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// `devices status`.
pub fn status() -> Result<Value> {
    status_in(&data_home()?)
}

pub(crate) fn status_in(data_home: &Path) -> Result<Value> {
    let state = DevicesState::open(&devices_dir(data_home))?;
    let enrollment = state.enrollment()?;
    Ok(json!({
        "ok": true,
        "enrolled": enrollment.is_some(),
        "legacyPairingsDetected": data_home.join(LEGACY_RELAY_DIR).exists(),
        "station": enrollment.as_ref().map(|record| record.station.clone()),
        "mailboxId": enrollment.as_ref().map(|record| record.mailbox_id.clone()),
        "peers": state.peers()?,
        "pairings": state.pairings()?,
    }))
}

/// `devices enroll --stdin-json`: bind a fresh device key to the invited
/// mailbox and record the mailbox id the Station assigns.
pub fn enroll(input: &Value) -> Result<Value> {
    enroll_in(&data_home()?, input)
}

pub(crate) fn enroll_in(data_home: &Path, input: &Value) -> Result<Value> {
    let object = input
        .as_object()
        .ok_or_else(|| anyhow!("devices_enroll_input_invalid"))?;
    for key in object.keys() {
        ensure!(
            ["station", "invitation", "ca"].contains(&key.as_str()),
            "devices_enroll_input_invalid"
        );
    }
    let station = input
        .get("station")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| anyhow!("devices_enroll_input_invalid"))?;
    let parsed = url::Url::parse(station).map_err(|_| anyhow!("devices_station_invalid"))?;
    ensure!(
        parsed.scheme() == "https" && parsed.host_str().is_some(),
        "devices_station_invalid"
    );
    let invitation = input
        .get("invitation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("btm1."))
        .ok_or_else(|| anyhow!("devices_invitation_invalid"))?;
    let ca = match input.get("ca") {
        None | Some(Value::Null) => None,
        Some(Value::String(pem)) => {
            ensure!(
                pem.len() <= MAX_CA_BYTES && pem.contains("-----BEGIN CERTIFICATE-----"),
                "devices_ca_invalid"
            );
            Some(pem.as_str())
        }
        Some(_) => bail!("devices_ca_invalid"),
    };
    let dir = devices_dir(data_home);
    let state = DevicesState::open(&dir)?;
    ensure!(state.enrollment()?.is_none(), "devices_already_enrolled");
    let ca_path = match ca {
        Some(pem) => Some(state.write_ca(pem)?),
        None => {
            state.remove_ca()?;
            None
        }
    };
    let custody = custody::DeviceKeyCustody::for_devices_dir(&dir)?;
    let seed = custody.mint()?;
    let credentials = Credentials::new(station.to_owned(), invitation.to_owned(), seed, None)
        .map_err(|_| anyhow!("devices_invitation_invalid"))?;
    let roots = ca_path.into_iter().collect::<Vec<_>>();
    let connected = runtime()?.block_on(MailboxClient::connect(&credentials, &roots));
    let client = match connected {
        Ok(client) => client,
        Err(error) => {
            custody.remove()?;
            bail!("{}", station_error_code(&error));
        }
    };
    let mailbox_id = hex(&client.mailbox_id());
    drop(client);
    state.write_enrollment(&Enrollment {
        station: station.to_owned(),
        invitation: invitation.to_owned(),
        mailbox_id: mailbox_id.clone(),
        enrolled_at: now_rfc3339(),
    })?;
    Ok(json!({ "ok": true, "mailboxId": mailbox_id }))
}

/// `devices pair offer [--label TEXT]`: stage a prekey, publish a pairing
/// offer bound to a fresh bootstrap handle and record the pending pairing.
pub fn pair_offer(label: Option<&str>) -> Result<Value> {
    pair_offer_in(&data_home()?, label)
}

pub(crate) fn pair_offer_in(data_home: &Path, label: Option<&str>) -> Result<Value> {
    validate_label(label)?;
    let enrollment = require_enrollment(data_home)?;
    let dir = devices_dir(data_home);
    let state = DevicesState::open(&dir)?;
    let line = device_line()?;
    let identity = DeviceIdentity::open(&state.identity_dir())?;
    let pairing_id = format!("pr-{}", fresh_id());
    let session_dir = format!("s-{pairing_id}");
    let mut responder = ResponderSession::open(&state.session_dir(&session_dir), line)?;
    let bootstrap = HandleToken::mint(decode_mailbox_id(&enrollment.mailbox_id)?)?;
    let offer = responder.issue_offer(
        &identity,
        bootstrap,
        state.next_pair_sequence()?,
        now_unix(),
    )?;
    state.upsert_pairing(&Pairing {
        pairing_id: pairing_id.clone(),
        role: "offerer".to_owned(),
        state: "awaiting-claim".to_owned(),
        created_at: now_rfc3339(),
        handle: bootstrap.as_text(),
        session_dir,
        peer_identity: None,
        label: label.map(str::to_owned),
        peer_id: None,
        reply_b64: None,
        reply_handle: None,
        reply_pending: false,
    })?;
    Ok(json!({
        "ok": true,
        "pairingId": pairing_id,
        "code": encode_offer_code(&offer)?,
    }))
}

/// One built claim: the envelope to submit, the handle it goes to and the
/// pairing row to record once the mailbox accepts it.
pub(crate) struct ClaimPrepared {
    pub(crate) pairing: Pairing,
    pub(crate) envelope: Vec<u8>,
    pub(crate) offer_handle: HandleToken,
}

/// Builds the claim envelope for `devices pair claim`: validates the input,
/// decodes the offer code and creates the initiator endpoint. On any failure
/// the half-made session directory is destroyed.
pub(crate) fn pair_claim_prepare(data_home: &Path, input: &Value) -> Result<ClaimPrepared> {
    let code = input
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .ok_or_else(|| anyhow!("devices_pairing_code_invalid"))?;
    ensure!(
        code.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "devices_pairing_code_invalid"
    );
    validate_label(input.get("label").and_then(Value::as_str))?;
    let enrollment = require_enrollment(data_home)?;
    let dir = devices_dir(data_home);
    let state = DevicesState::open(&dir)?;
    let offer = decode_offer_code(code)?;
    let line = device_line()?;
    let identity = DeviceIdentity::open(&state.identity_dir())?;
    let pairing_id = format!("pr-{}", fresh_id());
    let session_dir = format!("s-{pairing_id}");
    let prepared = (|| -> Result<(Vec<u8>, HandleToken)> {
        let consumer = EndpointConsumer::new();
        let mut initiator = InitiatorSession::open(&state.session_dir(&session_dir), line)?;
        let reply_handle = HandleToken::mint(decode_mailbox_id(&enrollment.mailbox_id)?)?;
        let envelope = initiator.begin(&consumer, &identity, &offer, reply_handle)?;
        Ok((envelope, reply_handle))
    })();
    let (envelope, reply_handle) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            destroy_session(&state, &session_dir);
            return Err(error);
        }
    };
    Ok(ClaimPrepared {
        pairing: Pairing {
            pairing_id,
            role: "claimer".to_owned(),
            state: "awaiting-reply".to_owned(),
            created_at: now_rfc3339(),
            handle: reply_handle.as_text(),
            session_dir,
            peer_identity: Some(serde_json::to_value(session::PeerIdentityJson::of(
                &offer.identity,
            ))?),
            label: input
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned),
            peer_id: None,
            reply_b64: None,
            reply_handle: None,
            reply_pending: false,
        },
        envelope,
        offer_handle: offer.handle,
    })
}

/// `devices pair claim --stdin-json`: resolve the offer's identity through
/// the code itself, build the claim envelope and submit it to the offerer's
/// bootstrap handle.
pub fn pair_claim(input: &Value) -> Result<Value> {
    let home = data_home()?;
    let enrollment = require_enrollment(&home)?;
    let dir = devices_dir(&home);
    let state = DevicesState::open(&dir)?;
    let prepared = pair_claim_prepare(&home, input)?;
    let credentials = credentials(&dir, &enrollment)?;
    let roots = state.ca_path()?.into_iter().collect::<Vec<_>>();
    let sent = runtime()?.block_on(async {
        let client = MailboxClient::connect(&credentials, &roots).await?;
        client.send(prepared.offer_handle, &prepared.envelope).await
    });
    match sent {
        Ok(()) => {}
        Err(licoarc_client::StationError::Ambiguous) => {
            // The submit may have landed; keep the lane so the reply routes.
        }
        Err(error) => {
            destroy_session(&state, &prepared.pairing.session_dir);
            bail!("{}", station_error_code(&error));
        }
    }
    let pairing_id = prepared.pairing.pairing_id.clone();
    state.upsert_pairing(&prepared.pairing)?;
    Ok(json!({ "ok": true, "pairingId": pairing_id }))
}

/// `devices pair cancel --pairing-id ID`: drop the pending pairing and every
/// trace of its half-made session.
pub fn pair_cancel(pairing_id: &str) -> Result<Value> {
    pair_cancel_in(&data_home()?, pairing_id)
}

pub(crate) fn pair_cancel_in(data_home: &Path, pairing_id: &str) -> Result<Value> {
    let state = DevicesState::open(&devices_dir(data_home))?;
    let pairing = state
        .pairing(pairing_id)?
        .ok_or_else(|| anyhow!("devices_pairing_not_found"))?;
    // A completed pairing shares its session directory with the live peer —
    // ending it is `devices unpair`'s job, never a blind directory removal.
    ensure!(pairing.state != "completed", "devices_pairing_completed");
    destroy_session(&state, &pairing.session_dir);
    state.remove_pairing(pairing_id)?;
    Ok(json!({ "ok": true }))
}

/// `devices unpair --peer-id ID`: the peer's later commands are refused.
pub fn unpair(peer_id: &str) -> Result<Value> {
    unpair_in(&data_home()?, peer_id)
}

pub(crate) fn unpair_in(data_home: &Path, peer_id: &str) -> Result<Value> {
    let dir = devices_dir(data_home);
    let state = DevicesState::open(&dir)?;
    let peer = state
        .peer(peer_id)?
        .ok_or_else(|| anyhow!("devices_peer_not_found"))?;
    destroy_session(&state, &peer.session_dir);
    for pairing in state.pairings_by_peer(peer_id)? {
        state.remove_pairing(&pairing.pairing_id)?;
    }
    ensure!(state.remove_peer(peer_id)?, "devices_peer_not_found");
    state.ledger()?.forget_peer(peer_id)?;
    Ok(json!({ "ok": true }))
}

/// A prepared outbound message: `duplicate` marks an intent already mapped
/// to a command id — its record stays empty and nothing is sent again.
pub(crate) struct PreparedSend {
    pub(crate) command_id: String,
    pub(crate) session_dir: String,
    pub(crate) record: Vec<u8>,
    pub(crate) duplicate: bool,
}

/// Validates one `message send` and binds the client intent to exactly one
/// command record — the binding is recorded before anything is sent, so a
/// retry of the same intent can never mint a second execution.
pub(crate) fn message_send_prepare_in(
    data_home: &Path,
    peer_id: &str,
    agent: &str,
    session_id: Option<&str>,
    input: &Value,
) -> Result<PreparedSend> {
    let body = input
        .get("body")
        .and_then(Value::as_str)
        .filter(|body| !body.trim().is_empty())
        .ok_or_else(|| anyhow!("devices_message_body_required"))?;
    let client_intent_id = input
        .get("clientIntentId")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| anyhow!("devices_client_intent_required"))?;
    ensure!(!agent.trim().is_empty(), "agent_binding_required");
    let dir = devices_dir(data_home);
    let state = DevicesState::open(&dir)?;
    let peer = state
        .peer(peer_id)?
        .ok_or_else(|| anyhow!("devices_peer_not_found"))?;
    if let Some((command_id, sent_to)) = state.ledger()?.outbound_command(client_intent_id)? {
        ensure!(sent_to == peer_id, "devices_client_intent_conflict");
        return Ok(PreparedSend {
            command_id,
            session_dir: peer.session_dir,
            record: Vec::new(),
            duplicate: true,
        });
    }
    let command_id = format!("cmd-{}", fresh_id());
    let command = command::DeviceCommand::agent_message(
        command_id.clone(),
        agent,
        session_id,
        body,
        now_rfc3339(),
    );
    let record = command::DeviceRecord::Command(command).encode()?;
    state
        .ledger()?
        .record_outbound(client_intent_id, &command_id, peer_id, &now_rfc3339())?;
    Ok(PreparedSend {
        command_id,
        session_dir: peer.session_dir,
        record,
        duplicate: false,
    })
}

/// `devices message send --peer-id ID --agent AGENT [--session-id ID] --stdin-json`.
pub fn message_send(
    peer_id: &str,
    agent: &str,
    session_id: Option<&str>,
    input: &Value,
) -> Result<Value> {
    let data_home = data_home()?;
    let enrollment = require_enrollment(&data_home)?;
    let dir = devices_dir(&data_home);
    let state = DevicesState::open(&dir)?;
    let peer = state
        .peer(peer_id)?
        .ok_or_else(|| anyhow!("devices_peer_not_found"))?;
    let prepared = message_send_prepare_in(&data_home, peer_id, agent, session_id, input)?;
    if prepared.duplicate {
        return Ok(json!({ "ok": true, "commandId": prepared.command_id }));
    }
    let line = device_line()?;
    let identity = DeviceIdentity::open(&state.identity_dir())?;
    let mut processor = DeviceProcessor::open(&state, line, identity, AgentMessageExecutor)?;
    processor.send_record(peer_id, &prepared.record)?;
    let credentials = credentials(&dir, &enrollment)?;
    let roots = state.ca_path()?.into_iter().collect::<Vec<_>>();
    // The runtime must outlive the client: `SubmitCarrier` drives the
    // connection's I/O task through the client's own runtime handle.
    let runtime = runtime()?;
    let client = runtime
        .block_on(MailboxClient::connect(&credentials, &roots))
        .map_err(|error| anyhow!("{}", station_error_code(&error)))?;
    let remote =
        HandleToken::parse(&peer.remote_handle).map_err(|_| anyhow!("devices_state_invalid"))?;
    let carrier = SubmitCarrier::new(Arc::new(client), remote)
        .map_err(|_| anyhow!("devices_station_unavailable"))?;
    processor.drain_outbound(&prepared.session_dir, carrier)?;
    Ok(json!({ "ok": true, "commandId": prepared.command_id }))
}

/// `devices message results [--peer-id ID] [--since RFC3339]`.
pub fn message_results(peer_id: Option<&str>, since: Option<&str>) -> Result<Value> {
    message_results_in(&data_home()?, peer_id, since)
}

pub(crate) fn message_results_in(
    data_home: &Path,
    peer_id: Option<&str>,
    since: Option<&str>,
) -> Result<Value> {
    let state = DevicesState::open(&devices_dir(data_home))?;
    Ok(json!({ "ok": true, "results": state.ledger()?.results(peer_id, since)? }))
}

/// `devices sync`: one drain of the mailbox until the Station reports
/// DRAINED, then one dispatch pass over everything the endpoint committed.
pub fn sync() -> Result<Value> {
    let data_home = data_home()?;
    let enrollment = require_enrollment(&data_home)?;
    let dir = devices_dir(&data_home);
    let state = DevicesState::open(&dir)?;
    let line = device_line()?;
    let identity = DeviceIdentity::open(&state.identity_dir())?;
    let mut processor = DeviceProcessor::open(&state, line, identity, AgentMessageExecutor)?;
    let credentials = credentials(&dir, &enrollment)?;
    let roots = state.ca_path()?.into_iter().collect::<Vec<_>>();
    // The runtime must outlive the client: `SubmitCarrier` drives the
    // connection's I/O task through the client's own runtime handle.
    let runtime = runtime()?;
    let client = runtime.block_on(async {
        let mut client = MailboxClient::connect(&credentials, &roots)
            .await
            .map_err(|error| anyhow!("{}", station_error_code(&error)))?;
        for envelope in processor.pending_replies()? {
            if client.send(envelope.handle, &envelope.packet).await.is_ok()
                && let Some(pairing_id) = &envelope.pairing_id
            {
                processor.confirm_reply(pairing_id)?;
            }
        }
        loop {
            match client
                .next()
                .await
                .map_err(|error| anyhow!("{}", station_error_code(&error)))?
            {
                Event::Drained => break,
                Event::Deliver {
                    sequence,
                    handle,
                    packet,
                } => match processor.accept(sequence, handle, &packet) {
                    Verdict::Done => {
                        for envelope in processor.take_outbox() {
                            if client.send(envelope.handle, &envelope.packet).await.is_ok()
                                && let Some(pairing_id) = &envelope.pairing_id
                            {
                                processor.confirm_reply(pairing_id)?;
                            }
                        }
                        client
                            .ack(sequence)
                            .await
                            .map_err(|error| anyhow!("{}", station_error_code(&error)))?;
                    }
                    Verdict::Defer => bail!("devices_spool_unavailable"),
                },
            }
        }
        Ok::<_, anyhow::Error>(client)
    })?;
    let client = Arc::new(client);
    let mut first_failure = None;
    // Every peer session is drained, not only lanes this pass dirtied:
    // committed-but-undispatched work is durable and survives a crashed or
    // failed earlier drain, and nothing else re-dispatches it.
    for peer in state.peer_rows()? {
        let Ok(remote) = HandleToken::parse(&peer.remote_handle) else {
            continue;
        };
        let Ok(carrier) = SubmitCarrier::new(Arc::clone(&client), remote) else {
            continue;
        };
        if let Err(error) = processor.drain_outbound(&peer.session_dir, carrier) {
            first_failure.get_or_insert(error);
        }
    }
    if let Some(error) = first_failure {
        return Err(error);
    }
    Ok(json!({
        "ok": true,
        "received": processor.received,
        "executed": processor.executed,
        "results": processor.results,
    }))
}

/// Removes one session endpoint's custody item and directory; used by
/// `pair cancel` and `unpair`. A custody backend failure never blocks the
/// local row removal: the namespace is bound to the deleted directory.
fn destroy_session(state: &DevicesState, session_dir: &str) {
    let dir = state.session_dir(session_dir);
    if let Ok(custody) = SessionCustody::open(&dir) {
        let _ = custody.destroy();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn require_enrollment(data_home: &Path) -> Result<Enrollment> {
    DevicesState::open(&devices_dir(data_home))?
        .enrollment()?
        .ok_or_else(|| anyhow!("devices_not_enrolled"))
}

pub(crate) fn credentials(dir: &Path, enrollment: &Enrollment) -> Result<Credentials> {
    let seed = custody::DeviceKeyCustody::for_devices_dir(dir)?.load()?;
    let mailbox = decode_mailbox_id(&enrollment.mailbox_id)?;
    Credentials::new(
        enrollment.station.clone(),
        enrollment.invitation.clone(),
        seed,
        Some(mailbox),
    )
    .map_err(|_| anyhow!("devices_enrollment_invalid"))
}

fn decode_mailbox_id(text: &str) -> Result<[u8; 16]> {
    ensure!(
        text.len() == 32 && text.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "devices_enrollment_invalid"
    );
    let mut out = [0_u8; 16];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| anyhow!("devices_enrollment_invalid"))?;
    }
    Ok(out)
}

fn validate_label(label: Option<&str>) -> Result<()> {
    if let Some(label) = label {
        ensure!(
            !label.trim().is_empty() && label.len() <= MAX_LABEL_BYTES,
            "devices_label_invalid"
        );
    }
    Ok(())
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("devices_runtime_unavailable")
}

pub(crate) fn station_error_code(error: &licoarc_client::StationError) -> &'static str {
    use licoarc_client::StationError;
    match error {
        StationError::Refused => "devices_station_refused",
        StationError::Revoked => "devices_mailbox_revoked",
        StationError::DeviceMismatch => "devices_device_key_mismatch",
        StationError::Superseded => "devices_mailbox_superseded",
        StationError::Protocol => "devices_station_protocol_violation",
        StationError::Rejected { .. } => "devices_station_rejected",
        StationError::Transient { .. } | StationError::Ambiguous | StationError::Io(_) => {
            "devices_station_unavailable"
        }
    }
}
