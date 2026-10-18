use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};

use super::{
    constants::{MAX_SOURCE_CHUNKS, ML_KEM_BRAID_CHUNK_BYTES},
    erasure_gf::{batch_inverse, batch_inverse_into, combine_codewords, gf_mul},
    wire::MlKemBraidChunk,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ErasureDecoder {
    message_bytes: usize,
    #[serde(with = "u16_chunk_map")]
    chunks: BTreeMap<u16, Vec<u8>>,
    decoded: Option<Vec<u8>>,
}

mod u16_chunk_map {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    pub fn serialize<S>(chunks: &BTreeMap<u16, Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        chunks
            .iter()
            .map(|(point, bytes)| (*point, bytes))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<u16, Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = Vec::<(u16, Vec<u8>)>::deserialize(deserializer)?;
        let mut chunks = BTreeMap::new();
        for (point, bytes) in entries {
            if chunks.insert(point, bytes).is_some() {
                return Err(de::Error::custom(
                    "ML-KEM Braid persisted decoder point is duplicated",
                ));
            }
        }
        Ok(chunks)
    }
}

impl ErasureDecoder {
    pub(super) fn new(message_bytes: usize) -> Result<Self> {
        ensure!(
            message_bytes != 0 && message_bytes.is_multiple_of(ML_KEM_BRAID_CHUNK_BYTES),
            "ML-KEM Braid decoder message size is invalid"
        );
        ensure!(
            message_bytes / ML_KEM_BRAID_CHUNK_BYTES <= MAX_SOURCE_CHUNKS,
            "ML-KEM Braid decoder exceeds the resource limit"
        );
        Ok(Self {
            message_bytes,
            chunks: BTreeMap::new(),
            decoded: None,
        })
    }

    pub(super) fn add_chunk(&mut self, chunk: &MlKemBraidChunk) -> Result<()> {
        ensure!(
            self.decoded.is_none(),
            "ML-KEM Braid decoder received data after completion"
        );
        ensure!(
            !self.chunks.contains_key(&chunk.point),
            "ML-KEM Braid duplicate or conflicting erasure chunk"
        );
        ensure!(
            self.chunks.len() < self.source_count(),
            "ML-KEM Braid decoder resource limit exceeded"
        );
        self.chunks.insert(chunk.point, chunk.bytes.to_vec());
        if self.chunks.len() == self.source_count() {
            self.decoded = Some(self.decode()?);
        }
        Ok(())
    }

    pub(super) fn has_message(&self) -> bool {
        self.decoded.is_some()
    }

    pub(super) fn take_message(&mut self) -> Result<Vec<u8>> {
        self.decoded
            .take()
            .ok_or_else(|| anyhow!("ML-KEM Braid message is incomplete"))
    }

    pub(super) fn source_count(&self) -> usize {
        self.message_bytes / ML_KEM_BRAID_CHUNK_BYTES
    }

    pub(super) fn validate_active(&self, expected_bytes: usize) -> Result<()> {
        self.validate()?;
        ensure!(
            self.message_bytes == expected_bytes
                && self.decoded.is_none()
                && self.chunks.len() < self.source_count(),
            "persisted ML-KEM Braid active decoder state is invalid"
        );
        Ok(())
    }

    pub(super) fn decode(&self) -> Result<Vec<u8>> {
        let count = self.source_count();
        if (0..count).all(|point| self.chunks.contains_key(&(point as u16))) {
            let mut message = Vec::with_capacity(self.message_bytes);
            for point in 0..count {
                message.extend_from_slice(
                    self.chunks
                        .get(&(point as u16))
                        .ok_or_else(|| anyhow!("ML-KEM Braid systematic chunk is missing"))?,
                );
            }
            return Ok(message);
        }

        let points = self.chunks.keys().copied().collect::<Vec<_>>();
        let codewords = self.chunks.values().collect::<Vec<_>>();
        let mut denominators = Vec::with_capacity(count);
        for (index, point) in points.iter().enumerate() {
            let mut denominator = 1u16;
            for (other_index, other) in points.iter().enumerate() {
                if index != other_index {
                    denominator = gf_mul(denominator, point ^ other);
                }
            }
            denominators.push(denominator);
        }
        let weights = batch_inverse(&denominators)?;
        let mut message = Vec::with_capacity(self.message_bytes);
        let mut differences = Vec::with_capacity(count);
        let mut inverse_prefixes = Vec::with_capacity(count + 1);
        let mut inverse_differences = Vec::with_capacity(count);
        let mut coefficients = Vec::with_capacity(count);
        // Coefficients are computed once per target and reused for all 16
        // symbols. Scratch allocations are reused, and state is capped at 48
        // chunks, keeping interpolation bounded and deterministic.
        for target in 0..count {
            if let Some(existing) = self.chunks.get(&(target as u16)) {
                message.extend_from_slice(existing);
                continue;
            }
            differences.clear();
            differences.extend(points.iter().map(|point| target as u16 ^ point));
            batch_inverse_into(
                &differences,
                &mut inverse_prefixes,
                &mut inverse_differences,
            )?;
            let all_differences = differences.iter().copied().fold(1u16, gf_mul);
            coefficients.clear();
            coefficients.extend(
                weights
                    .iter()
                    .zip(&inverse_differences)
                    .map(|(weight, inverse)| gf_mul(gf_mul(*weight, all_differences), *inverse)),
            );
            message.extend_from_slice(&combine_codewords(&codewords, &coefficients));
        }
        Ok(message)
    }

    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.message_bytes != 0
                && self.message_bytes.is_multiple_of(ML_KEM_BRAID_CHUNK_BYTES)
                && self.source_count() <= MAX_SOURCE_CHUNKS,
            "persisted ML-KEM Braid decoder size is invalid"
        );
        ensure!(
            self.chunks.len() <= self.source_count()
                && self
                    .chunks
                    .values()
                    .all(|chunk| chunk.len() == ML_KEM_BRAID_CHUNK_BYTES),
            "persisted ML-KEM Braid decoder chunks are invalid"
        );
        if let Some(decoded) = &self.decoded {
            ensure!(
                decoded.len() == self.message_bytes && self.chunks.len() == self.source_count(),
                "persisted ML-KEM Braid decoded message is invalid"
            );
        }
        Ok(())
    }
}
