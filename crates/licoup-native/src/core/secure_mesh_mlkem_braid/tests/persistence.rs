use super::super::{constants::MAX_PERSISTED_SESSION_BYTES, session::MlKemBraidSession};

#[test]
fn persisted_state_rejects_oversized_input_before_deserialization() {
    let oversized = vec![b' '; MAX_PERSISTED_SESSION_BYTES + 1];
    assert!(MlKemBraidSession::restore(&oversized).is_err());
}
