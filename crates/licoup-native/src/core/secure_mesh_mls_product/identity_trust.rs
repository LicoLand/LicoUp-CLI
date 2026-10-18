use super::constants::MLS_CREDENTIAL_MAGIC;
use super::helpers::{append_len_prefixed, identity_validate, read_len_prefixed};
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::types::SignatureScheme;
use std::collections::BTreeMap;

use crate::core::secure_mesh_mls::{SecureMeshMlsGroup, SecureMeshMlsParticipant};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

pub fn mls_credential_identity_bytes(identity: &DeviceTrustPublicIdentity) -> Result<Vec<u8>> {
    identity_validate(identity)?;
    let mut out = Vec::new();
    out.extend_from_slice(MLS_CREDENTIAL_MAGIC);
    append_len_prefixed(&mut out, identity.endpoint_id.as_bytes())?;
    out.extend_from_slice(&identity.rotation_epoch.to_be_bytes());
    append_len_prefixed(&mut out, &identity.identity_public_key)?;
    Ok(out)
}

pub fn device_identity_from_mls_credential(
    credential: &[u8],
    signing_public_key: &[u8],
) -> Result<DeviceTrustPublicIdentity> {
    ensure!(
        credential.starts_with(MLS_CREDENTIAL_MAGIC),
        "secure mesh MLS credential magic mismatch"
    );
    let mut offset = MLS_CREDENTIAL_MAGIC.len();
    let endpoint = read_len_prefixed(credential, &mut offset)?;
    ensure!(
        credential.len() >= offset + 8,
        "secure mesh MLS credential is truncated"
    );
    let rotation_epoch = u64::from_be_bytes(
        credential[offset..offset + 8]
            .try_into()
            .map_err(|_| anyhow!("secure mesh MLS credential rotation epoch is invalid"))?,
    );
    offset += 8;
    let identity_public_key = read_len_prefixed(credential, &mut offset)?;
    let identity_public_key: [u8; 32] = identity_public_key
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("secure mesh MLS credential identity public key is invalid"))?;
    let signing_public_key: [u8; 32] = signing_public_key
        .try_into()
        .map_err(|_| anyhow!("secure mesh MLS member signing public key is invalid"))?;
    DeviceTrustPublicIdentity::new(
        String::from_utf8(endpoint)
            .map_err(|_| anyhow!("secure mesh MLS credential endpoint is not utf8"))?,
        identity_public_key,
        signing_public_key,
        rotation_epoch,
    )
}

pub fn directory_roster_from_group(
    group: &SecureMeshMlsGroup,
) -> Result<BTreeMap<String, DeviceTrustPublicIdentity>> {
    let mut roster = BTreeMap::new();
    for (credential, signing_public_key) in group.member_credential_signing_pairs()? {
        let identity = device_identity_from_mls_credential(&credential, &signing_public_key)?;
        ensure!(
            roster
                .insert(identity.endpoint_id.clone(), identity)
                .is_none(),
            "secure mesh MLS group roster contains a duplicate endpoint"
        );
    }
    Ok(roster)
}

pub fn participant_from_device_identity(
    identity: &DeviceTrustPublicIdentity,
    device_signing_key: &SigningKey,
) -> Result<SecureMeshMlsParticipant> {
    identity_validate(identity)?;
    ensure!(
        device_signing_key.verifying_key().to_bytes() == identity.signing_public_key,
        "secure mesh MLS device signing key does not match trust identity"
    );
    let credential_identity = mls_credential_identity_bytes(identity)?;
    let signer = SignatureKeyPair::from_raw(
        SignatureScheme::ED25519,
        device_signing_key.to_bytes().to_vec(),
        identity.signing_public_key.to_vec(),
    );
    SecureMeshMlsParticipant::from_credential_parts(credential_identity, signer)
}

pub fn require_verified_member_trust(trust_state: &DeviceTrustState) -> Result<()> {
    ensure!(
        matches!(
            trust_state,
            DeviceTrustState::Verified | DeviceTrustState::CrossSigned
        ),
        "secure mesh MLS member trust is not verified"
    );
    Ok(())
}
