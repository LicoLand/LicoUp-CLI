use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::SigningKey;
use time::OffsetDateTime;

use crate::core::secure_mesh_directory::{AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose};
use crate::core::secure_mesh_pqxdh::{
    ML_KEM_1024_PUBLIC_KEY_BYTES, validate_ml_kem_1024_public_key,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

use super::validation::{
    append_len_prefixed_bytes, bail_prekey, ensure_active_trust_state, ensure_not_expired,
    ensure_signature_shape, hex_sha256, parse_rfc3339, sign_payload, verify_signature,
};

pub const SECURE_MESH_PREKEY_PROTOCOL_VERSION: &str =
    "licomesh.secure-mesh.pairwise-pqxdh-mlkem1024.v1";

const PREKEY_MAGIC: &[u8] = b"LCOSM-PREKEY-PQXDH-MLKEM1024-v1";
const MAX_PREKEY_ID_LEN: usize = 128;
const MAX_PUBLIC_KEY_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPreKeyRecord {
    pub prekey_id: String,
    pub public_key: Vec<u8>,
    pub signature: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureMeshPreKeyKind {
    SignedPreKey,
    OneTimePreKey,
    OneTimeMlKem1024PreKey,
}

impl SecureMeshPreKeyKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SignedPreKey => "signed_prekey",
            Self::OneTimePreKey => "one_time_prekey",
            Self::OneTimeMlKem1024PreKey => "one_time_mlkem1024_prekey",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwisePreKeyBundle {
    pub endpoint_identity: DeviceTrustPublicIdentity,
    pub trust_state: DeviceTrustState,
    pub signed_prekey: SecureMeshPreKeyRecord,
    pub one_time_prekey: Option<SecureMeshPreKeyRecord>,
    pub one_time_mlkem1024_prekey: SecureMeshPreKeyRecord,
    pub prekey_publication_version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPreKeyValidationPolicy {
    pub require_verified_device: bool,
    pub require_one_time_prekey: bool,
}

impl Default for SecureMeshPreKeyValidationPolicy {
    fn default() -> Self {
        Self {
            require_verified_device: true,
            require_one_time_prekey: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPreKeyBundleValidation {
    pub endpoint_id: String,
    pub signed_prekey_id: String,
    pub one_time_prekey_id: Option<String>,
    pub one_time_mlkem1024_prekey_id: String,
    pub directory_authorization_digest: String,
}

pub fn sign_prekey_record(
    signer_key: &SigningKey,
    endpoint_identity: &DeviceTrustPublicIdentity,
    kind: SecureMeshPreKeyKind,
    prekey_id: impl Into<String>,
    public_key: impl Into<Vec<u8>>,
    created_at: impl Into<String>,
    expires_at: impl Into<String>,
) -> Result<SecureMeshPreKeyRecord> {
    let mut record = SecureMeshPreKeyRecord {
        prekey_id: prekey_id.into(),
        public_key: public_key.into(),
        signature: String::new(),
        created_at: created_at.into(),
        expires_at: expires_at.into(),
    };
    validate_prekey_shape(&record, kind, false)?;
    let payload = prekey_signature_payload(endpoint_identity, kind, &record)?;
    record.signature = sign_payload(signer_key, &payload);
    Ok(record)
}

pub fn verify_prekey_record(
    endpoint_identity: &DeviceTrustPublicIdentity,
    kind: SecureMeshPreKeyKind,
    record: &SecureMeshPreKeyRecord,
    now: OffsetDateTime,
) -> Result<()> {
    validate_prekey_shape(record, kind, true)?;
    ensure_not_expired(&record.created_at, &record.expires_at, now, "prekey")?;
    verify_signature(
        endpoint_identity,
        &prekey_signature_payload(endpoint_identity, kind, record)?,
        &record.signature,
        "prekey",
    )
}

pub(super) fn validate_pairwise_prekey_bundle_crypto(
    bundle: &SecureMeshPairwisePreKeyBundle,
    policy: &SecureMeshPreKeyValidationPolicy,
    now: OffsetDateTime,
) -> Result<SecureMeshPreKeyBundleValidation> {
    ensure_active_trust_state(bundle.trust_state.clone(), policy.require_verified_device)?;
    verify_prekey_record(
        &bundle.endpoint_identity,
        SecureMeshPreKeyKind::SignedPreKey,
        &bundle.signed_prekey,
        now,
    )?;
    let one_time_prekey_id = match &bundle.one_time_prekey {
        Some(record) => {
            verify_prekey_record(
                &bundle.endpoint_identity,
                SecureMeshPreKeyKind::OneTimePreKey,
                record,
                now,
            )?;
            Some(record.prekey_id.clone())
        }
        None if policy.require_one_time_prekey => {
            bail_prekey("secure mesh one-time prekey is required for a new pairwise session")?
        }
        None => None,
    };
    verify_prekey_record(
        &bundle.endpoint_identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        &bundle.one_time_mlkem1024_prekey,
        now,
    )?;
    validate_ml_kem_1024_public_key(&bundle.one_time_mlkem1024_prekey.public_key)?;
    Ok(SecureMeshPreKeyBundleValidation {
        endpoint_id: bundle.endpoint_identity.endpoint_id.clone(),
        signed_prekey_id: bundle.signed_prekey.prekey_id.clone(),
        one_time_prekey_id,
        one_time_mlkem1024_prekey_id: bundle.one_time_mlkem1024_prekey.prekey_id.clone(),
        directory_authorization_digest: String::new(),
    })
}

pub fn validate_pairwise_prekey_bundle(
    bundle: &SecureMeshPairwisePreKeyBundle,
    directory_authorization: &AuthorizedDirectoryLeaf,
    policy: &SecureMeshPreKeyValidationPolicy,
    now: OffsetDateTime,
) -> Result<SecureMeshPreKeyBundleValidation> {
    ensure!(
        directory_authorization.purpose()
            == DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
        "secure mesh prekey session bootstrap requires its exact directory authorization purpose"
    );
    directory_authorization.require_device_identity(&bundle.endpoint_identity)?;
    directory_authorization.require_signed_prekey_digest(
        &signed_prekey_bundle_digest(bundle)?,
        bundle.prekey_publication_version,
    )?;
    directory_authorization.require_one_time_prekey_batch_digest(
        &one_time_prekey_batch_digest(bundle)?,
        bundle.prekey_publication_version,
    )?;
    let mut validation = validate_pairwise_prekey_bundle_crypto(bundle, policy, now)?;
    validation.directory_authorization_digest = directory_authorization
        .transcript_binding_digest()
        .to_string();
    Ok(validation)
}

pub fn signed_prekey_bundle_digest(bundle: &SecureMeshPairwisePreKeyBundle) -> Result<String> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"LCOSM-DIRECTORY-PQXDH-SIGNED-PREKEY-v1");
    append_len_prefixed_bytes(
        &mut transcript,
        bundle.endpoint_identity.endpoint_id.as_bytes(),
    )?;
    append_len_prefixed_bytes(
        &mut transcript,
        bundle.endpoint_identity.fingerprint()?.as_bytes(),
    )?;
    append_prekey_record_digest_material(&mut transcript, &bundle.signed_prekey)?;
    transcript.extend_from_slice(&bundle.prekey_publication_version.to_be_bytes());
    Ok(hex_sha256(&transcript))
}

pub fn one_time_prekey_batch_digest(bundle: &SecureMeshPairwisePreKeyBundle) -> Result<String> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"LCOSM-DIRECTORY-PQXDH-ONE-TIME-PREKEY-BATCH-v1");
    append_len_prefixed_bytes(
        &mut transcript,
        bundle.endpoint_identity.endpoint_id.as_bytes(),
    )?;
    transcript.extend_from_slice(&bundle.prekey_publication_version.to_be_bytes());
    match &bundle.one_time_prekey {
        Some(record) => {
            transcript.push(1);
            append_prekey_record_digest_material(&mut transcript, record)?;
        }
        None => transcript.push(0),
    }
    transcript.push(1);
    append_prekey_record_digest_material(&mut transcript, &bundle.one_time_mlkem1024_prekey)?;
    Ok(hex_sha256(&transcript))
}

pub fn prekey_public_key_from_base64url(value: &str) -> Result<Vec<u8>> {
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .context("secure mesh prekey public key is not base64url")?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_PUBLIC_KEY_BYTES,
        "secure mesh prekey public key size is outside bounds"
    );
    Ok(bytes)
}

fn append_prekey_record_digest_material(
    out: &mut Vec<u8>,
    record: &SecureMeshPreKeyRecord,
) -> Result<()> {
    append_len_prefixed_bytes(out, record.prekey_id.as_bytes())?;
    append_len_prefixed_bytes(out, &record.public_key)?;
    append_len_prefixed_bytes(out, record.signature.as_bytes())?;
    append_len_prefixed_bytes(out, record.created_at.as_bytes())?;
    append_len_prefixed_bytes(out, record.expires_at.as_bytes())
}

fn validate_prekey_shape(
    record: &SecureMeshPreKeyRecord,
    kind: SecureMeshPreKeyKind,
    require_signature: bool,
) -> Result<()> {
    ensure!(
        !record.prekey_id.trim().is_empty(),
        "secure mesh {} id is required",
        kind.as_str()
    );
    ensure!(
        record.prekey_id.len() <= MAX_PREKEY_ID_LEN,
        "secure mesh {} id is too large",
        kind.as_str()
    );
    if kind == SecureMeshPreKeyKind::OneTimeMlKem1024PreKey {
        ensure!(
            record.public_key.len() == ML_KEM_1024_PUBLIC_KEY_BYTES,
            "secure mesh {} public key size is invalid",
            kind.as_str()
        );
        validate_ml_kem_1024_public_key(&record.public_key)?;
    } else {
        ensure!(
            !record.public_key.is_empty() && record.public_key.len() <= MAX_PUBLIC_KEY_BYTES,
            "secure mesh {} public key size is outside bounds",
            kind.as_str()
        );
    }
    if require_signature {
        ensure_signature_shape(&record.signature, kind.as_str())?;
    }
    parse_rfc3339(&record.created_at, kind.as_str())?;
    parse_rfc3339(&record.expires_at, kind.as_str())?;
    Ok(())
}

pub(super) fn prekey_signature_payload(
    endpoint_identity: &DeviceTrustPublicIdentity,
    kind: SecureMeshPreKeyKind,
    record: &SecureMeshPreKeyRecord,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(PREKEY_MAGIC);
    append_len_prefixed_bytes(&mut out, SECURE_MESH_PREKEY_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(&mut out, endpoint_identity.endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, endpoint_identity.fingerprint()?.as_bytes())?;
    append_len_prefixed_bytes(&mut out, kind.as_str().as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.prekey_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.created_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, record.expires_at.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &record.public_key)?;
    Ok(out)
}
