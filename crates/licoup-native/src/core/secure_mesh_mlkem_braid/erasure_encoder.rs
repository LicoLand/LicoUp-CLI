use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};

use super::{
    constants::{MAX_SOURCE_CHUNKS, ML_KEM_BRAID_CHUNK_BYTES},
    erasure_gf::{batch_inverse, combine_codewords, gf_mul},
    wire::MlKemBraidChunk,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ErasureEncoder {
    source: Vec<Vec<u8>>,
    next_point: u32,
    #[serde(skip)]
    denominator_inverses: Vec<u16>,
}

impl ErasureEncoder {
    pub(super) fn new(message: &[u8]) -> Result<Self> {
        ensure!(
            !message.is_empty() && message.len().is_multiple_of(ML_KEM_BRAID_CHUNK_BYTES),
            "ML-KEM Braid erasure message size is invalid"
        );
        let source = message
            .chunks_exact(ML_KEM_BRAID_CHUNK_BYTES)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        ensure!(
            source.len() <= MAX_SOURCE_CHUNKS,
            "ML-KEM Braid erasure message exceeds the resource limit"
        );
        let mut encoder = Self {
            source,
            next_point: 0,
            denominator_inverses: Vec::new(),
        };
        encoder.rebuild_cache()?;
        Ok(encoder)
    }

    pub(super) fn next_chunk(&mut self) -> Result<MlKemBraidChunk> {
        ensure!(
            self.next_point <= u16::MAX as u32,
            "ML-KEM Braid erasure domain is exhausted"
        );
        let point = self.next_point as u16;
        let bytes = self.evaluate(point)?;
        self.next_point = self
            .next_point
            .checked_add(1)
            .ok_or_else(|| anyhow!("ML-KEM Braid erasure point overflow"))?;
        Ok(MlKemBraidChunk::new(point, bytes))
    }

    pub(super) fn message_bytes(&self) -> usize {
        self.source.len() * ML_KEM_BRAID_CHUNK_BYTES
    }

    pub(super) fn evaluate(&self, point: u16) -> Result<[u8; ML_KEM_BRAID_CHUNK_BYTES]> {
        let source_count = self.source.len();
        if usize::from(point) < source_count {
            return self.source[usize::from(point)]
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("ML-KEM Braid source chunk length is invalid"));
        }
        // The source count is capped at 48, so parity emission uses bounded
        // stack storage instead of allocating on every send transition.
        let mut coefficients = [0u16; MAX_SOURCE_CHUNKS];
        for source_point in 0..source_count {
            let mut numerator = 1u16;
            for other in 0..source_count {
                if other != source_point {
                    numerator = gf_mul(numerator, point ^ other as u16);
                }
            }
            coefficients[source_point] = gf_mul(numerator, self.denominator_inverses[source_point]);
        }
        Ok(combine_codewords(
            &self.source,
            &coefficients[..source_count],
        ))
    }

    pub(super) fn rebuild_cache(&mut self) -> Result<()> {
        ensure!(
            !self.source.is_empty() && self.source.len() <= MAX_SOURCE_CHUNKS,
            "persisted ML-KEM Braid encoder source count is invalid"
        );
        ensure!(
            self.source
                .iter()
                .all(|chunk| chunk.len() == ML_KEM_BRAID_CHUNK_BYTES),
            "persisted ML-KEM Braid encoder chunk length is invalid"
        );
        ensure!(
            self.next_point <= u16::MAX as u32 + 1,
            "persisted ML-KEM Braid encoder point is invalid"
        );
        let mut denominators = Vec::with_capacity(self.source.len());
        for point in 0..self.source.len() {
            let mut denominator = 1u16;
            for other in 0..self.source.len() {
                if other != point {
                    denominator = gf_mul(denominator, point as u16 ^ other as u16);
                }
            }
            denominators.push(denominator);
        }
        self.denominator_inverses = batch_inverse(&denominators)?;
        Ok(())
    }
}
