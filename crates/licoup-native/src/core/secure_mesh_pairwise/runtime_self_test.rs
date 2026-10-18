use std::sync::OnceLock;

use anyhow::{Result, anyhow, ensure};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload as AeadPayload},
};

use super::key_ratchet::{SecureMeshPairwisePrivateKey, advance_chain};
use super::session_negotiation::{collect_pqxdh_classical_secret, derive_initial_keys};
use super::support::NONCE_LEN;
use crate::core::secure_mesh_pqxdh::{
    SecureMeshMlKem1024PreKeySeed, decapsulate_ml_kem_1024, derive_triple_ratchet_initial_secrets,
    encapsulate_ml_kem_1024,
};
use crate::core::secure_mesh_sparse_pq_ratchet::{
    SecureMeshSparsePqRatchet, derive_hybrid_message_key,
};

pub(super) static PAIRWISE_RUNTIME_CRYPTO_SELF_TEST: OnceLock<bool> = OnceLock::new();

/// Exercises the in-memory PQXDH and Triple Ratchet primitives used by the
/// mobile runtime without touching persisted client state.
pub fn runtime_crypto_self_test() -> bool {
    *PAIRWISE_RUNTIME_CRYPTO_SELF_TEST.get_or_init(|| {
        (|| -> Result<()> {
            let alice_identity = SecureMeshPairwisePrivateKey::generate();
            let alice_ephemeral = SecureMeshPairwisePrivateKey::generate();
            let bob_identity = SecureMeshPairwisePrivateKey::generate();
            let bob_signed_prekey = SecureMeshPairwisePrivateKey::generate();
            let bob_one_time_prekey = SecureMeshPairwisePrivateKey::generate();

            let initiator_dh1 = alice_identity.diffie_hellman(&bob_signed_prekey.public_key())?;
            let initiator_dh2 = alice_ephemeral.diffie_hellman(&bob_identity.public_key())?;
            let initiator_dh3 = alice_ephemeral.diffie_hellman(&bob_signed_prekey.public_key())?;
            let initiator_dh4 =
                alice_ephemeral.diffie_hellman(&bob_one_time_prekey.public_key())?;

            let initiator_classical_secret = collect_pqxdh_classical_secret(
                "runtime-self-test:initiator",
                "runtime-self-test:responder",
                &initiator_dh1,
                &initiator_dh2,
                &initiator_dh3,
                Some(&initiator_dh4),
            )?;

            let responder_dh1 = bob_signed_prekey.diffie_hellman(&alice_identity.public_key())?;
            let responder_dh2 = bob_identity.diffie_hellman(&alice_ephemeral.public_key())?;
            let responder_dh3 = bob_signed_prekey.diffie_hellman(&alice_ephemeral.public_key())?;
            let responder_dh4 =
                bob_one_time_prekey.diffie_hellman(&alice_ephemeral.public_key())?;
            let responder_classical_secret = collect_pqxdh_classical_secret(
                "runtime-self-test:initiator",
                "runtime-self-test:responder",
                &responder_dh1,
                &responder_dh2,
                &responder_dh3,
                Some(&responder_dh4),
            )?;
            ensure!(
                initiator_classical_secret.as_slice() == responder_classical_secret.as_slice(),
                "pairwise runtime PQXDH classical agreement failed"
            );

            let bob_mlkem1024_seed = SecureMeshMlKem1024PreKeySeed::generate();
            let bob_mlkem1024_public_key = bob_mlkem1024_seed.public_key();
            let initiator_mlkem1024 = encapsulate_ml_kem_1024(&bob_mlkem1024_public_key)?;
            let responder_mlkem1024 = decapsulate_ml_kem_1024(
                &bob_mlkem1024_seed,
                &bob_mlkem1024_public_key,
                &initiator_mlkem1024.ciphertext,
            )?;
            let session_binding = b"runtime-self-test:session";
            let initiator_triple_secrets = derive_triple_ratchet_initial_secrets(
                initiator_classical_secret.as_slice(),
                initiator_mlkem1024.shared_secret(),
                &alice_identity.public_key(),
                &bob_identity.public_key(),
                session_binding,
            )?;
            let responder_triple_secrets = derive_triple_ratchet_initial_secrets(
                responder_classical_secret.as_slice(),
                &responder_mlkem1024,
                &alice_identity.public_key(),
                &bob_identity.public_key(),
                session_binding,
            )?;
            ensure!(
                initiator_triple_secrets.ec_secret() == responder_triple_secrets.ec_secret()
                    && initiator_triple_secrets.scka_secret()
                        == responder_triple_secrets.scka_secret()
                    && initiator_triple_secrets.ec_secret()
                        != initiator_triple_secrets.scka_secret(),
                "pairwise runtime PQXDH key schedule failed"
            );

            let initiator_keys = derive_initial_keys(
                initiator_triple_secrets.ec_secret(),
                "runtime-self-test:session",
                "runtime-self-test:initiator",
                "runtime-self-test:responder",
            )?;
            let responder_keys = derive_initial_keys(
                responder_triple_secrets.ec_secret(),
                "runtime-self-test:session",
                "runtime-self-test:initiator",
                "runtime-self-test:responder",
            )?;
            ensure!(
                initiator_keys.root_key == responder_keys.root_key
                    && initiator_keys.initiator_chain_key == responder_keys.initiator_chain_key
                    && initiator_keys.responder_chain_key == responder_keys.responder_chain_key
                    && initiator_keys.initiator_chain_key != initiator_keys.responder_chain_key,
                "pairwise runtime key schedule failed"
            );

            let (_, initiator_classical_message_key) =
                advance_chain(&initiator_keys.initiator_chain_key, 1, 0, "message")?;
            let (_, responder_classical_message_key) =
                advance_chain(&responder_keys.initiator_chain_key, 1, 0, "message")?;
            ensure!(
                initiator_classical_message_key.as_ref()
                    == responder_classical_message_key.as_ref(),
                "pairwise runtime classical ratchet key mismatch"
            );
            let mut initiator_sparse_pq =
                SecureMeshSparsePqRatchet::new_initiator(initiator_triple_secrets.scka_secret())?;
            let mut responder_sparse_pq =
                SecureMeshSparsePqRatchet::new_responder(responder_triple_secrets.scka_secret())?;
            let initiator_post_quantum = initiator_sparse_pq.send_key()?;
            let responder_post_quantum =
                responder_sparse_pq.receive_key(&initiator_post_quantum.header)?;
            let initiator_message_key = derive_hybrid_message_key(
                &initiator_classical_message_key,
                &initiator_post_quantum.message_key,
                session_binding,
            )?;
            let responder_message_key = derive_hybrid_message_key(
                &responder_classical_message_key,
                &responder_post_quantum,
                session_binding,
            )?;
            ensure!(
                initiator_message_key.as_ref() == responder_message_key.as_ref(),
                "pairwise runtime Triple Ratchet key mismatch"
            );
            let nonce = [0x5au8; NONCE_LEN];
            let aad = b"licomesh-pairwise-runtime-self-test-aad";
            let plaintext = b"licomesh-pairwise-runtime-self-test-body";
            let cipher = ChaCha20Poly1305::new(Key::from_slice(initiator_message_key.as_ref()));
            let ciphertext = cipher
                .encrypt(
                    Nonce::from_slice(&nonce),
                    AeadPayload {
                        msg: plaintext,
                        aad,
                    },
                )
                .map_err(|_| anyhow!("pairwise runtime encryption failed"))?;
            ensure!(
                !ciphertext
                    .windows(plaintext.len())
                    .any(|window| window == plaintext),
                "pairwise runtime ciphertext exposed plaintext"
            );
            let opener = ChaCha20Poly1305::new(Key::from_slice(responder_message_key.as_ref()));
            let opened = opener
                .decrypt(
                    Nonce::from_slice(&nonce),
                    AeadPayload {
                        msg: &ciphertext,
                        aad,
                    },
                )
                .map_err(|_| anyhow!("pairwise runtime decryption failed"))?;
            ensure!(opened == plaintext, "pairwise runtime plaintext mismatch");
            let mut tampered = ciphertext;
            tampered[0] ^= 1;
            ensure!(
                opener
                    .decrypt(
                        Nonce::from_slice(&nonce),
                        AeadPayload {
                            msg: &tampered,
                            aad,
                        },
                    )
                    .is_err(),
                "pairwise runtime tamper was accepted"
            );
            Ok(())
        })()
        .is_ok()
    })
}
