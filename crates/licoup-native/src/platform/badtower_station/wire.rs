//! Closed request and response wire schemas.

use std::fmt;

use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::contract::MAX_RECEIVE_ENVELOPES;

#[derive(Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LeaseRequest {
    pub(super) lease_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LeaseResponse {
    pub(super) mailbox_id: String,
    pub(super) lease_expires_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct DeliveryResponse {
    pub(super) accepted: bool,
    pub(super) duplicate: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct DeletionResponse {
    pub(super) acknowledged: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReceiveResponse {
    pub(super) envelopes: BoundedEnvelopeWires,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LicoArcEnvelopeWire {
    pub(super) contract_version: String,
    pub(super) envelope_id: String,
    pub(super) mailbox_id: String,
    pub(super) ciphertext: String,
    pub(super) expires_at: String,
}

pub(super) struct BoundedEnvelopeWires(Vec<LicoArcEnvelopeWire>);

impl BoundedEnvelopeWires {
    pub(super) fn into_inner(self) -> Vec<LicoArcEnvelopeWire> {
        self.0
    }
}

impl<'de> Deserialize<'de> for BoundedEnvelopeWires {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(BoundedEnvelopeVisitor)
    }
}

struct BoundedEnvelopeVisitor;

impl<'de> Visitor<'de> for BoundedEnvelopeVisitor {
    type Value = BoundedEnvelopeWires;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "at most {MAX_RECEIVE_ENVELOPES} Lico Arc envelopes"
        )
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if sequence
            .size_hint()
            .is_some_and(|size| size > usize::from(MAX_RECEIVE_ENVELOPES))
        {
            return Err(A::Error::custom("station envelope count exceeds bounds"));
        }
        let mut envelopes = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(usize::from(MAX_RECEIVE_ENVELOPES)),
        );
        while let Some(envelope) = sequence.next_element()? {
            if envelopes.len() == usize::from(MAX_RECEIVE_ENVELOPES) {
                return Err(A::Error::custom("station envelope count exceeds bounds"));
            }
            envelopes.push(envelope);
        }
        Ok(BoundedEnvelopeWires(envelopes))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ErrorResponse {
    pub(super) error: ErrorBody,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ErrorBody {
    pub(super) code: StationErrorCode,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StationErrorCode {
    InvalidRequest,
    LeaseRequired,
    TransportConflict,
    StationLimit,
    InternalError,
}
