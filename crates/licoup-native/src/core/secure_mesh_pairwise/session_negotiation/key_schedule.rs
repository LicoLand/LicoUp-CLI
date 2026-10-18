use anyhow::{Result, anyhow};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use super::super::key_ratchet::SecureMeshPairwisePrivateKey;
use super::super::support::{
    CAPABILITY_BOUND_KEY_SCHEDULE_MAGIC, CHAIN_KEY_LEN, HEADER_KEY_LEN,
    PQXDH_CLASSICAL_INFO_DOMAIN, PQXDH_CLASSICAL_SALT_DOMAIN, PUBLIC_KEY_LEN, ROOT_KEY_LEN,
    SECRET_DOMAIN, SECURE_MESH_PAIRWISE_CIPHER_SUITE, append_len_prefixed_bytes,
};
use super::transcript_codec::SecureMeshPairwiseSessionIntro;
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_prekey::SecureMeshPairwisePreKeyBundle;
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(in crate::core::secure_mesh_pairwise) struct InitialPairwiseKeys {
    pub(in crate::core::secure_mesh_pairwise) root_key: [u8; ROOT_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) initiator_chain_key: [u8; CHAIN_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) responder_chain_key: [u8; CHAIN_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) initiator_header_key: [u8; HEADER_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) responder_header_key: [u8; HEADER_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) initiator_next_header_key: [u8; HEADER_KEY_LEN],
    pub(in crate::core::secure_mesh_pairwise) responder_next_header_key: [u8; HEADER_KEY_LEN],
}

pub(in crate::core::secure_mesh_pairwise) fn derive_pqxdh_classical_initiator_secret(
    local_identity: &DeviceTrustPublicIdentity,
    local_identity_secret: &SecureMeshPairwisePrivateKey,
    local_ephemeral: &SecureMeshPairwisePrivateKey,
    remote_bundle: &SecureMeshPairwisePreKeyBundle,
) -> Result<Zeroizing<Vec<u8>>> {
    let dh1 = local_identity_secret.diffie_hellman(&remote_bundle.signed_prekey.public_key)?;
    let dh2 =
        local_ephemeral.diffie_hellman(&remote_bundle.endpoint_identity.identity_public_key)?;
    let dh3 = local_ephemeral.diffie_hellman(&remote_bundle.signed_prekey.public_key)?;
    let dh4 = remote_bundle
        .one_time_prekey
        .as_ref()
        .map(|record| local_ephemeral.diffie_hellman(&record.public_key))
        .transpose()?;
    collect_pqxdh_classical_secret(
        &local_identity.endpoint_id,
        &remote_bundle.endpoint_identity.endpoint_id,
        &dh1,
        &dh2,
        &dh3,
        dh4.as_ref().map(|value| &**value),
    )
}

pub(in crate::core::secure_mesh_pairwise) fn derive_pqxdh_classical_responder_secret(
    local_identity_secret: &SecureMeshPairwisePrivateKey,
    local_signed_prekey_secret: &SecureMeshPairwisePrivateKey,
    local_one_time_prekey_secret: Option<&SecureMeshPairwisePrivateKey>,
    intro: &SecureMeshPairwiseSessionIntro,
) -> Result<Zeroizing<Vec<u8>>> {
    let dh1 = local_signed_prekey_secret.diffie_hellman(&intro.initiator_identity_public_key)?;
    let dh2 = local_identity_secret.diffie_hellman(&intro.initiator_ephemeral_public_key)?;
    let dh3 = local_signed_prekey_secret.diffie_hellman(&intro.initiator_ephemeral_public_key)?;
    let dh4 = match (
        &intro.responder_one_time_prekey_id,
        local_one_time_prekey_secret,
    ) {
        (Some(_), Some(secret)) => {
            Some(secret.diffie_hellman(&intro.initiator_ephemeral_public_key)?)
        }
        (Some(_), None) => {
            return Err(anyhow!(
                "secure mesh pairwise one-time prekey secret is required"
            ));
        }
        (None, _) => None,
    };
    collect_pqxdh_classical_secret(
        &intro.initiator_endpoint_id,
        &intro.responder_endpoint_id,
        &dh1,
        &dh2,
        &dh3,
        dh4.as_ref().map(|value| &**value),
    )
}

pub(in crate::core::secure_mesh_pairwise) fn collect_pqxdh_classical_secret(
    initiator_endpoint_id: &str,
    responder_endpoint_id: &str,
    dh1: &[u8; PUBLIC_KEY_LEN],
    dh2: &[u8; PUBLIC_KEY_LEN],
    dh3: &[u8; PUBLIC_KEY_LEN],
    dh4: Option<&[u8; PUBLIC_KEY_LEN]>,
) -> Result<Zeroizing<Vec<u8>>> {
    let mut secret = Zeroizing::new(Vec::new());
    secret.extend_from_slice(SECRET_DOMAIN);
    append_len_prefixed_bytes(secret.as_mut(), initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(secret.as_mut(), responder_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(secret.as_mut(), dh1)?;
    append_len_prefixed_bytes(secret.as_mut(), dh2)?;
    append_len_prefixed_bytes(secret.as_mut(), dh3)?;
    if let Some(dh4) = dh4 {
        append_len_prefixed_bytes(secret.as_mut(), dh4)?;
    }
    Ok(secret)
}

pub(in crate::core::secure_mesh_pairwise) fn derive_initial_keys(
    shared_secret: &[u8],
    session_id: &str,
    initiator_endpoint_id: &str,
    responder_endpoint_id: &str,
) -> Result<InitialPairwiseKeys> {
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(PQXDH_CLASSICAL_SALT_DOMAIN);
    salt_hasher.update(session_id.as_bytes());
    let salt = salt_hasher.finalize();
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
    let mut info = Vec::new();
    info.extend_from_slice(PQXDH_CLASSICAL_INFO_DOMAIN);
    append_len_prefixed_bytes(&mut info, SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut info, initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, responder_endpoint_id.as_bytes())?;
    let mut out = [0u8; ROOT_KEY_LEN + (2 * CHAIN_KEY_LEN) + (4 * HEADER_KEY_LEN)];
    hkdf.expand(&info, &mut out)
        .map_err(|_| anyhow!("secure mesh pairwise initial key derivation failed"))?;
    let mut root_key = [0u8; ROOT_KEY_LEN];
    let mut initiator_chain_key = [0u8; CHAIN_KEY_LEN];
    let mut responder_chain_key = [0u8; CHAIN_KEY_LEN];
    let mut initiator_header_key = [0u8; HEADER_KEY_LEN];
    let mut responder_header_key = [0u8; HEADER_KEY_LEN];
    let mut initiator_next_header_key = [0u8; HEADER_KEY_LEN];
    let mut responder_next_header_key = [0u8; HEADER_KEY_LEN];
    root_key.copy_from_slice(&out[0..ROOT_KEY_LEN]);
    initiator_chain_key.copy_from_slice(&out[ROOT_KEY_LEN..ROOT_KEY_LEN + CHAIN_KEY_LEN]);
    let mut offset = ROOT_KEY_LEN + CHAIN_KEY_LEN;
    responder_chain_key.copy_from_slice(&out[offset..offset + CHAIN_KEY_LEN]);
    offset += CHAIN_KEY_LEN;
    initiator_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    responder_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    initiator_next_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    responder_next_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    out.zeroize();
    Ok(InitialPairwiseKeys {
        root_key,
        initiator_chain_key,
        responder_chain_key,
        initiator_header_key,
        responder_header_key,
        initiator_next_header_key,
        responder_next_header_key,
    })
}

pub(in crate::core::secure_mesh_pairwise) fn derive_capability_bound_initial_keys(
    initial_root_key: &[u8; ROOT_KEY_LEN],
    capability_transcript_digest: &str,
    session_id: &str,
    initiator_endpoint_id: &str,
    responder_endpoint_id: &str,
) -> Result<InitialPairwiseKeys> {
    let capability_digest = crate::core::secure_mesh_capability_proof::decode_sha256_digest(
        capability_transcript_digest,
        "capability-bound key schedule transcript digest",
    )?;
    let mut salt = Sha256::new();
    salt.update(CAPABILITY_BOUND_KEY_SCHEDULE_MAGIC);
    salt.update(capability_digest);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt.finalize()), initial_root_key);
    let mut info = Vec::new();
    append_len_prefixed_bytes(&mut info, SECURE_MESH_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut info, SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes())?;
    append_len_prefixed_bytes(&mut info, session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, initiator_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, responder_endpoint_id.as_bytes())?;
    let mut out = [0u8; ROOT_KEY_LEN + (2 * CHAIN_KEY_LEN) + (4 * HEADER_KEY_LEN)];
    hkdf.expand(&info, &mut out)
        .map_err(|_| anyhow!("secure mesh pairwise capability-bound key derivation failed"))?;
    let mut root_key = [0u8; ROOT_KEY_LEN];
    let mut initiator_chain_key = [0u8; CHAIN_KEY_LEN];
    let mut responder_chain_key = [0u8; CHAIN_KEY_LEN];
    let mut initiator_header_key = [0u8; HEADER_KEY_LEN];
    let mut responder_header_key = [0u8; HEADER_KEY_LEN];
    let mut initiator_next_header_key = [0u8; HEADER_KEY_LEN];
    let mut responder_next_header_key = [0u8; HEADER_KEY_LEN];
    let mut offset = 0;
    root_key.copy_from_slice(&out[offset..offset + ROOT_KEY_LEN]);
    offset += ROOT_KEY_LEN;
    initiator_chain_key.copy_from_slice(&out[offset..offset + CHAIN_KEY_LEN]);
    offset += CHAIN_KEY_LEN;
    responder_chain_key.copy_from_slice(&out[offset..offset + CHAIN_KEY_LEN]);
    offset += CHAIN_KEY_LEN;
    initiator_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    responder_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    initiator_next_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    offset += HEADER_KEY_LEN;
    responder_next_header_key.copy_from_slice(&out[offset..offset + HEADER_KEY_LEN]);
    out.zeroize();
    Ok(InitialPairwiseKeys {
        root_key,
        initiator_chain_key,
        responder_chain_key,
        initiator_header_key,
        responder_header_key,
        initiator_next_header_key,
        responder_next_header_key,
    })
}
