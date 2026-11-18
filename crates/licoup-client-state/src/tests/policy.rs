#[test]
fn state_limits_and_collection_authority_are_explicit() {
    assert_eq!(crate::policy::MAX_ACTIVITY_FILE_BYTES, 64 * 1024 * 1024);
    assert_eq!(crate::policy::MAX_ACTIVITY_EVENT_BYTES, 4 * 1024 * 1024);
    assert_eq!(crate::policy::MAX_ACTIVITY_EVENTS, 10_000);
    assert_eq!(crate::policy::MAX_REDACTION_DEPTH, 64);
    assert_eq!(crate::policy::MAX_REDACTION_PATHS, 4_096);
    assert_eq!(crate::policy::COLLECTIONS.len(), 16);
    assert!(crate::policy::COLLECTIONS.contains(&"settings"));
    assert!(crate::policy::COLLECTIONS.contains(&"provider-quota-snapshots"));
    assert!(crate::policy::COLLECTIONS.contains(&"conversation-archive-profiles"));
    assert!(crate::policy::COLLECTIONS.contains(&"target-discovery-cache"));
    assert!(crate::policy::COLLECTIONS.contains(&"local-server-assemblies"));
    assert!(crate::policy::COLLECTIONS.contains(&"local-server-assembly-cleanup"));
    assert!(crate::policy::COLLECTIONS.contains(&"local-server-assembly-transaction"));
    assert!(crate::policy::COLLECTIONS.contains(&"mcp-install-transactions"));
}
