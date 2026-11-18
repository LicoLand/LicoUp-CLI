use crate::domain::agent_hub::version::{concrete_display, parse_comparable, update_available};

#[test]
fn newer_latest_is_available() {
    assert!(update_available("0.42.1", "0.43.0"));
    assert!(update_available("v1.2.3", "1.2.4"));
    assert!(update_available("1.2", "1.2.1"));
}

#[test]
fn equal_or_older_latest_is_not_available() {
    assert!(!update_available("0.42.1", "0.42.1"));
    assert!(!update_available("1.2.3", "1.2.3"));
    assert!(!update_available("2.0.0", "1.9.9"));
}

#[test]
fn missing_or_unparseable_is_not_available() {
    assert!(!update_available("", "1.0.0"));
    assert!(!update_available("1.0.0", ""));
    assert!(!update_available("latest", "1.0.0"));
    assert!(!update_available("1.0.0", "latest"));
    assert!(!update_available("latest-stable", "vendor-latest"));
    assert!(!update_available("not-a-version", "1.0.0"));
    assert!(!update_available("1.0.0", "not-a-version"));
}

#[test]
fn concrete_display_strips_policy_and_keeps_installed_text() {
    assert_eq!(concrete_display("0.42.1"), "0.42.1");
    assert_eq!(concrete_display("v0.42.1"), "0.42.1");
    assert_eq!(concrete_display("codex-cli 0.42.1"), "0.42.1");
    assert_eq!(concrete_display("latest"), "");
    assert_eq!(concrete_display("latest-stable"), "");
    assert_eq!(concrete_display(""), "");
    assert!(parse_comparable("0.42").is_some());
}
