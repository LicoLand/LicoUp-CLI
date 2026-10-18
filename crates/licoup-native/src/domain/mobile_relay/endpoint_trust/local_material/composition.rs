use super::material_mutation::ensure_mobile_relay_endpoint_material;
use super::state_codec::local_endpoint_state;
use crate::domain::mobile_relay::endpoint_trust::ensure_mobile_relay_key_transparency;
use crate::domain::mobile_relay::secret_custody::RuntimeSecretMaterial;
use anyhow::Result;
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn ensure_mobile_relay_endpoint_descriptor(
    config: &mut Value,
    secret_material: &mut RuntimeSecretMaterial,
    endpoint_kind: &str,
) -> Result<Value> {
    ensure_mobile_relay_endpoint_material(config, secret_material, endpoint_kind)?;
    ensure_mobile_relay_key_transparency(config)?;
    local_endpoint_state(config, secret_material)?.public_descriptor()
}
