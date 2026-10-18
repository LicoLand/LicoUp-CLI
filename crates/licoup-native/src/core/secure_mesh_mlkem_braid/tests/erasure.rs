use super::super::{erasure_decoder::ErasureDecoder, erasure_encoder::ErasureEncoder};

#[test]
fn reed_solomon_known_answer_and_any_n_recovery() {
    let mut message = vec![0u8; 64];
    for symbol in message[..32].chunks_exact_mut(2) {
        symbol.copy_from_slice(&1u16.to_be_bytes());
    }
    for symbol in message[32..].chunks_exact_mut(2) {
        symbol.copy_from_slice(&2u16.to_be_bytes());
    }
    let mut encoder = ErasureEncoder::new(&message).unwrap();
    let first = encoder.next_chunk().unwrap();
    let _second = encoder.next_chunk().unwrap();
    let parity = encoder.next_chunk().unwrap();
    assert_eq!(first.point(), 0);
    assert!(
        parity
            .bytes()
            .chunks_exact(2)
            .all(|symbol| symbol == 7u16.to_be_bytes())
    );

    let mut decoder = ErasureDecoder::new(64).unwrap();
    decoder.add_chunk(&parity).unwrap();
    decoder.add_chunk(&first).unwrap();
    assert_eq!(decoder.take_message().unwrap(), message);
}
