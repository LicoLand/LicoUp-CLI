// mobile relay peer commands: the cross-device production entry
//
// The routes below are the native surface of
// `domain::cross_device_entry`. They are the real callers of that entry: the
// protocol line is admitted through `ProtocolWorkSetup`, an inbound packet goes
// to the entry's device edge, and the conversation row is written by the
// entry's peer ingress. Nothing here decides a protocol outcome, verifies a
// packet, or creates a membership; the routes only read what the caller
// supplied and report what the owner answered.

use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

use super::{AdmittedCommand, CliExecution, handler_error};
use crate::domain::client_conversation::ConversationStore;
use crate::domain::client_conversation::peer_ingress::{AdmissionFact, PeerBinding};
use crate::domain::cross_device_entry;

/// Largest accepted authority artifact, in bytes.
const MAX_AUTHORITY_ARTIFACT_BYTES: u64 = 256 * 1024;

/// Largest accepted protected packet, in bytes.
const MAX_PROTECTED_PACKET_BYTES: u64 = 1024 * 1024;

pub(super) fn handle_mobile_peer(command: AdmittedCommand) -> Result<CliExecution> {
    let params = command.option_json("stdin-json").cloned();
    let result = match command.path() {
        ["mobile", "relay", "peer", "status"] => status()?,
        ["mobile", "relay", "peer", "protocol-line"] => {
            protocol_line(command.option_text("authority-file").unwrap_or_default())?
        }
        ["mobile", "relay", "peer", "record"] => record(&required_params(params, "record")?)?,
        ["mobile", "relay", "peer", "bind"] => bind(&required_params(params, "bind")?)?,
        ["mobile", "relay", "peer", "revoke"] => revoke(&required_params(params, "revoke")?)?,
        _ => return Err(handler_error("command_failed", "use_cli_help").into()),
    };
    Ok(CliExecution::Json(result))
}

fn required_params(params: Option<Value>, action: &str) -> Result<Value> {
    let params = params.ok_or_else(|| anyhow!("cross_device_request_missing:{action}"))?;
    ensure!(
        params.is_object(),
        "cross_device_request_malformed:{action}"
    );
    Ok(params)
}

/// The entry's own state, read without changing it.
fn status() -> Result<Value> {
    let observed = cross_device_entry::with_entry(|entry| {
        (
            entry.edge_attached(),
            entry.ledger().len(),
            entry.bindings().entries().len(),
        )
    });
    let (entry_installed, edge_attached, verification_records, peer_bindings) = match observed {
        Ok(values) => (true, values.0, values.1, values.2),
        // A process that never installed the entry answers exactly that: it is
        // not an error to ask, and nothing may be reported as present.
        Err(_) => (false, false, 0usize, 0usize),
    };
    Ok(json!({
        "ok": true,
        "entryInstalled": entry_installed,
        "deviceEdgeAttached": edge_attached,
        "verificationRecords": verification_records,
        "peerBindings": peer_bindings,
    }))
}

/// Admits the caller's authority artifact through the pinned SDK.
///
/// The ports belong to the device layer and are not attached here, so they
/// answer `Unavailable` for every operation; admitting the artifact reads none
/// of them. The SDK decides whether these bytes are the fixed Candidate this
/// build integrates, and a refusal writes nothing and sends nothing.
fn protocol_line(authority_file: &str) -> Result<Value> {
    ensure!(
        !authority_file.trim().is_empty(),
        "cross_device_authority_file_missing"
    );
    let authority = read_bounded_file(authority_file, MAX_AUTHORITY_ARTIFACT_BYTES)?;
    let admitted = cross_device_entry::admit_protocol_work(
        authority,
        cross_device_entry::UnattachedCustody,
        cross_device_entry::UnattachedHandles,
        cross_device_entry::UnattachedState,
        cross_device_entry::SystemClock,
        cross_device_entry::UnattachedTransport,
    )
    .map_err(|refusal| anyhow!("{}: {}", refusal.code(), refusal.cause()))?;
    let line = admitted.line();
    Ok(json!({
        "ok": true,
        "authorityAdmitted": true,
        "protocolLineId": encode_hex(line.protocol_line_id()),
        "protectionProfileId": encode_hex(line.protection_profile_id()),
    }))
}

/// Hands one inbound protected packet to the entry's device edge.
///
/// The packet is opaque to this route: only the SDK entry behind the edge
/// decides what it is, and the entry refuses
/// (`cross_device_endpoint_unavailable`) when no edge is attached.
fn record(params: &Value) -> Result<Value> {
    let packet_file = required_text(params, "packetFile")?;
    let packet = read_bounded_file(&packet_file, MAX_PROTECTED_PACKET_BYTES)?;
    let body = cross_device_entry::decode_body(params)?;
    let forwarder = cross_device_entry::decode_forwarder(params)?;
    let station = cross_device_entry::decode_station_hint(params)?;
    let store = open_conversation_store()?;
    let receipt = cross_device_entry::with_entry(|entry| {
        entry.record_packet(&store, &packet, forwarder, body, station)
    })
    .map_err(refusal_error)??;
    Ok(json!({
        "ok": true,
        "logicalId": encode_hex(&receipt.logical_id()),
        "verificationOrdinal": receipt.verification().ordinal(),
        "admission": admission_json(receipt.admission()),
        "effectIntents": receipt.effect_intents(),
        "stationReported": receipt
            .station()
            .map(|hint| json!({
                "accepted": hint.station_reported_accepted(),
                "duplicate": hint.station_reported_duplicate(),
                "endpointEvidence": hint.is_endpoint_evidence(),
            })),
    }))
}

/// Binds one verified device to a membership the host already owns.
///
/// The conversation and the membership must exist locally and the membership
/// must still be active; this route creates neither.
fn bind(params: &Value) -> Result<Value> {
    let conversation_id = required_text(params, "conversationId")?;
    let membership_id = required_text(params, "membershipId")?;
    let provider_id = required_text(params, "providerId")?;
    let author = required_digest(params, "authorDigest")?;
    let device = required_digest(params, "deviceDigest")?;
    let store = open_conversation_store()?;
    let conversation = store
        .get(&conversation_id)
        .map_err(|_| anyhow!("cross_device_conversation_missing"))?;
    let membership = conversation
        .memberships
        .iter()
        .find(|membership| membership.id == membership_id)
        .ok_or_else(|| anyhow!("cross_device_membership_missing"))?;
    ensure!(
        membership.status == licoup_conversation::MembershipStatus::Active,
        "cross_device_membership_inactive"
    );
    cross_device_entry::with_entry(|entry| {
        entry.bindings().bind(
            author,
            device,
            PeerBinding {
                conversation_id: conversation_id.clone(),
                membership_id: membership_id.clone(),
                provider_id: provider_id.clone(),
            },
        );
    })
    .map_err(refusal_error)?;
    Ok(json!({
        "ok": true,
        "bound": true,
        "conversationId": conversation_id,
        "membershipId": membership_id,
    }))
}

/// Removes one binding, which is how revocation is expressed.
fn revoke(params: &Value) -> Result<Value> {
    let author = required_digest(params, "authorDigest")?;
    let device = required_digest(params, "deviceDigest")?;
    let removed = cross_device_entry::with_entry(|entry| entry.bindings().revoke(&author, &device))
        .map_err(refusal_error)?;
    Ok(json!({ "ok": true, "revoked": removed }))
}

fn open_conversation_store() -> Result<ConversationStore> {
    let root = licoup_foundation::platform::paths::portable_data_dir()
        .map_err(|_| anyhow!("cross_device_data_root_unavailable"))?;
    ConversationStore::open(&root)
        .map_err(|_| anyhow!("cross_device_conversation_store_unavailable"))
}

fn refusal_error(refusal: cross_device_entry::CrossDeviceRefusal) -> anyhow::Error {
    anyhow!("{}", refusal.code())
}

fn admission_json(admission: &AdmissionFact) -> Value {
    match admission {
        AdmissionFact::Admitted { event_id, sequence } => {
            json!({ "kind": "admitted", "eventId": event_id, "sequence": sequence })
        }
        AdmissionFact::Duplicate { event_id, sequence } => {
            json!({ "kind": "duplicate", "eventId": event_id, "sequence": sequence })
        }
    }
}

fn required_text(params: &Value, name: &str) -> Result<String> {
    let value = params
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("cross_device_field_missing:{name}"))?;
    Ok(value.to_owned())
}

fn required_digest(params: &Value, name: &str) -> Result<[u8; 32]> {
    let raw = required_text(params, name)?;
    decode_hex_32(&raw).ok_or_else(|| anyhow!("cross_device_digest_invalid:{name}"))
}

/// A bounded, regular-file read of one caller-named path.
fn read_bounded_file(path: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path).map_err(|_| anyhow!("cross_device_input_unreadable"))?;
    ensure!(metadata.is_file(), "cross_device_input_not_a_file");
    ensure!(metadata.len() <= max_bytes, "cross_device_input_too_large");
    std::fs::read(path).map_err(|_| anyhow!("cross_device_input_unreadable"))
}

fn encode_hex(bytes: &[u8]) -> String {
    use core::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_hex_32(raw: &str) -> Option<[u8; 32]> {
    if raw.len() != 64 {
        return None;
    }
    let mut decoded = [0u8; 32];
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
