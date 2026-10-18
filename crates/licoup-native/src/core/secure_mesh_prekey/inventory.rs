pub const SECURE_MESH_PREKEY_STATUS: &str = "signed_curve_prekey_one_time_curve_prekey_signed_one_time_mlkem1024_prekey_keypackage_validation_low_water_available_pqxdh_runtime_available";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshInventoryStatus {
    pub signed_prekey_present: bool,
    pub available_one_time_prekeys: usize,
    pub available_key_packages: usize,
    pub one_time_prekey_low_watermark: usize,
    pub key_package_low_watermark: usize,
    pub one_time_prekey_replenishment_required: bool,
    pub key_package_replenishment_required: bool,
}

pub fn evaluate_prekey_inventory(
    signed_prekey_present: bool,
    available_one_time_prekeys: usize,
    available_key_packages: usize,
    one_time_prekey_low_watermark: usize,
    key_package_low_watermark: usize,
) -> SecureMeshInventoryStatus {
    SecureMeshInventoryStatus {
        signed_prekey_present,
        available_one_time_prekeys,
        available_key_packages,
        one_time_prekey_low_watermark,
        key_package_low_watermark,
        one_time_prekey_replenishment_required: available_one_time_prekeys
            <= one_time_prekey_low_watermark,
        key_package_replenishment_required: available_key_packages <= key_package_low_watermark,
    }
}
