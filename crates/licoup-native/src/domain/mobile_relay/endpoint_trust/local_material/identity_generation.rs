use crate::domain::mobile_relay::endpoint_trust::{
    decode_key_32, public_key_fingerprint, random_base64url,
};
use crate::domain::mobile_relay::support::MOBILE_RELAY_KEY_BYTES;
use anyhow::Result;
use base64::{Engine, engine::general_purpose};
use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub(super) struct IdentityMaterial {
    pub(super) private_key: String,
    pub(super) public_key: String,
    pub(super) fingerprint: String,
}

pub(super) struct SigningMaterial {
    pub(super) key: SigningKey,
    pub(super) private_key: String,
    pub(super) public_key: String,
}

pub(super) fn generate_identity_material() -> IdentityMaterial {
    let mut private = [0u8; MOBILE_RELAY_KEY_BYTES];
    OsRng.fill_bytes(&mut private);
    let secret = StaticSecret::from(private);
    let public = PublicKey::from(&secret).to_bytes();
    IdentityMaterial {
        private_key: general_purpose::URL_SAFE_NO_PAD.encode(private),
        public_key: general_purpose::URL_SAFE_NO_PAD.encode(public),
        fingerprint: public_key_fingerprint(&public),
    }
}

pub(super) fn derive_identity_public(private_key: &str) -> Result<([u8; 32], String, String)> {
    let decoded = Zeroizing::new(decode_key_32(
        private_key,
        "mobile relay local private key",
    )?);
    let secret = StaticSecret::from(*decoded);
    let public = PublicKey::from(&secret).to_bytes();
    Ok((
        public,
        general_purpose::URL_SAFE_NO_PAD.encode(public),
        public_key_fingerprint(&public),
    ))
}

pub(super) fn signing_material(existing_private_key: Option<&str>) -> Result<SigningMaterial> {
    let key = match existing_private_key {
        Some(value) => SigningKey::from_bytes(&decode_key_32(value, "mobile relay signing key")?),
        None => SigningKey::generate(&mut OsRng),
    };
    Ok(SigningMaterial {
        private_key: general_purpose::URL_SAFE_NO_PAD.encode(key.to_bytes()),
        public_key: general_purpose::URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes()),
        key,
    })
}

pub(super) fn generate_endpoint_id(endpoint_kind: &str) -> String {
    format!(
        "{}_{}",
        if endpoint_kind == "mobile" {
            "mobile"
        } else {
            "pc"
        },
        Uuid::new_v4()
    )
}

pub(super) fn generate_session_id() -> String {
    format!("mrelay_session_{}", Uuid::new_v4())
}

pub(super) fn generate_pairing_secret() -> String {
    random_base64url(MOBILE_RELAY_KEY_BYTES)
}
