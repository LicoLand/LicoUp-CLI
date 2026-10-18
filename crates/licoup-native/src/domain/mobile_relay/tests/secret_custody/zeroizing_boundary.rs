use crate::core::secure_mesh_secret_store::{SecretBytes, SecretZeroizeProbe};
use base64::{Engine as _, engine::general_purpose};

use super::super::test_support::*;

const FIRST_CANARY: &[u8] = b"synthetic-bundle-canary-alpha";
const SECOND_CANARY: &[u8] = b"synthetic-bundle-canary-beta";

fn secret(bytes: &[u8]) -> SecretBytes {
    SecretBytes::try_from_bytes(bytes.to_vec()).unwrap()
}

fn encoded_frame(entries: &[(u8, &[u8])]) -> SecretBytes {
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&MOBILE_RELAY_SECRET_BUNDLE_MAGIC);
    encoded.push(MOBILE_RELAY_SECRET_BUNDLE_VERSION);
    encoded.push(u8::try_from(entries.len()).unwrap());
    for (tag, bytes) in entries {
        encoded.push(*tag);
        encoded.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_be_bytes());
        encoded.extend_from_slice(bytes);
    }
    SecretBytes::try_from_bytes(encoded).unwrap()
}

fn assert_serialized_without_canaries(value: &Value, canaries: &[&str]) {
    let serialized = serde_json::to_string(value).unwrap();
    for canary in canaries {
        assert!(
            !serialized.contains(canary),
            "secret canary escaped a JSON projection"
        );
    }
}

fn pair_runtime_secret_material_consumers(
    pc_config: &mut Value,
    pc_material: &mut RuntimeSecretMaterial,
    mobile_config: &mut Value,
    mobile_material: &mut RuntimeSecretMaterial,
) {
    let pc_descriptor =
        ensure_mobile_relay_endpoint_descriptor(pc_config, pc_material, "desktop_sidecar").unwrap();
    ensure_mobile_relay_endpoint_descriptor(mobile_config, mobile_material, "mobile").unwrap();
    apply_peer_secure_mesh_descriptor(mobile_config, mobile_material, &pc_descriptor, true)
        .unwrap();
    let mobile_descriptor =
        ensure_mobile_relay_endpoint_descriptor(mobile_config, mobile_material, "mobile").unwrap();
    apply_peer_secure_mesh_descriptor(pc_config, pc_material, &mobile_descriptor, true).unwrap();
    ensure_mobile_relay_endpoint_descriptor(pc_config, pc_material, "desktop_sidecar").unwrap();
}

#[test]
fn admitted_and_native_secrets_drive_real_pairing_prekey_and_secure_command_consumers_without_json_residue()
 {
    let dir = temp_dir("mobile-relay-zeroizing-runtime-boundary");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    const CANARY_A_RAW: &[u8; 32] = b"runtime-canary-alpha-00000000000";
    const CANARY_B_RAW: &[u8; 32] = b"runtime-canary-beta--00000000000";
    let canary_a = general_purpose::URL_SAFE_NO_PAD.encode(CANARY_A_RAW);
    let canary_b = general_purpose::URL_SAFE_NO_PAD.encode(CANARY_B_RAW);
    let canaries = [canary_a.as_str(), canary_b.as_str()];
    let namespace = "runtime-secret-material-acceptance";
    let store = EphemeralSecretStore::new();
    let bundle_handle = native_e2ee_secret_bundle_handle_for_namespace(namespace).unwrap();
    let native_bundle = MobileRelayE2eeSecretBundle::try_from_fields(vec![(
        MobileRelayE2eeSecretField::PairingSecret,
        SecretBytes::try_from_bytes(canary_a.as_bytes().to_vec()).unwrap(),
    )])
    .unwrap();
    store
        .set_secret(
            &bundle_handle,
            encode_mobile_relay_e2ee_secret_bundle(native_bundle).unwrap(),
        )
        .unwrap();

    let mut pc_config = default_config();
    let mut pc_material = RuntimeSecretMaterial::new();
    let mut pc_overrides = RuntimeSecretOverrides::default();
    hydrate_runtime_secret_material_from_secret_store(
        &pc_config,
        &mut pc_material,
        &mut pc_overrides,
        &store,
        namespace,
    )
    .unwrap();
    assert_eq!(
        pc_material
            .e2ee_secret(MobileRelayE2eeSecretField::PairingSecret)
            .unwrap()
            .expose_utf8()
            .unwrap(),
        canary_a
    );

    let admitted_a_probe = SecretZeroizeProbe::new();
    let admitted_b_probe = SecretZeroizeProbe::new();
    let mut mobile_config = default_config();
    let mut mobile_context = RuntimeSecretContext::default();
    apply_pairing_invite_params_with_context(
        &mut mobile_config,
        &json!({"e2eePairingSecret": canary_a}),
        Some(&mut mobile_context),
    )
    .unwrap();
    mobile_context
        .material
        .attach_test_zeroize_probe(
            MobileRelayE2eeSecretField::PairingSecret,
            admitted_a_probe.clone(),
        )
        .unwrap();

    pair_runtime_secret_material_consumers(
        &mut pc_config,
        &mut pc_material,
        &mut mobile_config,
        &mut mobile_context.material,
    );
    let pc_descriptor = local_endpoint_state(&pc_config, &pc_material)
        .unwrap()
        .public_descriptor()
        .unwrap();
    let mobile_descriptor = local_endpoint_state(&mobile_config, &mobile_context.material)
        .unwrap()
        .public_descriptor()
        .unwrap();
    let proof_a = mobile_relay_claim_proof_for_pair(
        &mobile_config,
        &mobile_context.material,
        "synthetic-pairing-id",
        &mobile_descriptor,
        &pc_descriptor,
    )
    .unwrap();
    assert!(
        mobile_relay_claim_proof_matches(
            &pc_config,
            &pc_material,
            "synthetic-pairing-id",
            &mobile_descriptor,
            &pc_descriptor,
            &proof_a,
        )
        .unwrap()
    );

    apply_pairing_invite_params_with_context(
        &mut mobile_config,
        &json!({"e2eePairingSecret": canary_b}),
        Some(&mut mobile_context),
    )
    .unwrap();
    mobile_context
        .material
        .attach_test_zeroize_probe(
            MobileRelayE2eeSecretField::PairingSecret,
            admitted_b_probe.clone(),
        )
        .unwrap();
    assert_eq!(
        admitted_a_probe.observations(),
        vec![vec![0; canary_a.len()]],
        "replacing admitted runtime material must wipe its previous owner"
    );
    let proof_b = mobile_relay_claim_proof_for_pair(
        &mobile_config,
        &mobile_context.material,
        "synthetic-pairing-id",
        &mobile_descriptor,
        &pc_descriptor,
    )
    .unwrap();
    assert_ne!(
        proof_a, proof_b,
        "the pairing consumer must use the supplied bytes"
    );
    assert!(
        !mobile_relay_claim_proof_matches(
            &pc_config,
            &pc_material,
            "synthetic-pairing-id",
            &mobile_descriptor,
            &pc_descriptor,
            &proof_b,
        )
        .unwrap(),
        "a carrier with different bytes must fail the real pairing verifier"
    );

    let rejected_probe = SecretZeroizeProbe::new();
    let rejected = mobile_context
        .material
        .insert_e2ee_secret(
            MobileRelayE2eeSecretField::PairingSecret,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                b"synthetic-rejected-duplicate".to_vec(),
                rejected_probe.clone(),
            )
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(
        rejected_probe.observations(),
        vec![vec![0; b"synthetic-rejected-duplicate".len()]]
    );
    for canary in canaries {
        assert!(!format!("{rejected:?}").contains(canary));
    }

    store
        .set_secret(
            &bundle_handle,
            encode_mobile_relay_e2ee_secret_bundle(
                MobileRelayE2eeSecretBundle::try_from_fields(vec![(
                    MobileRelayE2eeSecretField::PairingSecret,
                    SecretBytes::try_from_bytes(canary_b.as_bytes().to_vec()).unwrap(),
                )])
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    hydrate_runtime_secret_material_from_secret_store(
        &pc_config,
        &mut pc_material,
        &mut pc_overrides,
        &store,
        namespace,
    )
    .unwrap();
    assert!(
        mobile_relay_claim_proof_matches(
            &pc_config,
            &pc_material,
            "synthetic-pairing-id",
            &mobile_descriptor,
            &pc_descriptor,
            &proof_b,
        )
        .unwrap()
    );

    pair_runtime_secret_material_consumers(
        &mut pc_config,
        &mut pc_material,
        &mut mobile_config,
        &mut mobile_context.material,
    );
    let command = secure_command_payload(
        &mobile_config,
        &mobile_context.material,
        "agent.message.send",
        Some("synthetic-agent"),
        "synthetic-workspace",
        json!({"message": "synthetic command"}),
    )
    .unwrap();
    let envelope = seal_mobile_relay_payload(
        &mobile_config,
        &mobile_context.material,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
        &command,
    )
    .unwrap();
    let opened = open_mobile_relay_payload(
        &pc_config,
        &pc_material,
        &envelope,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::Command,
    )
    .unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&opened).unwrap(), command);

    let public_config_projection = public_config(&mobile_config);
    let public_status_projection = pairing_status_response(&mobile_config);
    let serialized_command_output = with_config(
        json!({"ok": true, "secureCommand": envelope, "bodyRedacted": true}),
        &mobile_config,
    );
    for projection in [
        &pc_config,
        &mobile_config,
        &pc_descriptor,
        &mobile_descriptor,
        &command,
        &public_config_projection,
        &public_status_projection,
        &serialized_command_output,
    ] {
        assert_serialized_without_canaries(projection, &canaries);
    }

    drop(mobile_context);
    assert_eq!(
        admitted_b_probe.observations(),
        vec![vec![0; canary_b.len()]],
        "final RuntimeSecretMaterial Drop must wipe the last admitted owner"
    );
    set_portable_data_dir_override(previous);
}

#[test]
fn typed_secret_bundle_round_trips_through_a_bounded_non_json_codec() {
    let private_probe = SecretZeroizeProbe::new();
    let signing_probe = SecretZeroizeProbe::new();
    let bundle = MobileRelayE2eeSecretBundle::try_from_fields(vec![
        (
            MobileRelayE2eeSecretField::PrivateKey,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                FIRST_CANARY.to_vec(),
                private_probe.clone(),
            )
            .unwrap(),
        ),
        (
            MobileRelayE2eeSecretField::SigningKey,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                SECOND_CANARY.to_vec(),
                signing_probe.clone(),
            )
            .unwrap(),
        ),
    ])
    .unwrap();

    let encoded = encode_mobile_relay_e2ee_secret_bundle(bundle).unwrap();
    assert_eq!(
        private_probe.observations(),
        vec![vec![0; FIRST_CANARY.len()]]
    );
    assert_eq!(
        signing_probe.observations(),
        vec![vec![0; SECOND_CANARY.len()]]
    );
    assert!(
        serde_json::from_slice::<serde_json::Value>(encoded.expose_bytes()).is_err(),
        "the custody bundle crossing the store port must not be a generic JSON value"
    );
    let encoded_debug = format!("{encoded:?}");
    assert!(!encoded_debug.contains(std::str::from_utf8(FIRST_CANARY).unwrap()));
    assert!(!encoded_debug.contains(std::str::from_utf8(SECOND_CANARY).unwrap()));

    let decoded = decode_mobile_relay_e2ee_secret_bundle(encoded).unwrap();
    assert_eq!(
        decoded
            .secret(MobileRelayE2eeSecretField::PrivateKey)
            .unwrap()
            .expose_bytes(),
        FIRST_CANARY
    );
    assert_eq!(
        decoded
            .secret(MobileRelayE2eeSecretField::SigningKey)
            .unwrap()
            .expose_bytes(),
        SECOND_CANARY
    );
    let decoded_debug = format!("{decoded:?}");
    assert!(decoded_debug.to_ascii_lowercase().contains("redacted"));
    assert!(!decoded_debug.contains(std::str::from_utf8(FIRST_CANARY).unwrap()));
    assert!(!decoded_debug.contains(std::str::from_utf8(SECOND_CANARY).unwrap()));
}

#[test]
fn typed_bundle_merge_wipes_replaced_fields_and_drop_wipes_retained_fields() {
    let replaced_probe = SecretZeroizeProbe::new();
    let retained_probe = SecretZeroizeProbe::new();
    let incoming_probe = SecretZeroizeProbe::new();
    let existing = MobileRelayE2eeSecretBundle::try_from_fields(vec![
        (
            MobileRelayE2eeSecretField::PrivateKey,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                FIRST_CANARY.to_vec(),
                replaced_probe.clone(),
            )
            .unwrap(),
        ),
        (
            MobileRelayE2eeSecretField::SigningKey,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                b"synthetic-retained-secret".to_vec(),
                retained_probe.clone(),
            )
            .unwrap(),
        ),
    ])
    .unwrap();
    let incoming = MobileRelayE2eeSecretBundle::try_from_fields(vec![(
        MobileRelayE2eeSecretField::PrivateKey,
        SecretBytes::try_from_bytes_with_test_zeroize_probe(
            SECOND_CANARY.to_vec(),
            incoming_probe.clone(),
        )
        .unwrap(),
    )])
    .unwrap();

    let merged = existing.merge_replacing(incoming).unwrap();
    assert_eq!(
        replaced_probe.observations(),
        vec![vec![0; FIRST_CANARY.len()]],
        "replacement must drop and wipe the superseded owned buffer"
    );
    assert_eq!(
        merged
            .secret(MobileRelayE2eeSecretField::PrivateKey)
            .unwrap()
            .expose_bytes(),
        SECOND_CANARY
    );
    assert!(retained_probe.observations().is_empty());
    assert!(incoming_probe.observations().is_empty());

    drop(merged);
    assert_eq!(
        retained_probe.observations(),
        vec![vec![0; b"synthetic-retained-secret".len()]]
    );
    assert_eq!(
        incoming_probe.observations(),
        vec![vec![0; SECOND_CANARY.len()]]
    );
}

#[test]
fn typed_bundle_codec_rejects_malformed_unknown_duplicate_truncated_and_oversize_input() {
    let private_tag = MobileRelayE2eeSecretField::PrivateKey.wire_tag();

    let mut wrong_magic = encoded_frame(&[(private_tag, FIRST_CANARY)])
        .expose_bytes()
        .to_vec();
    wrong_magic[0] ^= 0xff;
    let malformed = decode_mobile_relay_e2ee_secret_bundle(secret(&wrong_magic)).unwrap_err();

    let unknown = decode_mobile_relay_e2ee_secret_bundle(encoded_frame(&[(u8::MAX, FIRST_CANARY)]))
        .unwrap_err();
    let duplicate = decode_mobile_relay_e2ee_secret_bundle(encoded_frame(&[
        (private_tag, FIRST_CANARY),
        (private_tag, SECOND_CANARY),
    ]))
    .unwrap_err();

    let mut truncated = Vec::new();
    truncated.extend_from_slice(&MOBILE_RELAY_SECRET_BUNDLE_MAGIC);
    truncated.push(MOBILE_RELAY_SECRET_BUNDLE_VERSION);
    truncated.push(1);
    truncated.push(private_tag);
    truncated.extend_from_slice(&8u32.to_be_bytes());
    truncated.extend_from_slice(b"short");
    let truncated = decode_mobile_relay_e2ee_secret_bundle(secret(&truncated)).unwrap_err();

    let mut oversized_field = Vec::new();
    oversized_field.extend_from_slice(&MOBILE_RELAY_SECRET_BUNDLE_MAGIC);
    oversized_field.push(MOBILE_RELAY_SECRET_BUNDLE_VERSION);
    oversized_field.push(1);
    oversized_field.push(private_tag);
    oversized_field.extend_from_slice(
        &u32::try_from(MOBILE_RELAY_SECRET_FIELD_MAX_BYTES + 1)
            .unwrap()
            .to_be_bytes(),
    );
    let oversized_field =
        decode_mobile_relay_e2ee_secret_bundle(secret(&oversized_field)).unwrap_err();

    let oversized_bundle = decode_mobile_relay_e2ee_secret_bundle(
        SecretBytes::try_from_bytes(vec![b'x'; MOBILE_RELAY_SECRET_BUNDLE_MAX_BYTES + 1]).unwrap(),
    )
    .unwrap_err();

    assert_eq!(
        malformed.to_string(),
        "mobile_relay_secret_bundle_malformed"
    );
    assert_eq!(
        unknown.to_string(),
        "mobile_relay_secret_bundle_unknown_field"
    );
    assert_eq!(
        duplicate.to_string(),
        "mobile_relay_secret_bundle_duplicate_field"
    );
    assert_eq!(
        truncated.to_string(),
        "mobile_relay_secret_bundle_truncated"
    );
    assert_eq!(
        oversized_field.to_string(),
        "mobile_relay_secret_bundle_field_oversize"
    );
    assert_eq!(
        oversized_bundle.to_string(),
        "mobile_relay_secret_bundle_oversize"
    );

    for error in [
        malformed,
        unknown,
        duplicate,
        truncated,
        oversized_field,
        oversized_bundle,
    ] {
        let error = format!("{error:?}");
        assert!(!error.contains(std::str::from_utf8(FIRST_CANARY).unwrap()));
        assert!(!error.contains(std::str::from_utf8(SECOND_CANARY).unwrap()));
    }
}

#[test]
fn typed_bundle_schema_rejects_duplicate_and_oversize_fields_before_encoding() {
    let duplicate = MobileRelayE2eeSecretBundle::try_from_fields(vec![
        (MobileRelayE2eeSecretField::PrivateKey, secret(FIRST_CANARY)),
        (
            MobileRelayE2eeSecretField::PrivateKey,
            secret(SECOND_CANARY),
        ),
    ])
    .unwrap_err();
    assert_eq!(
        duplicate.to_string(),
        "mobile_relay_secret_bundle_duplicate_field"
    );

    let mut oversized = vec![b'x'; MOBILE_RELAY_SECRET_FIELD_MAX_BYTES + 1];
    oversized[..FIRST_CANARY.len()].copy_from_slice(FIRST_CANARY);
    let oversized = MobileRelayE2eeSecretBundle::try_from_fields(vec![(
        MobileRelayE2eeSecretField::PrivateKey,
        SecretBytes::try_from_bytes(oversized).unwrap(),
    )])
    .unwrap_err();
    assert_eq!(
        oversized.to_string(),
        "mobile_relay_secret_bundle_field_oversize"
    );
    assert!(!format!("{oversized:?}").contains(std::str::from_utf8(FIRST_CANARY).unwrap()));
}
