use rand_chacha::ChaCha12Rng as StdRng;
use rand_core::SeedableRng;

use super::{
    super::{
        session::MlKemBraidSession,
        transition::{checked_next_epoch, previous_epoch},
    },
    support::TEST_SECRET,
};

#[test]
fn duplicate_chunk_poisoning_is_fail_closed() {
    let mut rng = StdRng::seed_from_u64(7);
    let mut alice = MlKemBraidSession::new_initiator(&TEST_SECRET).unwrap();
    let mut bob = MlKemBraidSession::new_responder(&TEST_SECRET).unwrap();
    let sent = alice.send_with_rng(&mut rng).unwrap();
    bob.receive(&sent.message).unwrap();
    assert!(bob.receive(&sent.message).is_err());
    assert!(bob.is_poisoned());
    assert!(bob.send_with_rng(&mut rng).is_err());
}

#[test]
fn header_tamper_poisons_session() {
    let mut rng = StdRng::seed_from_u64(11);
    let mut alice = MlKemBraidSession::new_initiator(&TEST_SECRET).unwrap();
    let mut bob = MlKemBraidSession::new_responder(&TEST_SECRET).unwrap();
    for index in 0..3 {
        let mut sent = alice.send_with_rng(&mut rng).unwrap();
        if index == 1 {
            sent.message.data.as_mut().unwrap().bytes[0] ^= 1;
        }
        let result = bob.receive(&sent.message);
        if index < 2 {
            result.unwrap();
        } else {
            assert!(result.is_err());
        }
    }
    assert!(bob.is_poisoned());
}

#[test]
fn epoch_cannot_wrap() {
    assert!(checked_next_epoch(u64::MAX).is_err());
    assert_eq!(previous_epoch(1).unwrap(), 0);
}
