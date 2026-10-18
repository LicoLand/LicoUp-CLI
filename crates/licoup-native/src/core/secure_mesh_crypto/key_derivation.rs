use anyhow::{Result, anyhow, ensure};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    constants::{
        CONTENT_KEY_LEN, HKDF_INFO_DOMAIN, HKDF_SALT_DOMAIN, PRIVATE_CONTEXT_AEAD_AAD,
        PRIVATE_CONTEXT_HKDF_INFO_DOMAIN, PRIVATE_CONTEXT_HKDF_SALT_DOMAIN,
        SECURE_MESH_CONTENT_CIPHER_SUITE,
    },
    content_key::ContentKey,
    length_codec::append_len_prefixed_bytes,
    model::{SecureMeshContentContext, SecureMeshPayloadKind},
};

pub(super) fn derive_aead_key(
    key: &ContentKey,
    context: &SecureMeshContentContext,
    kind: SecureMeshPayloadKind,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        key.as_slice().len() == CONTENT_KEY_LEN,
        "secure mesh content key length is invalid"
    );
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(HKDF_SALT_DOMAIN);
    salt_hasher.update(aad);
    let salt = salt_hasher.finalize();
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), key.as_slice());
    let mut info = Vec::new();
    info.extend_from_slice(HKDF_INFO_DOMAIN);
    append_len_prefixed_bytes(&mut info, context.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut info, kind.as_str().as_bytes())?;
    append_len_prefixed_bytes(&mut info, SECURE_MESH_CONTENT_CIPHER_SUITE.as_bytes())?;
    let mut okm = Zeroizing::new(vec![0u8; CONTENT_KEY_LEN]);
    hkdf.expand(&info, okm.as_mut_slice())
        .map_err(|_| anyhow!("secure mesh content key derivation failed"))?;
    Ok(okm)
}

pub(super) fn derive_private_context_aead_key(key: &ContentKey) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(
        key.as_slice().len() == CONTENT_KEY_LEN,
        "secure mesh private-context content key length is invalid"
    );
    let mut salt_hasher = Sha256::new();
    salt_hasher.update(PRIVATE_CONTEXT_HKDF_SALT_DOMAIN);
    salt_hasher.update(PRIVATE_CONTEXT_AEAD_AAD);
    let salt = salt_hasher.finalize();
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), key.as_slice());
    let mut info = Vec::new();
    info.extend_from_slice(PRIVATE_CONTEXT_HKDF_INFO_DOMAIN);
    append_len_prefixed_bytes(&mut info, PRIVATE_CONTEXT_AEAD_AAD)?;
    let mut okm = Zeroizing::new(vec![0u8; CONTENT_KEY_LEN]);
    hkdf.expand(&info, okm.as_mut_slice())
        .map_err(|_| anyhow!("secure mesh private-context key derivation failed"))?;
    Ok(okm)
}
