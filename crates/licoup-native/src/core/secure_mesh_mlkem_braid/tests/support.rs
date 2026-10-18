use rand_chacha::ChaCha12Rng as StdRng;

use super::super::{output::MlKemBraidOutputKey, session::MlKemBraidSession};

pub(super) const TEST_SECRET: [u8; 32] = [0x5a; 32];

pub(super) fn drive_until_epoch_one(
    alice: &mut MlKemBraidSession,
    bob: &mut MlKemBraidSession,
    rng: &mut StdRng,
) -> ([u8; 32], [u8; 32]) {
    let mut alice_key = None;
    let mut bob_key = None;
    for _ in 0..512 {
        let sent_a = alice.send_with_rng(rng).unwrap();
        let sending_epoch_a = sent_a.sending_epoch;
        record_key(&mut alice_key, sent_a.output_key);
        let received_b = bob.receive(&sent_a.message).unwrap();
        assert_eq!(received_b.receiving_epoch, sending_epoch_a);
        record_key(&mut bob_key, received_b.output_key);

        let sent_b = bob.send_with_rng(rng).unwrap();
        let sending_epoch_b = sent_b.sending_epoch;
        record_key(&mut bob_key, sent_b.output_key);
        let received_a = alice.receive(&sent_b.message).unwrap();
        assert_eq!(received_a.receiving_epoch, sending_epoch_b);
        record_key(&mut alice_key, received_a.output_key);
        if alice_key.is_some() && bob_key.is_some() && alice.epoch() == 2 && bob.epoch() == 2 {
            break;
        }
    }
    (alice_key.unwrap(), bob_key.unwrap())
}

pub(super) fn record_key(target: &mut Option<[u8; 32]>, candidate: Option<MlKemBraidOutputKey>) {
    if let Some(key) = candidate {
        if key.epoch() == 1 {
            *target = Some(*key.key());
        }
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}
