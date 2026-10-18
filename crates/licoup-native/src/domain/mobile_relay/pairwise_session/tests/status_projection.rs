use super::super::status_projection::AuthorizedPairwiseSessionStatus;

#[test]
fn blocked_status_never_projects_established_capabilities() {
    let status = AuthorizedPairwiseSessionStatus::blocked("pairwise_session_missing");
    assert!(!status.established);
    assert_eq!(status.blocker, Some("pairwise_session_missing"));
    assert!(status.capability_projection.is_none());
}
