use rand_chacha::ChaCha12Rng as StdRng;
use rand_core::SeedableRng;

use super::{
    super::{protocol_state::MlKemBraidStateName, session::MlKemBraidSession},
    support::{TEST_SECRET, drive_until_epoch_one, record_key},
};

#[test]
fn full_rotation_emits_equal_epoch_key_and_restores_state() {
    let mut rng = StdRng::seed_from_u64(23);
    let mut alice = MlKemBraidSession::new_initiator(&TEST_SECRET).unwrap();
    let mut bob = MlKemBraidSession::new_responder(&TEST_SECRET).unwrap();
    let first = alice.send_with_rng(&mut rng).unwrap();
    bob.receive(&first.message).unwrap();
    let persisted = alice.persist().unwrap();
    alice = MlKemBraidSession::restore(&persisted).unwrap();

    let (alice_key, bob_key) = drive_until_epoch_one(&mut alice, &mut bob, &mut rng);
    assert_eq!(alice_key, bob_key);
    assert_eq!(alice.epoch(), 2);
    assert_eq!(bob.epoch(), 2);
    assert_eq!(alice.state_name(), MlKemBraidStateName::NoHeaderReceived);
    assert!(matches!(
        bob.state_name(),
        MlKemBraidStateName::KeysUnsampled | MlKemBraidStateName::KeysSampled
    ));
}

#[test]
fn reordered_and_lost_chunks_still_make_pcs_progress() {
    let mut rng = StdRng::seed_from_u64(31);
    let mut alice = MlKemBraidSession::new_initiator(&TEST_SECRET).unwrap();
    let mut bob = MlKemBraidSession::new_responder(&TEST_SECRET).unwrap();
    let mut a_to_b = Vec::new();
    let mut b_to_a = Vec::new();
    let mut alice_key = None;
    let mut bob_key = None;

    for tick in 0..1_500usize {
        let sent_a = alice.send_with_rng(&mut rng).unwrap();
        record_key(&mut alice_key, sent_a.output_key);
        if tick % 5 != 0 {
            a_to_b.push(sent_a.message);
        }

        let sent_b = bob.send_with_rng(&mut rng).unwrap();
        record_key(&mut bob_key, sent_b.output_key);
        if tick % 7 != 0 {
            b_to_a.push(sent_b.message);
        }

        if tick % 3 != 0 {
            if let Some(message) = a_to_b.pop() {
                let received = bob.receive(&message).unwrap();
                record_key(&mut bob_key, received.output_key);
            }
            if let Some(message) = b_to_a.pop() {
                let received = alice.receive(&message).unwrap();
                record_key(&mut alice_key, received.output_key);
            }
        }

        if alice_key.is_some() && bob_key.is_some() && alice.epoch() >= 2 && bob.epoch() >= 2 {
            break;
        }
    }
    assert_eq!(alice_key, bob_key);
    assert!(alice_key.is_some());
    assert!(alice.epoch() >= 2 && bob.epoch() >= 2);
}
