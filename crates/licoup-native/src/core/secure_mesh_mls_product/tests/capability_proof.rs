use super::support::*;

#[test]
fn secure_mesh_mls_wire_profile_ignores_app_version_and_rejects_revision_mismatch() {
    let simulated_app_versions = ["0.0.1-alpha", "0.0.2", "27.4.9"];
    let digests = simulated_app_versions
        .iter()
        .map(|_| secure_mesh_mls_build_protocol_digest().unwrap())
        .collect::<Vec<_>>();
    assert!(digests.windows(2).all(|pair| pair[0] == pair[1]));

    let device = device("mobile:mls-wire-profile-revision");
    let now = capability_now();
    for incompatible_revision in [
        SECURE_MESH_PROTOCOL_BUILD_REVISION - 1,
        SECURE_MESH_PROTOCOL_BUILD_REVISION + 1,
    ] {
        let incompatible_digest =
            secure_mesh_mls_build_protocol_digest_for_revision(incompatible_revision).unwrap();
        assert_ne!(digests[0], incompatible_digest);
        let request = CapabilityProofRequest {
            build_protocol_digest: incompatible_digest,
            policy_revision: SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION,
            challenge: [0x7e; 32],
            issued_at_unix_seconds: now.unix_timestamp() - 1,
            expires_at_unix_seconds: now.unix_timestamp() + 60,
        };
        let proof = sign_capability_proof(
            &device.identity,
            &device.signing_key,
            &capability_evaluation(),
            &request,
        )
        .unwrap();
        let error = crate::core::secure_mesh_capability_proof::verify_capability_proof(
            &device.identity,
            &proof,
            &mls_capability_verification_context(request.challenge, now).unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("build protocol binding"));
    }
}

#[test]
fn secure_mesh_mls_capability_proof_freshness_accepts_realistic_clock_windows() {
    let now = capability_now();

    for (name, proof_time, should_succeed) in [
        ("earlier", now - time::Duration::seconds(2), true),
        (
            "future-within-skew",
            now + time::Duration::seconds(CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS),
            true,
        ),
        (
            "future-beyond-skew",
            now + time::Duration::seconds(CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS + 1),
            false,
        ),
        (
            "expired",
            now - time::Duration::seconds(CAPABILITY_PROOF_MAX_LIFETIME_SECONDS + 1),
            false,
        ),
    ] {
        let owner = device(&format!("desktop_gui:freshness-owner-{name}"));
        let member = device(&format!("mobile:freshness-member-{name}"));
        let mut group = create_product_group(
            &owner.participant,
            &owner.identity,
            &DeviceTrustState::Verified,
            format!("freshness-{name}"),
        )
        .unwrap();
        let key_package = member.participant.generate_key_package().unwrap();
        let path = ledger_path(name);
        let mut ledger = SecureMeshMlsSecurityLedger::open(&path).unwrap();
        let result = add_test_product_member_with_times(
            &mut group,
            &owner,
            &member,
            &key_package,
            &mut ledger,
            &format!("kp-{name}"),
            proof_time,
            now,
        );
        assert_eq!(result.is_ok(), should_succeed, "freshness case {name}");
        let _ = std::fs::remove_file(path);
    }
}
