use super::*;
use crate::core::secure_mesh_secret_store::SecretBytes;
use licoup_foundation::platform::file_security::{
    atomic_write_private_text_bounded, ensure_private_dir,
};
use std::collections::BTreeSet;

struct TemporaryRoot(PathBuf);

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temporary_root(name: &str) -> TemporaryRoot {
    let path = env::temp_dir().join(format!("licoup-{name}-{}", Uuid::new_v4()));
    fs::create_dir_all(&path).unwrap();
    TemporaryRoot(path)
}

#[test]
fn credential_custody_inventory_classifies_each_credential_once_without_values() {
    let root = temporary_root("credential-custody-inventory");
    ensure_private_dir(&root.0).unwrap();
    let metadata = json!({
        "schemaVersion": "licoup.llm-api-key-inventory.v1",
        "leaseDays": 30,
        "entries": [
            {
                "credentialId": "8a1f3f70-0000-4000-8000-000000000001",
                "provider": "deepseek",
                "label": "Synthetic DeepSeek label",
                "createdAtEpochSeconds": 1,
                "expiresAtEpochSeconds": 2
            },
            {
                "credentialId": "8a1f3f70-0000-4000-8000-000000000002",
                "provider": "kimi",
                "label": "Synthetic Kimi label",
                "createdAtEpochSeconds": 1
            }
        ]
    });
    atomic_write_private_text_bounded(
        &root.0.join("llm-api-key-inventory.json"),
        &serde_json::to_string(&metadata).unwrap(),
        64 * 1024,
    )
    .unwrap();
    let config = json!({
        "pcToken": "pc-token-value-canary",
        "mobileToken": "",
        "mobileTokenPresent": true,
        "pairedDevices": [
            {"id": "pc-a", "pairingId": "pair-a", "mobileToken": "", "credentialPresent": true},
            {"id": "pc-b", "pairingId": "pair-a", "mobileToken": "", "credentialPresent": true}
        ],
        "mobileRelayE2ee": {
            "endpointId": "pc_fixture-identity",
            "publicKeyBase64url": "synthetic-public-key",
            "fingerprint": "synthetic-fingerprint",
            "signingPublicKeyBase64url": "synthetic-signing-key"
        }
    });
    let observations = CredentialCustodyObservations {
        selected_custody_backend: "memory-only-ephemeral".to_string(),
        identity_material_in_selected_custody: Some(false),
        provider_key_material_in_selected_custody: Some(false),
        relay_token_material: ["pcToken", "mobileToken"]
            .into_iter()
            .map(|key| (key.to_string(), false))
            .chain(
                config["pairedDevices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(paired_device_token_secret_store_key)
                    .map(|key| (key, false)),
            )
            .collect(),
        opaque_platform_items: vec!["legacyProtectedInventory".to_string()],
    };

    let inventory = credential_custody_inventory(&root.0, &config, &observations).unwrap();

    assert_eq!(
        inventory["schemaVersion"],
        CREDENTIAL_CUSTODY_INVENTORY_SCHEMA
    );
    assert_eq!(inventory["redacted"], true);
    assert_eq!(inventory["secretValuesIncluded"], false);
    assert_eq!(inventory["metadataProvesCustody"], false);
    assert_eq!(inventory["entryCount"], 7);
    let entries = inventory["entries"].as_array().unwrap();
    let mut references = BTreeSet::new();
    for entry in entries {
        let reference = entry["credentialRef"].as_str().unwrap();
        assert!(
            references.insert(reference.to_string()),
            "credential classified more than once: {reference}"
        );
        assert_eq!(entry["secretPortable"], false);
        match entry["class"].as_str().unwrap() {
            "providerApiKey" => assert_eq!(entry["recovery"], "reauthorizeProvider"),
            "relayAccessToken" => assert_eq!(entry["recovery"], "rePairDevice"),
            "deviceIdentityKey" => assert_eq!(entry["recovery"], "reacquireThroughPlatformCustody"),
            "platformOpaqueCredential" => {
                assert_eq!(entry["recovery"], "reacquireThroughPlatformCustody");
            }
            class => panic!("unexpected credential class: {class}"),
        }
    }
    assert_eq!(references.len(), entries.len());
    let portable_token = entries
        .iter()
        .find(|entry| entry["credentialRef"] == "relay-token:pcToken")
        .unwrap();
    assert_eq!(portable_token["custody"], CUSTODY_LOCATION_PORTABLE_CONFIG);
    let identity = entries
        .iter()
        .find(|entry| entry["class"] == "deviceIdentityKey")
        .unwrap();
    assert_eq!(identity["custody"], CUSTODY_LOCATION_SELECTED_UNAVAILABLE);
    let serialized = serde_json::to_string(&inventory).unwrap();
    assert!(!serialized.contains("pc-token-value-canary"));
    assert!(!serialized.contains("Synthetic DeepSeek label"));
    assert!(!serialized.contains("Synthetic Kimi label"));
    assert!(!serialized.contains("legacyProtectedInventory"));
    for (observed, action) in [
        (Some(true), "retainAuthorizedLocalCustody"),
        (None, "observeLocalCustody"),
    ] {
        let mut facts = observations.clone();
        facts.identity_material_in_selected_custody = observed;
        let report = credential_custody_inventory(&root.0, &config, &facts).unwrap();
        let identity = report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["class"] == "deviceIdentityKey")
            .unwrap();
        assert_eq!(identity["recovery"], action);
        assert_eq!(report["metadataProvesCustody"], false);
    }
}

#[test]
fn custody_inventory_does_not_promote_unobserved_or_missing_sibling_tokens() {
    let config = json!({
        "pcTokenPresent": true, "mobileTokenPresent": true,
        "pairedDevices": [
            {"id":"pc-a", "pairingId":"pair-a", "credentialPresent":true},
            {"id":"pc-b", "pairingId":"pair-b", "credentialPresent":true}
        ]
    });
    let devices = config["pairedDevices"].as_array().unwrap();
    let a = paired_device_token_secret_store_key(&devices[0]).unwrap();
    let b = paired_device_token_secret_store_key(&devices[1]).unwrap();
    let facts = CredentialCustodyObservations {
        relay_token_material: [
            ("pcToken".to_string(), true),
            ("mobileToken".to_string(), false),
            (a.clone(), true),
        ]
        .into(),
        ..Default::default()
    };
    let report = classify_credential_custody(&config, &[], &facts);
    for (key, custody, action) in [
        (
            "pcToken",
            CUSTODY_LOCATION_PLATFORM_SECRET_STORE,
            "retainAuthorizedLocalCustody",
        ),
        (
            "mobileToken",
            CUSTODY_LOCATION_SELECTED_UNAVAILABLE,
            "rePairDevice",
        ),
        (
            &a,
            CUSTODY_LOCATION_PLATFORM_SECRET_STORE,
            "retainAuthorizedLocalCustody",
        ),
        (&b, CUSTODY_LOCATION_NOT_OBSERVED, "observeLocalCustody"),
    ] {
        let entry = report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["credentialRef"] == format!("relay-token:{key}"))
            .unwrap();
        assert_eq!(entry["custody"], custody);
        assert_eq!(entry["recovery"], action);
    }
}

#[test]
fn provider_api_key_inventory_metadata_is_validated_and_deduplicated() {
    let root = temporary_root("provider-key-metadata");
    ensure_private_dir(&root.0).unwrap();
    let path = root.0.join("llm-api-key-inventory.json");

    atomic_write_private_text_bounded(&path, "{not json", 64 * 1024).unwrap();
    let malformed = read_provider_api_key_inventory_metadata(&root.0)
        .unwrap_err()
        .to_string();
    assert!(
        malformed.contains("llm_api_key_inventory_invalid"),
        "{malformed}"
    );

    let duplicate = json!({
        "schemaVersion": "licoup.llm-api-key-inventory.v1",
        "leaseDays": 7,
        "entries": [
            {
                "credentialId": "8a1f3f70-0000-4000-8000-000000000001",
                "provider": "kilo",
                "label": "Synthetic label",
                "createdAtEpochSeconds": 1
            },
            {
                "credentialId": "8a1f3f70-0000-4000-8000-000000000001",
                "provider": "kilo",
                "label": "Synthetic label",
                "createdAtEpochSeconds": 2
            }
        ]
    });
    atomic_write_private_text_bounded(
        &path,
        &serde_json::to_string(&duplicate).unwrap(),
        64 * 1024,
    )
    .unwrap();
    let inconsistent = read_provider_api_key_inventory_metadata(&root.0)
        .unwrap_err()
        .to_string();
    assert!(
        inconsistent.contains("llm_api_key_inventory_inconsistent"),
        "{inconsistent}"
    );

    fs::remove_file(&path).unwrap();
    assert!(
        read_provider_api_key_inventory_metadata(&root.0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn runtime_override_policy_rejects_unredacted_secret_fields() {
    assert!(contains_unredacted_token_secret_override(&json!({
        "pcToken": "test-only-token"
    })));
    assert!(contains_unredacted_e2ee_secret_override(&json!({
        "privateKeyBase64url": "test-only-private-material"
    })));
    assert!(!contains_unredacted_token_secret_override(&json!({
        "pcToken": "redacted"
    })));
    assert!(!contains_unredacted_e2ee_secret_override(&json!({
        "privateKeyBase64url": "***"
    })));
}

#[test]
fn native_secret_bundle_roundtrip_is_field_allowlisted() {
    let encoded = encode_mobile_relay_e2ee_secret_bundle(
        MobileRelayE2eeSecretBundle::try_from_fields(vec![
            (
                MobileRelayE2eeSecretField::PrivateKey,
                SecretBytes::try_from_bytes(b"test-private".to_vec()).unwrap(),
            ),
            (
                MobileRelayE2eeSecretField::SigningKey,
                SecretBytes::try_from_bytes(b"test-signing".to_vec()).unwrap(),
            ),
        ])
        .unwrap(),
    )
    .unwrap();
    let decoded = decode_mobile_relay_e2ee_secret_bundle(encoded).unwrap();
    assert!(
        decoded
            .secret(MobileRelayE2eeSecretField::PrivateKey)
            .is_some()
    );
    assert!(
        decoded
            .secret(MobileRelayE2eeSecretField::SigningKey)
            .is_some()
    );
}
