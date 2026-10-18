use super::super::concurrency::{BoundedGate, LimitFailure};
use std::time::Duration;

#[test]
fn gate_fails_fast_at_capacity_and_releases_on_drop() {
    let gate = BoundedGate::new(1);
    let permit = gate.acquire(Duration::ZERO).unwrap();
    assert_eq!(gate.acquire(Duration::ZERO).err(), Some(LimitFailure::Busy));
    drop(permit);
    assert!(gate.acquire(Duration::ZERO).is_ok());
}
