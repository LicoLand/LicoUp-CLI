use super::super::evaluate_prekey_inventory;

#[test]
fn low_water_equality_requests_local_replenishment() {
    let status = evaluate_prekey_inventory(true, 5, 1, 5, 1);
    assert!(status.signed_prekey_present);
    assert!(status.one_time_prekey_replenishment_required);
    assert!(status.key_package_replenishment_required);
}

#[test]
fn inventory_above_low_water_requires_no_replenishment() {
    let status = evaluate_prekey_inventory(true, 6, 2, 5, 1);
    assert!(!status.one_time_prekey_replenishment_required);
    assert!(!status.key_package_replenishment_required);
    assert_eq!(status.one_time_prekey_low_watermark, 5);
    assert_eq!(status.key_package_low_watermark, 1);
}

#[test]
fn inventory_preserves_signed_prekey_presence_independently() {
    let status = evaluate_prekey_inventory(false, 100, 100, 5, 5);
    assert!(!status.signed_prekey_present);
    assert_eq!(status.available_one_time_prekeys, 100);
    assert_eq!(status.available_key_packages, 100);
}
