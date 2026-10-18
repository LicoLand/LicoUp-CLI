use crate::core::secure_mesh_transparency::reset_kt_persistent_authority_state;
use crate::domain::mobile_relay::endpoint_trust::{
    clear_mobile_relay_pairing_state, local_public_device_identity, secure_mesh_kt_authority_path,
};
use crate::domain::mobile_relay::key_transparency::config::{
    RuntimeSecretContext, begin_kt_authority_reset, kt_authority_reset_failpoint,
};
use crate::domain::mobile_relay::support::text_param;
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(super) fn reset_authority_state_if_required(
    config: &mut Value,
    secret_context: &mut RuntimeSecretContext,
    params: &Value,
    authority_changed: bool,
    reset_in_progress: bool,
) -> Result<()> {
    if !authority_changed && !reset_in_progress {
        return Ok(());
    }
    ensure!(
        text_param(params, &["confirmSecurityReset"]).as_deref()
            == Some("RESET_KEY_TRANSPARENCY_AUTHORITY"),
        "secure mesh KT authority replacement requires explicit security reset"
    );
    if !reset_in_progress {
        begin_kt_authority_reset()?;
    }
    kt_authority_reset_failpoint("after_guard_persisted")?;
    if let Ok(identity) = local_public_device_identity(config) {
        let (secret_store, authorization, namespace) = secret_context
            .secret_store_batch
            .authorization()?
            .ok_or_else(|| {
                anyhow!("secure mesh MLS selected custody is unavailable for authority reset")
            })?;
        crate::domain::secure_mesh_mls::reset_selected_custody_for_kt_authority_change(
            &identity,
            secret_store.as_ref(),
            &authorization,
            &namespace,
        )?;
    }
    kt_authority_reset_failpoint("after_mls_selected_custody_reset")?;
    crate::domain::secure_mesh_mls::reset_durable_state_for_kt_authority_change()?;
    kt_authority_reset_failpoint("after_mls_durable_state_reset")?;
    if let Some(endpoint_id) = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("endpointId"))
        .and_then(Value::as_str)
    {
        reset_kt_persistent_authority_state(secure_mesh_kt_authority_path(endpoint_id)?)?;
    }
    kt_authority_reset_failpoint("after_kt_authority_state_reset")?;
    clear_mobile_relay_pairing_state(config)?;
    kt_authority_reset_failpoint("after_pairwise_and_trust_reset")?;
    if let Some(e2ee) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        for key in [
            "keyTransparencyResponse",
            "keyTransparencyAuthorization",
            "pendingKeyTransparencyClaim",
            "pendingKeyTransparencyPurpose",
            "directoryVersion",
            "mlsKeyPackageVersion",
            "mlsKeyPackageDigest",
        ] {
            e2ee.remove(key);
        }
    }
    Ok(())
}
