use super::super::{
    DEFAULT_PORT, VENDOR_DEFAULT_PORT, is_reserved_conflict_port, policy,
    select_available_port_with,
};

#[test]
fn vendor_port_is_attach_only_and_never_selected_for_owned_start() {
    assert!(is_reserved_conflict_port(VENDOR_DEFAULT_PORT));
    assert!(!is_reserved_conflict_port(DEFAULT_PORT));
    let selected =
        select_available_port_with(VENDOR_DEFAULT_PORT, |port| !is_reserved_conflict_port(port))
            .unwrap();
    assert_ne!(selected, VENDOR_DEFAULT_PORT);
    assert_eq!(policy::PORT_EXHAUSTED, "openclaw_gateway_port_exhausted");
}
