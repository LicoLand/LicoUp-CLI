use super::super::test_support::*;

#[test]
fn mobile_relay_existing_corrupt_config_fails_closed_without_replacement() {
    let dir = temp_dir("mobile-relay-corrupt-config-fails-closed");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let path = config_path().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let corrupt = b"{not-valid-json";
    fs::write(&path, corrupt).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let error = load_config().unwrap_err().to_string();
    assert!(error.contains("exists but is invalid"));
    assert_eq!(fs::read(&path).unwrap(), corrupt);

    set_portable_data_dir_override(previous);
}

#[cfg(unix)]
#[test]
fn mobile_relay_existing_insecure_config_permissions_fail_closed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("mobile-relay-insecure-config-permissions");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let path = config_path().unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, serde_json::to_vec(&default_config()).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let error = load_config().unwrap_err().to_string();
    assert!(error.contains("owner-only") || error.contains("permissions"));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_stale_config_snapshot_cannot_overwrite_newer_commit() {
    let dir = temp_dir("mobile-relay-stale-config-cas");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut winner = load_config().unwrap();
    let mut stale = winner.clone();
    winner["pcClientName"] = json!("winner");
    save_config(&mut winner).unwrap();
    stale["pcClientName"] = json!("stale-loser");

    let error = save_config(&mut stale).unwrap_err().to_string();
    assert!(error.contains("snapshot is stale"));
    let durable = load_config_without_persistence().unwrap();
    assert_eq!(durable["pcClientName"], "winner");
    assert_eq!(
        durable[CONFIG_GENERATION_FIELD],
        winner[CONFIG_GENERATION_FIELD]
    );

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_concurrent_config_writers_commit_exactly_one_snapshot() {
    use std::sync::Barrier;

    let dir = temp_dir("mobile-relay-concurrent-config-cas");
    let previous = set_portable_data_dir_override(Some(dir.clone()));
    let snapshot = load_config().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for label in ["writer-a", "writer-b"] {
        let root = dir.clone();
        let barrier = barrier.clone();
        let mut candidate = snapshot.clone();
        candidate["pcClientName"] = json!(label);
        handles.push(thread::spawn(move || {
            let prior = set_portable_data_dir_override(Some(root.to_path_buf()));
            barrier.wait();
            let result = save_config(&mut candidate).map(|_| label.to_string());
            set_portable_data_dir_override(prior);
            result
        }));
    }
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let durable = load_config_without_persistence().unwrap();
    assert!(matches!(
        durable["pcClientName"].as_str(),
        Some("writer-a" | "writer-b")
    ));

    set_portable_data_dir_override(previous);
}

#[test]
fn mobile_relay_public_config_redacts_secret_material() {
    let dir = temp_dir("mobile-relay-redacted-config");
    let previous = set_portable_data_dir_override(Some(dir.to_path_buf()));
    let mut config = default_config();
    config["pairingId"] = json!("pair-redacted");
    config["pcToken"] = json!("pc-token-redaction-canary");
    config["mobileToken"] = json!("mobile-token-redaction-canary");
    ensure_mobile_relay_endpoint_descriptor(
        &mut config,
        &mut test_runtime_secret_material(stringify!(&mut config)),
        "mobile",
    )
    .unwrap();
    config["pairedDevices"] = json!([
        {
            "id": "pc-redacted",
            "pcClientId": "pc-redacted",
            "pcClientName": "Mac",
            "pairingId": "pair-redacted",
            "mobileToken": "paired-device-token-redaction-canary"
        }
    ]);
    let runtime_secrets = MobileRelayE2eeSecretField::ALL
        .map(|field| test_runtime_e2ee_secret(stringify!(&config), field));
    persist_test_runtime_secret_material(stringify!(&config)).unwrap();
    save_config(&mut config).unwrap();

    let output = config_get(&json!({})).unwrap();
    let serialized = serde_json::to_string(&output).unwrap();
    for secret in [
        "pc-token-redaction-canary",
        "mobile-token-redaction-canary",
        "paired-device-token-redaction-canary",
        runtime_secrets[0].as_str(),
        runtime_secrets[1].as_str(),
        runtime_secrets[2].as_str(),
        runtime_secrets[3].as_str(),
        runtime_secrets[4].as_str(),
        runtime_secrets[5].as_str(),
    ] {
        assert!(
            !serialized.contains(secret),
            "public mobile relay config leaked secret canary: {secret}"
        );
    }
    assert_eq!(output["config"]["pcToken"], "");
    assert_eq!(output["config"]["mobileToken"], "");
    assert_eq!(output["config"]["pcTokenPresent"], true);
    assert_eq!(output["config"]["mobileTokenPresent"], true);
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["privateKeyMaterial"],
        "redacted"
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["signingKeyMaterial"],
        "redacted"
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["signedPrekeyPrivateKeyMaterial"],
        "redacted"
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["oneTimePrekeyPrivateKeyMaterial"],
        "redacted"
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["oneTimeMlKem1024PrekeySeedMaterial"],
        "redacted"
    );
    assert_eq!(
        output["config"]["mobileRelayE2ee"]["pairingSecretMaterial"],
        "redacted"
    );
    assert_eq!(output["config"]["pairedDevices"][0]["mobileToken"], "");
    assert_eq!(
        output["config"]["pairedDevices"][0]["credentialPresent"],
        true
    );

    set_portable_data_dir_override(previous);
}
