use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::{Zeroize, Zeroizing};

use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_CIPHERTEXT_BYTES, ML_KEM_1024_PUBLIC_KEY_BYTES, ML_KEM_1024_SHARED_SECRET_BYTES,
    SecureMeshMlKem1024PreKeySeed, decapsulate_ml_kem_1024, encapsulate_ml_kem_1024,
    validate_ml_kem_1024_public_key,
};

pub(crate) const MLS_ML_KEM_1024_EPOCH_EXTENSION_TYPE_ID: u16 = 0xff11;
pub(crate) const MLS_ML_KEM_1024_EPOCH_SCHEMA_VERSION: u32 = 1;
const EPOCH_SECRET_BYTES: usize = 32;
const WRAP_NONCE_BYTES: usize = 12;
const WRAP_TAG_BYTES: usize = 16;
const MEMBER_ID_DOMAIN: &[u8] = b"LICO-SM-MLS-MLKEM1024-MEMBER-ID-v1";
const WRAP_AAD_DOMAIN: &[u8] = b"LICO-SM-MLS-MLKEM1024-EPOCH-WRAP-v1";
const WRAP_KDF_INFO: &[u8] = b"licomesh.secure-mesh.mls.mlkem1024-epoch-wrap.v1";
const PAYLOAD_KDF_INFO: &[u8] = b"licomesh.secure-mesh.mls.mlkem1024-payload-key.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SecureMeshMlsMlKem1024RecipientWrap {
    pub public_key_base64url: String,
    pub kem_ciphertext_base64url: String,
    pub nonce_base64url: String,
    pub wrapped_epoch_secret_base64url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct SecureMeshMlsMlKem1024EpochExtension {
    pub schema_version: u32,
    pub epoch: u64,
    pub previous_epoch_digest: Option<String>,
    pub recipients: BTreeMap<String, SecureMeshMlsMlKem1024RecipientWrap>,
}

pub(crate) fn mlkem1024_member_id(credential_identity: &[u8]) -> Result<String> {
    ensure!(
        !credential_identity.is_empty(),
        "secure mesh MLS ML-KEM-1024 credential identity is empty"
    );
    let mut hash = Sha256::new();
    hash.update(MEMBER_ID_DOMAIN);
    append_len_prefixed(&mut hash, credential_identity)?;
    Ok(general_purpose::URL_SAFE_NO_PAD.encode(hash.finalize()))
}

pub(crate) fn mlkem1024_epoch_extension_digest(
    extension: &SecureMeshMlsMlKem1024EpochExtension,
) -> Result<String> {
    let encoded = serde_json::to_vec(extension)
        .context("secure mesh MLS ML-KEM-1024 epoch extension encoding failed")?;
    let digest: [u8; 32] = Sha256::digest(encoded).into();
    Ok(crate::core::secure_mesh_capability_proof::encode_sha256_digest(&digest))
}

pub(crate) fn create_mlkem1024_epoch_extension(
    group_id: &[u8],
    epoch: u64,
    previous_epoch_digest: Option<String>,
    member_public_keys: &BTreeMap<String, Vec<u8>>,
) -> Result<(
    SecureMeshMlsMlKem1024EpochExtension,
    Zeroizing<[u8; EPOCH_SECRET_BYTES]>,
)> {
    ensure!(
        !group_id.is_empty() && epoch > 0 && !member_public_keys.is_empty(),
        "secure mesh MLS ML-KEM-1024 epoch inputs are incomplete"
    );
    let mut epoch_secret = Zeroizing::new([0u8; EPOCH_SECRET_BYTES]);
    OsRng.fill_bytes(epoch_secret.as_mut());
    let mut recipients = BTreeMap::new();
    for (member_id, public_key) in member_public_keys {
        ensure!(
            general_purpose::URL_SAFE_NO_PAD
                .decode(member_id)
                .is_ok_and(|value| value.len() == 32),
            "secure mesh MLS ML-KEM-1024 member id is invalid"
        );
        validate_ml_kem_1024_public_key(public_key)?;
        let encapsulation = encapsulate_ml_kem_1024(public_key)?;
        let aad = wrap_aad(
            group_id,
            epoch,
            member_id,
            public_key,
            &encapsulation.ciphertext,
        )?;
        let wrap_key = derive_wrap_key(encapsulation.shared_secret(), &aad)?;
        let mut nonce = [0u8; WRAP_NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let wrapped = ChaCha20Poly1305::new(Key::from_slice(wrap_key.as_ref()))
            .encrypt(
                Nonce::from_slice(&nonce),
                AeadPayload {
                    msg: epoch_secret.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 epoch wrapping failed"))?;
        ensure!(
            wrapped.len() == EPOCH_SECRET_BYTES + WRAP_TAG_BYTES,
            "secure mesh MLS ML-KEM-1024 wrapped epoch secret length is invalid"
        );
        recipients.insert(
            member_id.clone(),
            SecureMeshMlsMlKem1024RecipientWrap {
                public_key_base64url: general_purpose::URL_SAFE_NO_PAD.encode(public_key),
                kem_ciphertext_base64url: general_purpose::URL_SAFE_NO_PAD
                    .encode(&encapsulation.ciphertext),
                nonce_base64url: general_purpose::URL_SAFE_NO_PAD.encode(nonce),
                wrapped_epoch_secret_base64url: general_purpose::URL_SAFE_NO_PAD.encode(wrapped),
            },
        );
    }
    Ok((
        SecureMeshMlsMlKem1024EpochExtension {
            schema_version: MLS_ML_KEM_1024_EPOCH_SCHEMA_VERSION,
            epoch,
            previous_epoch_digest,
            recipients,
        },
        epoch_secret,
    ))
}

pub(crate) fn open_mlkem1024_epoch_extension(
    group_id: &[u8],
    expected_epoch: u64,
    expected_member_ids: &BTreeSet<String>,
    credential_identity: &[u8],
    seed: &SecureMeshMlKem1024PreKeySeed,
    extension: &SecureMeshMlsMlKem1024EpochExtension,
) -> Result<Zeroizing<[u8; EPOCH_SECRET_BYTES]>> {
    ensure!(
        extension.schema_version == MLS_ML_KEM_1024_EPOCH_SCHEMA_VERSION,
        "secure mesh MLS ML-KEM-1024 epoch schema is unsupported"
    );
    ensure!(
        extension.epoch == expected_epoch && expected_epoch > 0,
        "secure mesh MLS ML-KEM-1024 epoch binding is invalid"
    );
    ensure!(
        extension
            .recipients
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            == *expected_member_ids,
        "secure mesh MLS ML-KEM-1024 recipient roster differs from the MLS roster"
    );
    let member_id = mlkem1024_member_id(credential_identity)?;
    let recipient = extension
        .recipients
        .get(&member_id)
        .ok_or_else(|| anyhow!("secure mesh MLS ML-KEM-1024 local recipient wrap is missing"))?;
    let public_key = decode_exact(
        &recipient.public_key_base64url,
        ML_KEM_1024_PUBLIC_KEY_BYTES,
        "public key",
    )?;
    ensure!(
        public_key == seed.public_key(),
        "secure mesh MLS ML-KEM-1024 recipient public key differs from selected custody"
    );
    let ciphertext = decode_exact(
        &recipient.kem_ciphertext_base64url,
        ML_KEM_1024_CIPHERTEXT_BYTES,
        "ciphertext",
    )?;
    let nonce = decode_exact(&recipient.nonce_base64url, WRAP_NONCE_BYTES, "nonce")?;
    let wrapped = decode_exact(
        &recipient.wrapped_epoch_secret_base64url,
        EPOCH_SECRET_BYTES + WRAP_TAG_BYTES,
        "wrapped epoch secret",
    )?;
    let shared_secret = decapsulate_ml_kem_1024(seed, &public_key, &ciphertext)?;
    let aad = wrap_aad(
        group_id,
        expected_epoch,
        &member_id,
        &public_key,
        &ciphertext,
    )?;
    let wrap_key = derive_wrap_key(&shared_secret, &aad)?;
    let mut opened = ChaCha20Poly1305::new(Key::from_slice(wrap_key.as_ref()))
        .decrypt(
            Nonce::from_slice(&nonce),
            AeadPayload {
                msg: &wrapped,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 epoch unwrap failed"))?;
    ensure!(
        opened.len() == EPOCH_SECRET_BYTES,
        "secure mesh MLS ML-KEM-1024 epoch secret length is invalid"
    );
    let mut fixed = Zeroizing::new([0u8; EPOCH_SECRET_BYTES]);
    fixed.copy_from_slice(&opened);
    opened.zeroize();
    Ok(fixed)
}

pub(crate) fn mix_mlkem1024_payload_key(
    mls_exported_secret: &[u8],
    mlkem1024_epoch_secret: &[u8; EPOCH_SECRET_BYTES],
    export_context: &[u8],
) -> Result<Zeroizing<[u8; EPOCH_SECRET_BYTES]>> {
    ensure!(
        mls_exported_secret.len() == EPOCH_SECRET_BYTES && !export_context.is_empty(),
        "secure mesh MLS hybrid payload KDF input is invalid"
    );
    let mut ikm = Zeroizing::new(Vec::with_capacity(EPOCH_SECRET_BYTES * 2));
    ikm.extend_from_slice(mls_exported_secret);
    ikm.extend_from_slice(mlkem1024_epoch_secret);
    let salt: [u8; 32] = Sha256::digest(export_context).into();
    let mut output = Zeroizing::new([0u8; EPOCH_SECRET_BYTES]);
    Hkdf::<Sha256>::new(Some(&salt), ikm.as_slice())
        .expand(PAYLOAD_KDF_INFO, output.as_mut())
        .map_err(|_| anyhow!("secure mesh MLS hybrid payload KDF failed"))?;
    Ok(output)
}

fn derive_wrap_key(
    shared_secret: &[u8; ML_KEM_1024_SHARED_SECRET_BYTES],
    aad: &[u8],
) -> Result<Zeroizing<[u8; 32]>> {
    let salt: [u8; 32] = Sha256::digest(aad).into();
    let mut output = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared_secret)
        .expand(WRAP_KDF_INFO, output.as_mut())
        .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 wrap KDF failed"))?;
    Ok(output)
}

fn wrap_aad(
    group_id: &[u8],
    epoch: u64,
    member_id: &str,
    public_key: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(
        WRAP_AAD_DOMAIN.len()
            + group_id.len()
            + member_id.len()
            + public_key.len()
            + ciphertext.len()
            + 32,
    );
    out.extend_from_slice(WRAP_AAD_DOMAIN);
    append_vec_len_prefixed(&mut out, group_id)?;
    out.extend_from_slice(&epoch.to_be_bytes());
    append_vec_len_prefixed(&mut out, member_id.as_bytes())?;
    append_vec_len_prefixed(&mut out, public_key)?;
    append_vec_len_prefixed(&mut out, ciphertext)?;
    Ok(out)
}

fn decode_exact(value: &str, expected: usize, label: &str) -> Result<Vec<u8>> {
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .with_context(|| format!("secure mesh MLS ML-KEM-1024 {label} is not base64url"))?;
    ensure!(
        decoded.len() == expected,
        "secure mesh MLS ML-KEM-1024 {label} length is invalid"
    );
    Ok(decoded)
}

fn append_len_prefixed(hash: &mut Sha256, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 identity is too large"))?;
    hash.update(len.to_be_bytes());
    hash.update(value);
    Ok(())
}

fn append_vec_len_prefixed(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh MLS ML-KEM-1024 field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mlkem1024_epoch_wrap_round_trip_and_roster_binding() {
        let alice = SecureMeshMlKem1024PreKeySeed::from_bytes([0x41; 64]);
        let bob = SecureMeshMlKem1024PreKeySeed::from_bytes([0x42; 64]);
        let alice_id = mlkem1024_member_id(b"alice").unwrap();
        let bob_id = mlkem1024_member_id(b"bob").unwrap();
        let members = BTreeMap::from([
            (alice_id.clone(), alice.public_key()),
            (bob_id.clone(), bob.public_key()),
        ]);
        let (extension, created_secret) =
            create_mlkem1024_epoch_extension(b"group", 1, None, &members).unwrap();
        let roster = members.keys().cloned().collect();
        let opened =
            open_mlkem1024_epoch_extension(b"group", 1, &roster, b"bob", &bob, &extension).unwrap();
        assert_eq!(opened.as_ref(), created_secret.as_ref());
        let wrong_roster = BTreeSet::from([alice_id]);
        assert!(
            open_mlkem1024_epoch_extension(b"group", 1, &wrong_roster, b"bob", &bob, &extension,)
                .is_err()
        );
        assert!(extension.recipients.contains_key(&bob_id));
    }
}
