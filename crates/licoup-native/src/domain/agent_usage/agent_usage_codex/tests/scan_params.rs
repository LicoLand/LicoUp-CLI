use super::super::scan_params::{
    bool_param, expand_user_path, roots_fingerprint, source_key, usage_roots,
};
use serde_json::json;
use std::path::{Path, PathBuf};

#[test]
fn explicit_root_and_boolean_params_are_normalized_without_projection() {
    let params = json!({"historyRoot": " ./history ", "forceRefresh": "yes"});
    assert_eq!(usage_roots(&params), vec![PathBuf::from("./history")]);
    assert_eq!(bool_param(&params, "forceRefresh"), Some(true));
    assert_eq!(
        bool_param(&json!({"forceRefresh": "off"}), "forceRefresh"),
        Some(false)
    );
}

#[test]
fn fingerprints_and_source_keys_are_stable_digests_without_raw_paths() {
    let roots = vec![
        PathBuf::from("synthetic-root"),
        PathBuf::from("synthetic-root"),
    ];
    let fingerprint = roots_fingerprint(&roots, "0|");
    let key = source_key(&fingerprint, Path::new("synthetic-root/rollout.jsonl"));
    assert_eq!(fingerprint.len(), 64);
    assert_eq!(key.len(), 64);
    assert!(!fingerprint.contains("synthetic-root"));
    assert!(!key.contains("rollout"));
    assert_eq!(
        expand_user_path("relative/history"),
        PathBuf::from("relative/history")
    );
    let home = expand_user_path("~");
    assert!(
        !home
            .to_string_lossy()
            .starts_with(concat!("/", "System", "/", "Volumes", "/", "Data", "/"))
    );
}
