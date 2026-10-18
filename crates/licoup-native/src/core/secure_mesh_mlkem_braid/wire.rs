use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::constants::{ENCODED_CHUNK_BYTES, ML_KEM_BRAID_CHUNK_BYTES};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MlKemBraidMessageType {
    #[serde(rename = "None")]
    None,
    #[serde(rename = "Hdr")]
    Hdr,
    #[serde(rename = "Ek")]
    Ek,
    #[serde(rename = "EkCt1Ack")]
    EkCt1Ack,
    #[serde(rename = "Ct1Ack")]
    Ct1Ack,
    #[serde(rename = "Ct1")]
    Ct1,
    #[serde(rename = "Ct2")]
    Ct2,
}

impl MlKemBraidMessageType {
    pub(super) fn carries_data(self) -> bool {
        !matches!(self, Self::None | Self::Ct1Ack)
    }
}

/// One systematic GF(2^16) codeword, including its two-byte evaluation point.
#[derive(Clone, Eq, PartialEq)]
pub struct MlKemBraidChunk {
    pub(super) point: u16,
    pub(super) bytes: [u8; ML_KEM_BRAID_CHUNK_BYTES],
}

impl MlKemBraidChunk {
    pub fn point(&self) -> u16 {
        self.point
    }

    pub fn bytes(&self) -> &[u8; ML_KEM_BRAID_CHUNK_BYTES] {
        &self.bytes
    }

    pub(super) fn new(point: u16, bytes: [u8; ML_KEM_BRAID_CHUNK_BYTES]) -> Self {
        Self { point, bytes }
    }
}

impl Serialize for MlKemBraidChunk {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut encoded = [0u8; ML_KEM_BRAID_CHUNK_BYTES + 2];
        encoded[..2].copy_from_slice(&self.point.to_be_bytes());
        encoded[2..].copy_from_slice(&self.bytes);
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(encoded))
    }
}

impl<'de> Deserialize<'de> for MlKemBraidChunk {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() != ENCODED_CHUNK_BYTES {
            return Err(de::Error::custom("ML-KEM Braid data length is invalid"));
        }
        let mut decoded = [0u8; ML_KEM_BRAID_CHUNK_BYTES + 2];
        let decoded_len = URL_SAFE_NO_PAD
            .decode_slice(encoded.as_bytes(), &mut decoded)
            .map_err(de::Error::custom)?;
        if decoded_len != decoded.len() || URL_SAFE_NO_PAD.encode(decoded) != encoded {
            return Err(de::Error::custom(
                "ML-KEM Braid data encoding is non-canonical",
            ));
        }
        let point = u16::from_be_bytes([decoded[0], decoded[1]]);
        let mut bytes = [0u8; ML_KEM_BRAID_CHUNK_BYTES];
        bytes.copy_from_slice(&decoded[2..]);
        Ok(Self { point, bytes })
    }
}

/// Strict wire representation with exactly epoch, type, and optional data.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct MlKemBraidMessage {
    pub(super) epoch: u64,
    #[serde(rename = "type")]
    pub(super) message_type: MlKemBraidMessageType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) data: Option<MlKemBraidChunk>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMlKemBraidMessage {
    epoch: u64,
    #[serde(rename = "type")]
    message_type: MlKemBraidMessageType,
    #[serde(default)]
    data: Option<MlKemBraidChunk>,
}

impl<'de> Deserialize<'de> for MlKemBraidMessage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawMlKemBraidMessage::deserialize(deserializer)?;
        let message = Self {
            epoch: raw.epoch,
            message_type: raw.message_type,
            data: raw.data,
        };
        message.validate().map_err(de::Error::custom)?;
        Ok(message)
    }
}

impl MlKemBraidMessage {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn message_type(&self) -> MlKemBraidMessageType {
        self.message_type
    }

    pub fn data(&self) -> Option<&MlKemBraidChunk> {
        self.data.as_ref()
    }

    pub(super) fn payload(
        epoch: u64,
        message_type: MlKemBraidMessageType,
        data: MlKemBraidChunk,
    ) -> Self {
        Self {
            epoch,
            message_type,
            data: Some(data),
        }
    }

    pub(super) fn empty(epoch: u64, message_type: MlKemBraidMessageType) -> Self {
        Self {
            epoch,
            message_type,
            data: None,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(self.epoch != 0, "ML-KEM Braid epoch zero is invalid");
        ensure!(
            self.message_type.carries_data() == self.data.is_some(),
            "ML-KEM Braid type/data combination is invalid"
        );
        Ok(())
    }
}
