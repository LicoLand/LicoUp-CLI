//! Four-operation BadTower station transport surface.

use std::collections::HashSet;

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use licoup_foundation::platform::url_security::canonical_https_or_loopback_http_origin;
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;

use super::contract::{
    BadTowerDeletionTransportHint, BadTowerDeliveryTransportHint, BadTowerLeaseTransportHint,
    BadTowerStationError, BadTowerStationErrorCategory, BadTowerStationOperation,
    MAX_LEASE_SECONDS, MAX_OPAQUE_ID_CHARS, MAX_RECEIVE_ENVELOPES, MAX_RECEIVE_RESPONSE_BYTES,
    MAX_REQUEST_BYTES, MAX_SMALL_RESPONSE_BYTES, MIN_OPAQUE_ID_CHARS, MIN_RECEIVE_ENVELOPES,
};
use super::http_io::{StationHttpClient, error};
use super::wire::{
    DeletionResponse, DeliveryResponse, LeaseRequest, LeaseResponse, LicoArcEnvelopeWire,
    ReceiveResponse,
};

pub(crate) struct BadTowerStationTransport {
    http: StationHttpClient,
}

impl BadTowerStationTransport {
    pub(crate) fn new(base_url: impl AsRef<str>) -> Result<Self, BadTowerStationError> {
        let operation = BadTowerStationOperation::ConfigureTransport;
        let base_url =
            canonical_https_or_loopback_http_origin(base_url.as_ref()).ok_or_else(|| {
                error(
                    operation,
                    BadTowerStationErrorCategory::InvalidEndpoint,
                    false,
                )
            })?;
        Ok(Self {
            http: StationHttpClient::new(base_url),
        })
    }

    pub(crate) fn lease_mailbox(
        &self,
        mailbox_id: &str,
        lease_seconds: u64,
    ) -> Result<BadTowerLeaseTransportHint, BadTowerStationError> {
        let operation = BadTowerStationOperation::LeaseMailbox;
        validate_opaque_id(operation, mailbox_id)?;
        if lease_seconds == 0 || lease_seconds > MAX_LEASE_SECONDS {
            return Err(error(
                operation,
                BadTowerStationErrorCategory::InvalidInput,
                false,
            ));
        }
        let body = encode_request(operation, &LeaseRequest { lease_seconds })?;
        let path = format!("/v1/mailboxes/{mailbox_id}/lease");
        let response: LeaseResponse =
            self.http
                .post_json(operation, &path, &body, 200, MAX_SMALL_RESPONSE_BYTES)?;
        if response.mailbox_id != mailbox_id
            || response.lease_expires_at.is_empty()
            || response.lease_expires_at.len() > 64
            || OffsetDateTime::parse(&response.lease_expires_at, &Rfc3339).is_err()
        {
            return Err(error(
                operation,
                BadTowerStationErrorCategory::ResponseProtocol,
                false,
            ));
        }
        Ok(BadTowerLeaseTransportHint::reported())
    }

    pub(crate) fn send_envelope(
        &self,
        envelope: &LicoArcRelayEnvelope,
    ) -> Result<BadTowerDeliveryTransportHint, BadTowerStationError> {
        let operation = BadTowerStationOperation::SendEnvelope;
        envelope
            .validate()
            .map_err(|_| error(operation, BadTowerStationErrorCategory::InvalidInput, false))?;
        validate_opaque_id(operation, envelope.mailbox_id())?;
        validate_opaque_id(operation, envelope.envelope_id())?;
        let body = envelope.to_json().map_err(|_| {
            error(
                operation,
                BadTowerStationErrorCategory::RequestEncoding,
                false,
            )
        })?;
        validate_request_size(operation, body.len())?;
        let response: DeliveryResponse = self.http.post_json(
            operation,
            "/v1/envelopes",
            &body,
            202,
            MAX_SMALL_RESPONSE_BYTES,
        )?;
        Ok(BadTowerDeliveryTransportHint::reported(
            response.accepted,
            response.duplicate,
        ))
    }

    pub(crate) fn receive_envelopes(
        &self,
        mailbox_id: &str,
        limit: u16,
    ) -> Result<Vec<LicoArcRelayEnvelope>, BadTowerStationError> {
        let operation = BadTowerStationOperation::ReceiveEnvelopes;
        validate_opaque_id(operation, mailbox_id)?;
        if !(MIN_RECEIVE_ENVELOPES..=MAX_RECEIVE_ENVELOPES).contains(&limit) {
            return Err(error(
                operation,
                BadTowerStationErrorCategory::InvalidInput,
                false,
            ));
        }
        let path = format!("/v1/mailboxes/{mailbox_id}/envelopes?limit={limit}");
        let response: ReceiveResponse =
            self.http
                .get_json(operation, &path, 200, MAX_RECEIVE_RESPONSE_BYTES)?;
        let wires = response.envelopes.into_inner();
        if wires.len() > usize::from(limit) {
            return Err(error(
                operation,
                BadTowerStationErrorCategory::ResponseProtocol,
                false,
            ));
        }

        let mut seen_envelope_ids = HashSet::with_capacity(wires.len());
        let mut envelopes = Vec::with_capacity(wires.len());
        for wire in wires {
            let envelope = decode_envelope(operation, wire)?;
            validate_opaque_id(operation, envelope.mailbox_id()).map_err(|_| {
                error(
                    operation,
                    BadTowerStationErrorCategory::ResponseProtocol,
                    false,
                )
            })?;
            validate_opaque_id(operation, envelope.envelope_id()).map_err(|_| {
                error(
                    operation,
                    BadTowerStationErrorCategory::ResponseProtocol,
                    false,
                )
            })?;
            if envelope.mailbox_id() != mailbox_id
                || !seen_envelope_ids.insert(envelope.envelope_id().to_string())
            {
                return Err(error(
                    operation,
                    BadTowerStationErrorCategory::ResponseProtocol,
                    false,
                ));
            }
            envelopes.push(envelope);
        }
        Ok(envelopes)
    }

    pub(crate) fn delete_envelope(
        &self,
        mailbox_id: &str,
        envelope_id: &str,
    ) -> Result<BadTowerDeletionTransportHint, BadTowerStationError> {
        let operation = BadTowerStationOperation::DeleteEnvelope;
        validate_opaque_id(operation, mailbox_id)?;
        validate_opaque_id(operation, envelope_id)?;
        let path = format!("/v1/mailboxes/{mailbox_id}/envelopes/{envelope_id}");
        let response: DeletionResponse =
            self.http
                .delete_json(operation, &path, 200, MAX_SMALL_RESPONSE_BYTES)?;
        Ok(BadTowerDeletionTransportHint::reported(
            response.acknowledged,
        ))
    }
}

fn encode_request<T>(
    operation: BadTowerStationOperation,
    request: &T,
) -> Result<String, BadTowerStationError>
where
    T: serde::Serialize,
{
    let encoded = serde_json::to_string(request).map_err(|_| {
        error(
            operation,
            BadTowerStationErrorCategory::RequestEncoding,
            false,
        )
    })?;
    validate_request_size(operation, encoded.len())?;
    Ok(encoded)
}

fn validate_request_size(
    operation: BadTowerStationOperation,
    size: usize,
) -> Result<(), BadTowerStationError> {
    if size == 0 || size > MAX_REQUEST_BYTES {
        return Err(error(
            operation,
            BadTowerStationErrorCategory::RequestTooLarge,
            false,
        ));
    }
    Ok(())
}

fn validate_opaque_id(
    operation: BadTowerStationOperation,
    value: &str,
) -> Result<(), BadTowerStationError> {
    if !(MIN_OPAQUE_ID_CHARS..=MAX_OPAQUE_ID_CHARS).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(error(
            operation,
            BadTowerStationErrorCategory::InvalidInput,
            false,
        ));
    }
    Ok(())
}

fn decode_envelope(
    operation: BadTowerStationOperation,
    wire: LicoArcEnvelopeWire,
) -> Result<LicoArcRelayEnvelope, BadTowerStationError> {
    let encoded = serde_json::to_string(&wire).map_err(|_| {
        error(
            operation,
            BadTowerStationErrorCategory::ResponseProtocol,
            false,
        )
    })?;
    LicoArcRelayEnvelope::from_json(&encoded).map_err(|_| {
        error(
            operation,
            BadTowerStationErrorCategory::ResponseProtocol,
            false,
        )
    })
}
