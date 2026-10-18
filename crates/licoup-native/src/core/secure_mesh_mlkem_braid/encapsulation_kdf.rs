use anyhow::{Result, anyhow, ensure};
use hkdf::Hkdf;
use libcrux_ml_kem::{
    KEY_GENERATION_SEED_SIZE,
    mlkem1024::incremental::{self, Ciphertext1, Ciphertext2},
};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use super::{
    authenticator::RatchetedAuthenticator,
    constants::{
        ML_KEM_BRAID_CT1_BYTES, ML_KEM_BRAID_CT2_BYTES, ML_KEM_BRAID_EK_BYTES,
        ML_KEM_BRAID_HEADER_BYTES, OUTPUT_KEY_LABEL, PROTOCOL_INFO,
    },
    erasure_encoder::ErasureEncoder,
    output::MlKemBraidOutputKey,
    secret::SecretBytes,
};

pub(super) fn derive_output_key(
    raw_shared_secret: &[u8],
    epoch: u64,
) -> Result<MlKemBraidOutputKey> {
    let mut info = Vec::with_capacity(PROTOCOL_INFO.len() + OUTPUT_KEY_LABEL.len() + 8);
    info.extend_from_slice(PROTOCOL_INFO);
    info.extend_from_slice(OUTPUT_KEY_LABEL);
    info.extend_from_slice(&epoch.to_be_bytes());
    let mut output = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&[0u8; 32]), raw_shared_secret)
        .expand(&info, output.as_mut())
        .map_err(|_| anyhow!("ML-KEM Braid output-key KDF failed"))?;
    Ok(MlKemBraidOutputKey { epoch, key: output })
}

pub(super) fn validate_encapsulation_key(header: &[u8], ek_vector: &[u8]) -> Result<()> {
    ensure!(
        header.len() == ML_KEM_BRAID_HEADER_BYTES && ek_vector.len() == ML_KEM_BRAID_EK_BYTES,
        "ML-KEM Braid encapsulation key length is invalid"
    );
    incremental::validate_pk_bytes(header, ek_vector)
        .map_err(|_| anyhow!("ML-KEM Braid encapsulation key integrity failed"))
}

// Keep the dependency's incremental order intact: validate PK1/PK2, retain
// Encaps1 state, then complete Encaps2 after the second key part is recovered.
pub(super) fn complete_encapsulation(
    auth: &RatchetedAuthenticator,
    epoch: u64,
    encaps_state: &SecretBytes,
    ct1: &[u8],
    ek_vector: &[u8],
) -> Result<ErasureEncoder> {
    encaps_state.ensure_len(incremental::encaps_state_len())?;
    ensure!(
        ct1.len() == ML_KEM_BRAID_CT1_BYTES && ek_vector.len() == ML_KEM_BRAID_EK_BYTES,
        "ML-KEM Braid encapsulation input length is invalid"
    );
    let state: &[u8; incremental::encaps_state_len()] = encaps_state
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("ML-KEM Braid encapsulation state is invalid"))?;
    let public_key: &[u8; incremental::pk2_len()] = ek_vector
        .try_into()
        .map_err(|_| anyhow!("ML-KEM Braid encapsulation key vector is invalid"))?;
    let ciphertext2 = incremental::encapsulate2(state, public_key);
    let mut authenticated = Vec::with_capacity(ML_KEM_BRAID_CT1_BYTES + ML_KEM_BRAID_CT2_BYTES);
    authenticated.extend_from_slice(ct1);
    authenticated.extend_from_slice(&ciphertext2.value);
    let mac = auth.mac_ciphertext(epoch, &authenticated)?;
    let mut encoded = ciphertext2.value.to_vec();
    encoded.extend_from_slice(&mac);
    ErasureEncoder::new(&encoded)
}

pub(super) fn decapsulate(
    key_seed: &SecretBytes,
    ct1: &[u8],
    ct2: &[u8],
) -> Result<Zeroizing<[u8; 32]>> {
    key_seed.ensure_len(KEY_GENERATION_SEED_SIZE)?;
    ensure!(
        ct1.len() == ML_KEM_BRAID_CT1_BYTES && ct2.len() == ML_KEM_BRAID_CT2_BYTES,
        "ML-KEM Braid ciphertext length is invalid"
    );
    let mut seed = Zeroizing::new([0u8; KEY_GENERATION_SEED_SIZE]);
    seed.copy_from_slice(key_seed.as_slice());
    let mut key_pair = Zeroizing::new([0u8; incremental::COMPRESSED_KEYPAIR_LEN]);
    incremental::generate_key_pair_compressed(*seed, &mut *key_pair);
    let ciphertext1 = Ciphertext1 {
        value: ct1
            .try_into()
            .map_err(|_| anyhow!("ML-KEM Braid ct1 is invalid"))?,
    };
    let ciphertext2 = Ciphertext2 {
        value: ct2
            .try_into()
            .map_err(|_| anyhow!("ML-KEM Braid ct2 is invalid"))?,
    };
    let mut shared_secret =
        incremental::decapsulate_compressed_key(&key_pair, &ciphertext1, &ciphertext2);
    let mut output = [0u8; 32];
    output.copy_from_slice(shared_secret.as_slice());
    shared_secret.zeroize();
    Ok(Zeroizing::new(output))
}
