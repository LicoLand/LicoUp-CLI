//! Post-quantum key material and the PQXDH-to-Triple-Ratchet key schedule.
//!
//! The relay never receives any secret from this module. ML-KEM private keys are
//! represented by their 64-byte generation seed so the platform secret store can
//! retain the minimum secret material and reconstruct the FIPS 203 key pair only
//! for decapsulation.

use anyhow::{Result, anyhow, ensure};
use hkdf::Hkdf;
use libcrux_ml_kem::mlkem1024::{MlKem1024, MlKem1024PublicKey};
use libcrux_traits::kem::arrayref::Kem;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

pub const SECURE_MESH_PQXDH_CIPHER_SUITE: &str =
    "licomesh.pqxdh-triple-ratchet.v1.x25519-ed25519-mlkem1024-hkdfsha256";
pub const ML_KEM_1024_KEY_GENERATION_SEED_BYTES: usize = 64;
pub const ML_KEM_1024_PUBLIC_KEY_BYTES: usize = 1_568;
pub const ML_KEM_1024_PRIVATE_KEY_BYTES: usize = 3_168;
pub const ML_KEM_1024_CIPHERTEXT_BYTES: usize = 1_568;
pub const ML_KEM_1024_SHARED_SECRET_BYTES: usize = 32;

const PQXDH_CURVE25519_ENCODING_TAG: u8 = 0x05;
const PQXDH_ML_KEM_1024_ENCODING_TAG: u8 = 0x22;
const PQXDH_F_PREFIX_BYTES: usize = 32;
const PQXDH_INFO: &[u8] = b"LicoMeshSecureMesh_CURVE25519_SHA-256_ML-KEM-1024";
const TRIPLE_RATCHET_INFO: &[u8] =
    b"licomesh.secure-mesh.triple-ratchet.initial-secrets.pqxdh-mlkem1024.v1";
type MlKem1024Algorithm = MlKem1024;

fn generate_ml_kem_1024_key_pair(
    seed: &[u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES],
) -> Result<(
    [u8; ML_KEM_1024_PUBLIC_KEY_BYTES],
    Zeroizing<[u8; ML_KEM_1024_PRIVATE_KEY_BYTES]>,
)> {
    let mut public_key = [0u8; ML_KEM_1024_PUBLIC_KEY_BYTES];
    let mut private_key = Zeroizing::new([0u8; ML_KEM_1024_PRIVATE_KEY_BYTES]);
    <MlKem1024Algorithm as Kem<
        ML_KEM_1024_PUBLIC_KEY_BYTES,
        ML_KEM_1024_PRIVATE_KEY_BYTES,
        ML_KEM_1024_CIPHERTEXT_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
        ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
    >>::keygen(&mut public_key, &mut private_key, seed)
    .map_err(|_| anyhow!("secure mesh ML-KEM-1024 key generation failed"))?;
    Ok((public_key, private_key))
}

/// The compact secret representation of an ML-KEM-1024 prekey.
///
/// It deliberately has no `Debug`, `Display`, or serialization implementation.
pub struct SecureMeshMlKem1024PreKeySeed(Zeroizing<[u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES]>);

impl SecureMeshMlKem1024PreKeySeed {
    pub fn generate() -> Self {
        let mut seed = [0u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES];
        OsRng.fill_bytes(&mut seed);
        Self(Zeroizing::new(seed))
    }

    pub fn from_bytes(seed: [u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES]) -> Self {
        Self(Zeroizing::new(seed))
    }

    pub fn public_key(&self) -> Vec<u8> {
        generate_ml_kem_1024_key_pair(&self.0)
            .expect("ML-KEM-1024 key generation accepts every 64-byte seed")
            .0
            .to_vec()
    }

    pub(crate) fn expose_for_secret_store(&self) -> [u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES] {
        *self.0
    }
}

impl Drop for SecureMeshMlKem1024PreKeySeed {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// ML-KEM ciphertext plus the shared secret held only by the initiator.
pub struct SecureMeshMlKem1024Encapsulation {
    pub ciphertext: Vec<u8>,
    shared_secret: Zeroizing<[u8; ML_KEM_1024_SHARED_SECRET_BYTES]>,
}

impl SecureMeshMlKem1024Encapsulation {
    pub fn shared_secret(&self) -> &[u8; ML_KEM_1024_SHARED_SECRET_BYTES] {
        &self.shared_secret
    }
}

/// The two independent 32-byte inputs required by the Triple Ratchet.
pub struct SecureMeshTripleRatchetInitialSecrets {
    ec_secret: Zeroizing<[u8; 32]>,
    scka_secret: Zeroizing<[u8; 32]>,
    associated_data: Vec<u8>,
}

impl SecureMeshTripleRatchetInitialSecrets {
    pub fn ec_secret(&self) -> &[u8; 32] {
        &self.ec_secret
    }

    pub fn scka_secret(&self) -> &[u8; 32] {
        &self.scka_secret
    }

    pub fn associated_data(&self) -> &[u8] {
        &self.associated_data
    }
}

pub fn validate_ml_kem_1024_public_key(public_key: &[u8]) -> Result<()> {
    ensure!(
        public_key.len() == ML_KEM_1024_PUBLIC_KEY_BYTES,
        "secure mesh ML-KEM-1024 public key length is invalid"
    );
    let public_key = MlKem1024PublicKey::try_from(public_key)
        .map_err(|_| anyhow!("secure mesh ML-KEM-1024 public key length is invalid"))?;
    ensure!(
        libcrux_ml_kem::mlkem1024::validate_public_key(&public_key),
        "secure mesh ML-KEM-1024 public key validation failed"
    );
    Ok(())
}

pub fn encapsulate_ml_kem_1024(public_key: &[u8]) -> Result<SecureMeshMlKem1024Encapsulation> {
    let mut randomness = Zeroizing::new([0u8; ML_KEM_1024_SHARED_SECRET_BYTES]);
    OsRng.fill_bytes(randomness.as_mut());
    encapsulate_ml_kem_1024_with_randomness(public_key, &randomness)
}

fn encapsulate_ml_kem_1024_with_randomness(
    public_key: &[u8],
    randomness: &[u8; ML_KEM_1024_SHARED_SECRET_BYTES],
) -> Result<SecureMeshMlKem1024Encapsulation> {
    validate_ml_kem_1024_public_key(public_key)?;
    let public_key: &[u8; ML_KEM_1024_PUBLIC_KEY_BYTES] = public_key
        .try_into()
        .map_err(|_| anyhow!("secure mesh ML-KEM-1024 public key length is invalid"))?;
    let mut ciphertext = [0u8; ML_KEM_1024_CIPHERTEXT_BYTES];
    let mut shared_secret = Zeroizing::new([0u8; ML_KEM_1024_SHARED_SECRET_BYTES]);
    <MlKem1024Algorithm as Kem<
        ML_KEM_1024_PUBLIC_KEY_BYTES,
        ML_KEM_1024_PRIVATE_KEY_BYTES,
        ML_KEM_1024_CIPHERTEXT_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
        ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
    >>::encaps(&mut ciphertext, &mut shared_secret, public_key, &randomness)
    .map_err(|_| anyhow!("secure mesh ML-KEM-1024 encapsulation failed"))?;
    Ok(SecureMeshMlKem1024Encapsulation {
        ciphertext: ciphertext.to_vec(),
        shared_secret,
    })
}

pub fn decapsulate_ml_kem_1024(
    prekey_seed: &SecureMeshMlKem1024PreKeySeed,
    expected_public_key: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<[u8; ML_KEM_1024_SHARED_SECRET_BYTES]>> {
    validate_ml_kem_1024_public_key(expected_public_key)?;
    ensure!(
        ciphertext.len() == ML_KEM_1024_CIPHERTEXT_BYTES,
        "secure mesh ML-KEM-1024 ciphertext length is invalid"
    );
    let (derived_public_key, private_key) = generate_ml_kem_1024_key_pair(&prekey_seed.0)?;
    ensure!(
        bool::from(derived_public_key.as_slice().ct_eq(expected_public_key)),
        "secure mesh ML-KEM-1024 prekey seed does not match the signed public key"
    );
    let ciphertext: &[u8; ML_KEM_1024_CIPHERTEXT_BYTES] = ciphertext
        .try_into()
        .map_err(|_| anyhow!("secure mesh ML-KEM-1024 ciphertext length is invalid"))?;
    let mut shared_secret = Zeroizing::new([0u8; ML_KEM_1024_SHARED_SECRET_BYTES]);
    <MlKem1024Algorithm as Kem<
        ML_KEM_1024_PUBLIC_KEY_BYTES,
        ML_KEM_1024_PRIVATE_KEY_BYTES,
        ML_KEM_1024_CIPHERTEXT_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
        ML_KEM_1024_KEY_GENERATION_SEED_BYTES,
        ML_KEM_1024_SHARED_SECRET_BYTES,
    >>::decaps(&mut shared_secret, ciphertext, &private_key)
    .map_err(|_| anyhow!("secure mesh ML-KEM-1024 decapsulation failed"))?;
    Ok(shared_secret)
}

/// Applies the PQXDH KDF, binds both device identities into AD, and expands the
/// resulting 32-byte session key into the two Triple Ratchet roots.
pub fn derive_triple_ratchet_initial_secrets(
    classical_key_material: &[u8],
    ml_kem_shared_secret: &[u8; ML_KEM_1024_SHARED_SECRET_BYTES],
    initiator_identity_public_key: &[u8; 32],
    responder_identity_public_key: &[u8; 32],
    transcript_binding: &[u8],
) -> Result<SecureMeshTripleRatchetInitialSecrets> {
    ensure!(
        !classical_key_material.is_empty() && !transcript_binding.is_empty(),
        "secure mesh PQXDH transcript material is incomplete"
    );

    let mut pqxdh_ikm = Zeroizing::new(Vec::with_capacity(
        PQXDH_F_PREFIX_BYTES + classical_key_material.len() + ml_kem_shared_secret.len(),
    ));
    pqxdh_ikm.extend_from_slice(&[0xff; PQXDH_F_PREFIX_BYTES]);
    pqxdh_ikm.extend_from_slice(classical_key_material);
    pqxdh_ikm.extend_from_slice(ml_kem_shared_secret);
    let mut pqxdh_info = Vec::with_capacity(PQXDH_INFO.len() + transcript_binding.len() + 8);
    pqxdh_info.extend_from_slice(PQXDH_INFO);
    append_len_prefixed(&mut pqxdh_info, transcript_binding)?;
    let mut session_key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&[0u8; 32]), pqxdh_ikm.as_slice())
        .expand(&pqxdh_info, session_key.as_mut())
        .map_err(|_| anyhow!("secure mesh PQXDH key derivation failed"))?;

    let associated_data = pqxdh_associated_data(
        initiator_identity_public_key,
        responder_identity_public_key,
        transcript_binding,
    )?;
    let mut triple_info = Vec::with_capacity(TRIPLE_RATCHET_INFO.len() + associated_data.len() + 8);
    triple_info.extend_from_slice(TRIPLE_RATCHET_INFO);
    append_len_prefixed(&mut triple_info, &associated_data)?;
    let mut expanded = Zeroizing::new([0u8; 64]);
    Hkdf::<Sha256>::new(Some(&[0u8; 32]), session_key.as_ref())
        .expand(&triple_info, expanded.as_mut())
        .map_err(|_| anyhow!("secure mesh Triple Ratchet key expansion failed"))?;
    let mut ec_secret = [0u8; 32];
    let mut scka_secret = [0u8; 32];
    ec_secret.copy_from_slice(&expanded[..32]);
    scka_secret.copy_from_slice(&expanded[32..]);
    ensure!(
        !bool::from(ec_secret.ct_eq(&scka_secret)),
        "secure mesh Triple Ratchet initial secrets are not independent"
    );
    Ok(SecureMeshTripleRatchetInitialSecrets {
        ec_secret: Zeroizing::new(ec_secret),
        scka_secret: Zeroizing::new(scka_secret),
        associated_data,
    })
}

fn pqxdh_associated_data(
    initiator_identity_public_key: &[u8; 32],
    responder_identity_public_key: &[u8; 32],
    transcript_binding: &[u8],
) -> Result<Vec<u8>> {
    let mut associated_data = Vec::with_capacity(2 + 64 + transcript_binding.len() + 8);
    associated_data.push(PQXDH_CURVE25519_ENCODING_TAG);
    associated_data.extend_from_slice(initiator_identity_public_key);
    associated_data.push(PQXDH_CURVE25519_ENCODING_TAG);
    associated_data.extend_from_slice(responder_identity_public_key);
    associated_data.push(PQXDH_ML_KEM_1024_ENCODING_TAG);
    append_len_prefixed(&mut associated_data, transcript_binding)?;
    Ok(associated_data)
}

fn append_len_prefixed(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh PQXDH transcript field is too large"))?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ml_kem_1024_roundtrip_and_implicit_rejection_are_active() {
        let prekey = SecureMeshMlKem1024PreKeySeed::from_bytes([7u8; 64]);
        let public_key = prekey.public_key();
        validate_ml_kem_1024_public_key(&public_key).unwrap();
        let encapsulation = encapsulate_ml_kem_1024(&public_key).unwrap();
        let opened =
            decapsulate_ml_kem_1024(&prekey, &public_key, &encapsulation.ciphertext).unwrap();
        assert_eq!(opened.as_ref(), encapsulation.shared_secret());

        let mut tampered = encapsulation.ciphertext.clone();
        tampered[0] ^= 1;
        let rejected_secret = decapsulate_ml_kem_1024(&prekey, &public_key, &tampered).unwrap();
        assert_ne!(rejected_secret.as_ref(), encapsulation.shared_secret());
    }

    #[test]
    fn ml_kem_1024_interface_matches_the_standard_fips_203_api() {
        let seed = [0x41u8; ML_KEM_1024_KEY_GENERATION_SEED_BYTES];
        let randomness = [0x52u8; ML_KEM_1024_SHARED_SECRET_BYTES];
        let prekey = SecureMeshMlKem1024PreKeySeed::from_bytes(seed);
        let public_key = prekey.public_key();
        let reference_key_pair = libcrux_ml_kem::mlkem1024::generate_key_pair(seed);
        assert_eq!(public_key.as_slice(), reference_key_pair.pk());

        let encapsulation =
            encapsulate_ml_kem_1024_with_randomness(&public_key, &randomness).unwrap();
        let (reference_ciphertext, reference_secret) =
            libcrux_ml_kem::mlkem1024::encapsulate(reference_key_pair.public_key(), randomness);
        assert_eq!(encapsulation.ciphertext, reference_ciphertext.as_slice());
        assert_eq!(encapsulation.shared_secret(), reference_secret.as_slice());

        let opened =
            decapsulate_ml_kem_1024(&prekey, &public_key, &encapsulation.ciphertext).unwrap();
        assert_eq!(opened.as_ref(), reference_secret.as_slice());
    }

    #[test]
    fn decapsulation_rejects_seed_to_signed_public_key_substitution() {
        let expected = SecureMeshMlKem1024PreKeySeed::from_bytes([8u8; 64]);
        let substituted = SecureMeshMlKem1024PreKeySeed::from_bytes([9u8; 64]);
        let public_key = expected.public_key();
        let encapsulation = encapsulate_ml_kem_1024(&public_key).unwrap();
        assert!(
            decapsulate_ml_kem_1024(&substituted, &public_key, &encapsulation.ciphertext,).is_err()
        );
    }

    #[test]
    fn pqxdh_schedule_is_deterministic_domain_separated_and_context_bound() {
        let first = derive_triple_ratchet_initial_secrets(
            b"classical-secret",
            &[3u8; 32],
            &[4u8; 32],
            &[5u8; 32],
            b"session-a",
        )
        .unwrap();
        assert_eq!(
            hex(first.ec_secret()),
            "0f98b1fe4d8782861382ad612b118a1e98dde620e3ad9886f6885aeffba8d7a3"
        );
        assert_eq!(
            hex(first.scka_secret()),
            "0e894284562bb8f8c3ffd786baf5cccdc9df8877969f668092ff1bbd151a55fe"
        );
        assert_eq!(
            hex(&libcrux_sha3::sha256(first.associated_data())),
            "3ce3c8ce777e15387ca031faa044134f8b48f46660e61f969058928d5b124cd7"
        );
        let second = derive_triple_ratchet_initial_secrets(
            b"classical-secret",
            &[3u8; 32],
            &[4u8; 32],
            &[5u8; 32],
            b"session-a",
        )
        .unwrap();
        let rebound = derive_triple_ratchet_initial_secrets(
            b"classical-secret",
            &[3u8; 32],
            &[4u8; 32],
            &[5u8; 32],
            b"session-b",
        )
        .unwrap();
        assert_eq!(first.ec_secret(), second.ec_secret());
        assert_eq!(first.scka_secret(), second.scka_secret());
        assert_ne!(first.ec_secret(), first.scka_secret());
        assert_ne!(first.ec_secret(), rebound.ec_secret());
        assert_ne!(first.scka_secret(), rebound.scka_secret());
        assert_ne!(first.associated_data(), rebound.associated_data());
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
