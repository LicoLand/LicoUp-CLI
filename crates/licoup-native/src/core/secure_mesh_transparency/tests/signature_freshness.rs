use super::super::*;
use super::support::leaf;
use ed25519_dalek::SigningKey;
use rand_core::OsRng;

#[test]
fn pinned_sth_rejects_wrong_key_stale_and_future_views() {
    let mut log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let body = leaf("device-a", 1, "active");
    let index = log.append_leaf(&body).unwrap();
    let proof = log.inclusion_proof_at(index, 100).unwrap();
    let policy = KtFreshnessPolicy::strict(10, 2).unwrap();
    verify_kt_inclusion(&proof, &log.pin(), policy, 105).unwrap();

    let wrong_log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    assert!(verify_kt_inclusion(&proof, &wrong_log.pin(), policy, 105).is_err());
    assert!(
        verify_kt_inclusion(&proof, &log.pin(), policy, 111)
            .unwrap_err()
            .to_string()
            .contains("stale")
    );

    let future = log.inclusion_proof_at(index, 110).unwrap();
    assert!(
        verify_kt_inclusion(&future, &log.pin(), policy, 100)
            .unwrap_err()
            .to_string()
            .contains("future")
    );
}

#[test]
fn freshness_policy_can_only_tighten_protocol_hard_limits() {
    assert!(KtFreshnessPolicy::strict(0, 0).is_err());
    assert!(KtFreshnessPolicy::strict(KT_PROTOCOL_MAX_STH_AGE_SECONDS + 1, 0).is_err());
    assert!(KtFreshnessPolicy::strict(60, KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS + 1).is_err());
    assert!(KtFreshnessPolicy::strict(u64::MAX, u64::MAX).is_err());
    KtFreshnessPolicy::strict(
        KT_PROTOCOL_MAX_STH_AGE_SECONDS,
        KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS,
    )
    .unwrap();
}

#[test]
fn cross_language_integer_contract_rejects_values_above_json_safe_range() {
    let log = SecureMeshKtLog::new(SigningKey::generate(&mut OsRng));
    let mut sth = log.sign_tree_head(100).unwrap();
    sth.tree_size = KT_JSON_SAFE_INTEGER_MAX + 1;
    let error = sth
        .verify(&log.pin(), KtFreshnessPolicy::strict(60, 2).unwrap(), 100)
        .unwrap_err();
    assert!(error.to_string().contains("cross-language safe range"));
}
