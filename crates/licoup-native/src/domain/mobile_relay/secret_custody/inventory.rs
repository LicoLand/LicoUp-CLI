use super::*;
use crate::domain::mobile_relay::endpoint_trust::local_identity_metadata_present;
use licoup_private_gateway::credential::{LLM_API_KEY_INVENTORY_SCHEMA, LlmApiKeyProvider};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Redacted classification of locally held credential classes and their
/// recovery paths. The document never contains secret values, labels or
/// opaque platform secret-store handles.
pub(in crate::domain::mobile_relay) const CREDENTIAL_CUSTODY_INVENTORY_SCHEMA: &str =
    "licoup.credential-custody-inventory.v1";

/// Data-root-relative path whose owner is `PlatformLlmApiKeyVault`.
///
/// The archive transports this non-secret metadata document, but metadata is
/// never treated as custody or key availability.
const LLM_API_KEY_INVENTORY_FILE: &str = "llm-api-key-inventory.json";
const MAX_PROVIDER_KEY_METADATA_BYTES: usize = 64 * 1024;
const MAX_PROVIDER_KEY_METADATA_ENTRIES: usize = 64;

pub(in crate::domain::mobile_relay) const CUSTODY_LOCATION_PLATFORM_SECRET_STORE: &str =
    "platformSecretStore";
pub(in crate::domain::mobile_relay) const CUSTODY_LOCATION_PORTABLE_CONFIG: &str = "portableConfig";
pub(in crate::domain::mobile_relay) const CUSTODY_LOCATION_SELECTED_UNAVAILABLE: &str =
    "selectedCustodyUnavailable";
pub(in crate::domain::mobile_relay) const CUSTODY_LOCATION_NOT_OBSERVED: &str =
    "notObservedWithoutAuthorization";

/// One credential class the client can hold locally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::mobile_relay) enum CredentialCustodyClass {
    DeviceIdentityKey,
    ProviderApiKey,
    RelayAccessToken,
    PlatformOpaqueCredential,
}

impl CredentialCustodyClass {
    pub(in crate::domain::mobile_relay) const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceIdentityKey => "deviceIdentityKey",
            Self::ProviderApiKey => "providerApiKey",
            Self::RelayAccessToken => "relayAccessToken",
            Self::PlatformOpaqueCredential => "platformOpaqueCredential",
        }
    }
}

/// Explicit path by which a credential can be recovered or reauthorized.
/// None of these actions copy an active secret to another device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::mobile_relay) enum CredentialRecoveryAction {
    RetainAuthorizedLocalCustody,
    ObserveLocalCustody,
    ReauthorizeProvider,
    RePairDevice,
    ReacquireThroughPlatformCustody,
}

impl CredentialRecoveryAction {
    pub(in crate::domain::mobile_relay) const fn as_str(self) -> &'static str {
        match self {
            Self::RetainAuthorizedLocalCustody => "retainAuthorizedLocalCustody",
            Self::ObserveLocalCustody => "observeLocalCustody",
            Self::ReauthorizeProvider => "reauthorizeProvider",
            Self::RePairDevice => "rePairDevice",
            Self::ReacquireThroughPlatformCustody => "reacquireThroughPlatformCustody",
        }
    }
}

/// Non-secret owner facts read from the provider key inventory document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::domain::mobile_relay) struct ProviderApiKeyFact {
    pub(in crate::domain::mobile_relay) credential_id: String,
    pub(in crate::domain::mobile_relay) provider: String,
    pub(in crate::domain::mobile_relay) expired: bool,
}

/// Custody observations supplied by the owning secret-store and identity
/// owners. They are observations, not authority, and never contain values.
/// `None` means the owner did not observe that class without an authorized
/// operation; it is never reported as availability.
#[derive(Clone, Debug, Default)]
pub(in crate::domain::mobile_relay) struct CredentialCustodyObservations {
    pub(in crate::domain::mobile_relay) selected_custody_backend: String,
    pub(in crate::domain::mobile_relay) identity_material_in_selected_custody: Option<bool>,
    pub(in crate::domain::mobile_relay) provider_key_material_in_selected_custody: Option<bool>,
    // Keyed by the exact token field or paired-device credential key. An
    // observed sibling must never imply that another credential is available.
    pub(in crate::domain::mobile_relay) relay_token_material: BTreeMap<String, bool>,
    pub(in crate::domain::mobile_relay) opaque_platform_items: Vec<String>,
}

/// Classify one data root's credential classes into a redacted inventory.
///
/// Reads only the root-owned provider key metadata document; custody state is
/// then combined with the caller's owner observations. Every credential
/// instance appears exactly once and reports an explicit recovery action.
pub(in crate::domain::mobile_relay) fn credential_custody_inventory(
    data_root: &Path,
    config: &Value,
    observations: &CredentialCustodyObservations,
) -> Result<Value> {
    let provider_api_keys = read_provider_api_key_inventory_metadata(data_root)?;
    Ok(classify_credential_custody(
        config,
        &provider_api_keys,
        observations,
    ))
}

/// Read non-secret provider key facts from the data-root inventory document.
///
/// The owner document is validated against its schema and bounded before use.
/// Missing or empty metadata is reported as no entries, never as custody.
pub(in crate::domain::mobile_relay) fn read_provider_api_key_inventory_metadata(
    data_root: &Path,
) -> Result<Vec<ProviderApiKeyFact>> {
    if !data_root.is_dir() {
        return Ok(Vec::new());
    }
    let Some(text) = licoup_foundation::platform::file_security::read_private_text_bounded(
        &data_root.join(LLM_API_KEY_INVENTORY_FILE),
        MAX_PROVIDER_KEY_METADATA_BYTES,
    )?
    else {
        return Ok(Vec::new());
    };
    let value: Value =
        serde_json::from_str(&text).map_err(|_| anyhow!("llm_api_key_inventory_invalid"))?;
    ensure!(
        value.get("schemaVersion").and_then(Value::as_str) == Some(LLM_API_KEY_INVENTORY_SCHEMA),
        "llm_api_key_inventory_invalid"
    );
    let entries = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?;
    ensure!(
        entries.len() <= MAX_PROVIDER_KEY_METADATA_ENTRIES,
        "llm_api_key_inventory_capacity_exceeded"
    );
    let now_epoch_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut seen = BTreeSet::new();
    let mut facts = Vec::with_capacity(entries.len());
    for entry in entries {
        let credential_id = entry
            .get("credentialId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?;
        ensure!(
            Uuid::parse_str(credential_id).is_ok(),
            "llm_api_key_inventory_invalid"
        );
        let provider_value = entry
            .get("provider")
            .cloned()
            .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?;
        let provider: LlmApiKeyProvider = serde_json::from_value(provider_value)
            .map_err(|_| anyhow!("llm_api_key_inventory_invalid"))?;
        let expires_at_epoch_seconds = match entry.get("expiresAtEpochSeconds") {
            Some(value) => Some(
                value
                    .as_u64()
                    .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?,
            ),
            None => None,
        };
        ensure!(
            seen.insert(credential_id.to_string()),
            "llm_api_key_inventory_inconsistent"
        );
        facts.push(ProviderApiKeyFact {
            credential_id: credential_id.to_string(),
            provider: provider.as_str().to_string(),
            expired: expires_at_epoch_seconds
                .is_some_and(|expires_at| now_epoch_seconds >= expires_at),
        });
    }
    Ok(facts)
}

/// Classify the credential classes present in the owner config and observed
/// custody into a deterministic, redacted inventory document.
pub(in crate::domain::mobile_relay) fn classify_credential_custody(
    config: &Value,
    provider_api_keys: &[ProviderApiKeyFact],
    observations: &CredentialCustodyObservations,
) -> Value {
    let mut entries: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for fact in provider_api_keys {
        push_entry(
            &mut entries,
            &mut seen,
            json!({
                "credentialRef": format!("llm-api-key:{}", fact.credential_id),
                "class": CredentialCustodyClass::ProviderApiKey.as_str(),
                "provider": fact.provider,
                "expired": fact.expired,
                "custody": custody_location(observations.provider_key_material_in_selected_custody),
                "metadataPortable": true,
                "secretPortable": false,
                "recovery": recovery_action(observations.provider_key_material_in_selected_custody, CredentialRecoveryAction::ReauthorizeProvider),
            }),
        );
    }

    for (field, presence_field, reference) in [
        ("pcToken", "pcTokenPresent", "relay-token:pcToken"),
        (
            "mobileToken",
            "mobileTokenPresent",
            "relay-token:mobileToken",
        ),
    ] {
        if relay_token_present(config, field, presence_field) {
            push_entry(
                &mut entries,
                &mut seen,
                json!({
                    "credentialRef": reference,
                    "class": CredentialCustodyClass::RelayAccessToken.as_str(),
                    "custody": token_custody_location(config, field, observations),
                    "metadataPortable": true,
                    "secretPortable": false,
                    "recovery": recovery_action(observations.relay_token_material.get(field).copied(), CredentialRecoveryAction::RePairDevice),
                }),
            );
        }
    }
    if let Some(devices) = config.get("pairedDevices").and_then(Value::as_array) {
        for device in devices {
            if !paired_device_credential_present(device) {
                continue;
            }
            let Some(key) = paired_device_token_secret_store_key(device) else {
                continue;
            };
            push_entry(
                &mut entries,
                &mut seen,
                json!({
                    "credentialRef": format!("relay-token:{key}"),
                    "class": CredentialCustodyClass::RelayAccessToken.as_str(),
                    "custody": if device
                        .get("mobileToken")
                        .and_then(Value::as_str)
                        .is_some_and(is_unredacted_secret)
                    {
                        CUSTODY_LOCATION_PORTABLE_CONFIG
                    } else {
                        custody_location(observations.relay_token_material.get(&key).copied())
                    },
                    "metadataPortable": true,
                    "secretPortable": false,
                    "recovery": recovery_action(observations.relay_token_material.get(&key).copied(), CredentialRecoveryAction::RePairDevice),
                }),
            );
        }
    }

    if let Some(e2ee) = config.get("mobileRelayE2ee").and_then(Value::as_object) {
        if local_identity_metadata_present(e2ee) {
            let endpoint_id = e2ee
                .get("endpointId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("unassigned");
            push_entry(
                &mut entries,
                &mut seen,
                json!({
                    "credentialRef": format!("device-identity:{endpoint_id}"),
                    "class": CredentialCustodyClass::DeviceIdentityKey.as_str(),
                    "custody": custody_location(observations.identity_material_in_selected_custody),
                    "metadataPortable": true,
                    "secretPortable": false,
                    "recovery": recovery_action(observations.identity_material_in_selected_custody, CredentialRecoveryAction::ReacquireThroughPlatformCustody),
                }),
            );
        }
    }

    let opaque: BTreeSet<_> = observations
        .opaque_platform_items
        .iter()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect();
    for (index, _) in opaque.iter().enumerate() {
        push_entry(
            &mut entries,
            &mut seen,
            json!({
                "credentialRef": format!("platform-opaque:{}", index + 1),
                "class": CredentialCustodyClass::PlatformOpaqueCredential.as_str(),
                "custody": CUSTODY_LOCATION_PLATFORM_SECRET_STORE,
                "metadataPortable": false,
                "secretPortable": false,
                "recovery": CredentialRecoveryAction::ReacquireThroughPlatformCustody.as_str(),
            }),
        );
    }

    entries.sort_by_key(|entry| {
        (
            entry
                .get("class")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            entry
                .get("credentialRef")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    });
    let mut counts_by_class: BTreeMap<&'static str, usize> = BTreeMap::new();
    for entry in &entries {
        if let Some(class) = entry.get("class").and_then(Value::as_str) {
            *counts_by_class
                .entry(static_class_label(class))
                .or_default() += 1;
        }
    }
    json!({
        "schemaVersion": CREDENTIAL_CUSTODY_INVENTORY_SCHEMA,
        "redacted": true,
        "secretValuesIncluded": false,
        "metadataProvesCustody": false,
        "selectedCustodyBackend": observations.selected_custody_backend.trim(),
        "entryCount": entries.len(),
        "countsByClass": counts_by_class,
        "entries": entries,
    })
}

fn push_entry(entries: &mut Vec<Value>, seen: &mut BTreeSet<String>, entry: Value) {
    let Some(reference) = entry
        .get("credentialRef")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    if seen.insert(reference) {
        entries.push(entry);
    }
}

fn static_class_label(class: &str) -> &'static str {
    for candidate in [
        CredentialCustodyClass::DeviceIdentityKey,
        CredentialCustodyClass::ProviderApiKey,
        CredentialCustodyClass::RelayAccessToken,
        CredentialCustodyClass::PlatformOpaqueCredential,
    ] {
        if candidate.as_str() == class {
            return candidate.as_str();
        }
    }
    "unknown"
}

fn custody_location(material_in_selected_custody: Option<bool>) -> &'static str {
    match material_in_selected_custody {
        Some(true) => CUSTODY_LOCATION_PLATFORM_SECRET_STORE,
        Some(false) => CUSTODY_LOCATION_SELECTED_UNAVAILABLE,
        None => CUSTODY_LOCATION_NOT_OBSERVED,
    }
}

fn recovery_action(observed: Option<bool>, unavailable: CredentialRecoveryAction) -> &'static str {
    match observed {
        Some(true) => CredentialRecoveryAction::RetainAuthorizedLocalCustody.as_str(),
        None => CredentialRecoveryAction::ObserveLocalCustody.as_str(),
        Some(false) => unavailable.as_str(),
    }
}

fn token_custody_location(
    config: &Value,
    field: &str,
    observations: &CredentialCustodyObservations,
) -> &'static str {
    if config
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(is_unredacted_secret)
    {
        return CUSTODY_LOCATION_PORTABLE_CONFIG;
    }
    custody_location(observations.relay_token_material.get(field).copied())
}

fn relay_token_present(config: &Value, field: &str, presence_field: &str) -> bool {
    config
        .get(presence_field)
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || config
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(is_unredacted_secret)
}

fn paired_device_credential_present(device: &Value) -> bool {
    device
        .get("credentialPresent")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || device
            .get("mobileToken")
            .and_then(Value::as_str)
            .is_some_and(is_unredacted_secret)
}
