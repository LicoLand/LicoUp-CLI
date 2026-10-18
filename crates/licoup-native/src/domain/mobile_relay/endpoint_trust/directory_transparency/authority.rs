use super::config::configured_kt_verifier;
use crate::core::secure_mesh_directory::SecureMeshDirectoryAuthority;
use crate::core::secure_mesh_transparency::KtFreshnessPolicy;
use crate::domain::mobile_relay::endpoint_trust::secure_mesh_kt_authority_path;
use crate::domain::mobile_relay::secret_custody::ensure_no_kt_authority_reset_in_progress;
use anyhow::Result;
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn open_mobile_relay_directory_authority(
    config: &Value,
    local_endpoint_id: &str,
) -> Result<SecureMeshDirectoryAuthority> {
    ensure_no_kt_authority_reset_in_progress()?;
    let settings = configured_kt_verifier(config)?;
    SecureMeshDirectoryAuthority::open(
        secure_mesh_kt_authority_path(local_endpoint_id)?,
        settings.pin.into_pin()?,
        KtFreshnessPolicy::strict(
            settings.max_sth_age_seconds,
            settings.max_future_skew_seconds,
        )?,
    )
}
