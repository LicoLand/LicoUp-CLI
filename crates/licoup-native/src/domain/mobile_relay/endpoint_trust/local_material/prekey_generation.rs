use crate::core::secure_mesh_pairwise::SecureMeshPairwisePrivateKey;
use crate::core::secure_mesh_pqxdh::{ML_KEM_1024_PUBLIC_KEY_BYTES, SecureMeshMlKem1024PreKeySeed};
use crate::core::secure_mesh_prekey::{SecureMeshPreKeyKind, sign_prekey_record};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use crate::domain::mobile_relay::endpoint_trust::{
    decode_fixed_base64url, decode_key_32, now_iso, random_base64url,
};
use crate::domain::mobile_relay::support::{
    MOBILE_RELAY_KEY_BYTES, MOBILE_RELAY_PREKEY_VALIDITY_DAYS,
};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose};
use ed25519_dalek::SigningKey;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub(super) struct CurvePreKeyMaterial {
    pub(super) id: String,
    pub(super) private_key: String,
    pub(super) public_key: String,
    pub(super) signature: String,
    pub(super) created_at: String,
    pub(super) expires_at: String,
}

pub(super) struct MlKemPreKeyMaterial {
    pub(super) id: String,
    pub(super) seed: String,
    pub(super) public_key: String,
    pub(super) signature: String,
    pub(super) created_at: String,
    pub(super) expires_at: String,
}

pub(super) struct CurvePreKeyRequest<'a> {
    pub(super) private_key: Option<&'a str>,
    pub(super) id: Option<&'a str>,
    pub(super) created_at: Option<&'a str>,
    pub(super) expires_at: Option<&'a str>,
    pub(super) signing_key: &'a SigningKey,
    pub(super) identity: &'a DeviceTrustPublicIdentity,
    pub(super) kind: SecureMeshPreKeyKind,
    pub(super) id_prefix: &'a str,
}

pub(super) fn curve_prekey_material(
    request: CurvePreKeyRequest<'_>,
) -> Result<CurvePreKeyMaterial> {
    let private_key = request
        .private_key
        .map(str::to_string)
        .unwrap_or_else(|| random_base64url(MOBILE_RELAY_KEY_BYTES));
    let private_bytes = decode_key_32(&private_key, "mobile relay prekey private key")?;
    let public_key = SecureMeshPairwisePrivateKey::from_bytes(private_bytes).public_key();
    let id = request
        .id
        .map(str::to_string)
        .unwrap_or_else(|| format!("mrelay_{}_{}", request.id_prefix, Uuid::new_v4()));
    let created_at = request
        .created_at
        .map(str::to_string)
        .unwrap_or_else(now_iso);
    let expires_at = request
        .expires_at
        .map(str::to_string)
        .unwrap_or_else(default_prekey_expiry);
    let record = sign_prekey_record(
        request.signing_key,
        request.identity,
        request.kind,
        id.clone(),
        public_key,
        created_at.clone(),
        expires_at.clone(),
    )?;
    Ok(CurvePreKeyMaterial {
        id,
        private_key,
        public_key: general_purpose::URL_SAFE_NO_PAD.encode(public_key),
        signature: record.signature,
        created_at,
        expires_at,
    })
}

pub(super) fn mlkem_prekey_material(
    seed: Option<&str>,
    id: Option<&str>,
    created_at: Option<&str>,
    expires_at: Option<&str>,
    signing_key: &SigningKey,
    identity: &DeviceTrustPublicIdentity,
) -> Result<MlKemPreKeyMaterial> {
    let (prekey_seed, seed) = match seed {
        Some(value) => (
            SecureMeshMlKem1024PreKeySeed::from_bytes(decode_fixed_base64url(
                value,
                "mobile relay ML-KEM-1024 one-time prekey seed",
            )?),
            value.to_string(),
        ),
        None => {
            let generated = SecureMeshMlKem1024PreKeySeed::generate();
            let encoded =
                general_purpose::URL_SAFE_NO_PAD.encode(generated.expose_for_secret_store());
            (generated, encoded)
        }
    };
    let public_key = prekey_seed.public_key();
    ensure!(
        public_key.len() == ML_KEM_1024_PUBLIC_KEY_BYTES,
        "mobile relay ML-KEM-1024 one-time prekey public key length is invalid"
    );
    let id = id
        .map(str::to_string)
        .unwrap_or_else(|| format!("mrelay_pqotpk_{}", Uuid::new_v4()));
    let created_at = created_at.map(str::to_string).unwrap_or_else(now_iso);
    let expires_at = expires_at
        .map(str::to_string)
        .unwrap_or_else(default_prekey_expiry);
    let record = sign_prekey_record(
        signing_key,
        identity,
        SecureMeshPreKeyKind::OneTimeMlKem1024PreKey,
        id.clone(),
        public_key.clone(),
        created_at.clone(),
        expires_at.clone(),
    )?;
    Ok(MlKemPreKeyMaterial {
        id,
        seed,
        public_key: general_purpose::URL_SAFE_NO_PAD.encode(public_key),
        signature: record.signature,
        created_at,
        expires_at,
    })
}

fn default_prekey_expiry() -> String {
    (OffsetDateTime::now_utc() + Duration::days(MOBILE_RELAY_PREKEY_VALIDITY_DAYS))
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-31T00:00:00Z".to_string())
}
