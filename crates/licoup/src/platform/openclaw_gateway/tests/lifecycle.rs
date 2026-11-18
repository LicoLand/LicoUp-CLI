use super::super::{DEFAULT_PORT, select_available_port_with};

#[test]
fn owned_port_scan_is_bounded_and_fails_closed() {
    let selected =
        select_available_port_with(DEFAULT_PORT, |port| port == DEFAULT_PORT + 2).unwrap();
    assert_eq!(selected, DEFAULT_PORT + 2);
    assert!(select_available_port_with(DEFAULT_PORT, |_| false).is_err());
}
