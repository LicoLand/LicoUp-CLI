use super::super::port;

#[test]
fn bounded_selection_skips_reserved_and_busy_ports() {
    let selected =
        port::select_with(4096, 3, &[4096, 4097], "exhausted", |port| port == 4099).unwrap();
    assert_eq!(selected, 4099);
    assert!(port::select_with(5000, 2, &[], "exhausted", |_| false).is_err());
}
