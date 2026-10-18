use super::codec::append_len_prefixed_bytes;
use super::identity::DeviceTrustPublicIdentity;
use super::{
    QR_MAGIC, SAFETY_NUMBER_CHUNK_COUNT, SAFETY_NUMBER_CHUNK_MODULUS,
    SAFETY_NUMBER_DIGITS_PER_CHUNK, SAS_MAGIC, SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
};
use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose};
use sha2::{Digest, Sha512};

pub fn sas_decimal_chunks(
    first: &DeviceTrustPublicIdentity,
    second: &DeviceTrustPublicIdentity,
) -> Result<[String; SAFETY_NUMBER_CHUNK_COUNT]> {
    let (left, right) = ordered_pair(first, second)?;
    let mut canonical = Vec::new();
    canonical.extend_from_slice(SAS_MAGIC);
    append_len_prefixed_bytes(&mut canonical, left.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut canonical, right.fingerprint()?.as_bytes())?;
    let digest = Sha512::digest(&canonical);
    Ok(std::array::from_fn(|index| {
        let offset = index * 4;
        let value = u32::from_be_bytes([
            digest[offset],
            digest[offset + 1],
            digest[offset + 2],
            digest[offset + 3],
        ]);
        format!(
            "{:0width$}",
            value % SAFETY_NUMBER_CHUNK_MODULUS,
            width = SAFETY_NUMBER_DIGITS_PER_CHUNK
        )
    }))
}

pub fn qr_verification_payload(
    first: &DeviceTrustPublicIdentity,
    second: &DeviceTrustPublicIdentity,
    roster_epoch: u64,
) -> Result<String> {
    let (left, right) = ordered_pair(first, second)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(QR_MAGIC);
    append_len_prefixed_bytes(
        &mut payload,
        SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION.as_bytes(),
    )?;
    payload.extend_from_slice(&roster_epoch.to_be_bytes());
    append_len_prefixed_bytes(&mut payload, left.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut payload, left.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut payload, right.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut payload, right.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(
        &mut payload,
        sas_decimal_chunks(left, right)?.join("-").as_bytes(),
    )?;
    Ok(format!(
        "{}:{}",
        SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION,
        general_purpose::URL_SAFE_NO_PAD.encode(payload)
    ))
}

fn ordered_pair<'a>(
    first: &'a DeviceTrustPublicIdentity,
    second: &'a DeviceTrustPublicIdentity,
) -> Result<(&'a DeviceTrustPublicIdentity, &'a DeviceTrustPublicIdentity)> {
    ensure!(
        first.endpoint_id != second.endpoint_id,
        "secure mesh device verification requires two distinct endpoints"
    );
    if first.endpoint_id <= second.endpoint_id {
        Ok((first, second))
    } else {
        Ok((second, first))
    }
}
