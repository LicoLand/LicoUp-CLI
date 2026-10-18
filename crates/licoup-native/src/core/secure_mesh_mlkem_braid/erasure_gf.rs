use anyhow::{Result, ensure};

use super::constants::{GF_REDUCTION_POLYNOMIAL, ML_KEM_BRAID_CHUNK_BYTES};

pub(super) fn combine_codewords<T>(
    codewords: &[T],
    coefficients: &[u16],
) -> [u8; ML_KEM_BRAID_CHUNK_BYTES]
where
    T: AsRef<[u8]>,
{
    let mut output = [0u8; ML_KEM_BRAID_CHUNK_BYTES];
    for symbol in 0..(ML_KEM_BRAID_CHUNK_BYTES / 2) {
        let mut value = 0u16;
        for (codeword, coefficient) in codewords.iter().zip(coefficients) {
            let codeword = codeword.as_ref();
            let source = u16::from_be_bytes([codeword[symbol * 2], codeword[symbol * 2 + 1]]);
            value ^= gf_mul(source, *coefficient);
        }
        output[symbol * 2..symbol * 2 + 2].copy_from_slice(&value.to_be_bytes());
    }
    output
}

pub(super) fn gf_mul(left: u16, right: u16) -> u16 {
    let mut left = u32::from(left);
    let mut right = right;
    let mut product = 0u32;
    for _ in 0..16 {
        if right & 1 != 0 {
            product ^= left;
        }
        right >>= 1;
        let carry = left & 0x8000;
        left = (left << 1) & 0xffff;
        if carry != 0 {
            left ^= GF_REDUCTION_POLYNOMIAL;
        }
    }
    product as u16
}

pub(super) fn gf_inverse(value: u16) -> Result<u16> {
    ensure!(value != 0, "ML-KEM Braid field inverse of zero");
    let mut exponent = 65_534u32;
    let mut base = value;
    let mut result = 1u16;
    while exponent != 0 {
        if exponent & 1 != 0 {
            result = gf_mul(result, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    Ok(result)
}

pub(super) fn batch_inverse(values: &[u16]) -> Result<Vec<u16>> {
    let mut prefixes = Vec::with_capacity(values.len() + 1);
    let mut output = Vec::with_capacity(values.len());
    batch_inverse_into(values, &mut prefixes, &mut output)?;
    Ok(output)
}

/// Uses one field inversion for the whole slice, matching the bounded
/// Lagrange interpolation in the Braid specification.
pub(super) fn batch_inverse_into(
    values: &[u16],
    prefixes: &mut Vec<u16>,
    output: &mut Vec<u16>,
) -> Result<()> {
    prefixes.clear();
    output.clear();
    if values.is_empty() {
        return Ok(());
    }
    ensure!(
        values.iter().all(|value| *value != 0),
        "ML-KEM Braid field denominator is zero"
    );
    prefixes.push(1u16);
    for value in values {
        prefixes.push(gf_mul(*prefixes.last().unwrap_or(&1), *value));
    }
    let mut inverse_product = gf_inverse(*prefixes.last().unwrap_or(&1))?;
    output.resize(values.len(), 0);
    for index in (0..values.len()).rev() {
        output[index] = gf_mul(inverse_product, prefixes[index]);
        inverse_product = gf_mul(inverse_product, values[index]);
    }
    Ok(())
}
