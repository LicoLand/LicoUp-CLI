use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use anyhow::{Result, ensure};

use super::group_model::SecureMeshMlsGroup;
use super::participant::SecureMeshMlsParticipant;

static MLS_RUNTIME_CRYPTO_SELF_TEST: OnceLock<bool> = OnceLock::new();

/// Exercises a complete in-memory OpenMLS create/add/join/application-message
/// round trip. The cached result is side-effect free with respect to client
/// storage and avoids repeating an expensive provider initialization.
pub fn runtime_crypto_self_test() -> bool {
    *MLS_RUNTIME_CRYPTO_SELF_TEST.get_or_init(|| {
        catch_unwind(AssertUnwindSafe(|| -> Result<()> {
            let alice = SecureMeshMlsParticipant::new(b"runtime-self-test:alice".to_vec())?;
            let bob = SecureMeshMlsParticipant::new(b"runtime-self-test:bob".to_vec())?;
            let bob_key_package = bob.generate_key_package()?;
            ensure!(
                !bob_key_package.as_public_bytes().is_empty(),
                "MLS runtime key package is empty"
            );
            let mut alice_group = SecureMeshMlsGroup::create(&alice, b"runtime-self-test:group")?;
            let welcome =
                alice_group.add_member_for_runtime_crypto_self_test(&alice, &bob_key_package)?;
            ensure!(
                !welcome.commit_message.is_empty() && !welcome.welcome_message.is_empty(),
                "MLS runtime welcome is incomplete"
            );
            let mut bob_group = SecureMeshMlsGroup::join_from_welcome_for_runtime_crypto_self_test(
                &bob,
                &welcome.welcome_message,
            )?;
            let aad = b"licomesh-mls-runtime-self-test-aad";
            let plaintext = b"licomesh-mls-runtime-self-test-body";
            let sealed = alice_group.seal_application_message(&alice, aad, plaintext)?;
            ensure!(
                !sealed
                    .windows(plaintext.len())
                    .any(|window| window == plaintext),
                "MLS runtime ciphertext exposed plaintext"
            );
            let opened = bob_group
                .open_application_message_for_runtime_crypto_self_test(&bob, aad, &sealed)?;
            ensure!(opened == plaintext, "MLS runtime plaintext mismatch");
            Ok(())
        }))
        .is_ok_and(|result| result.is_ok())
    })
}
