//! Retirement-eligibility fixtures.
//!
//! Every fixture is synthetic: the subject, the source device and the new
//! identity are made-up labels, and no real device or protected key is involved.

use licoup_endpoint_collaboration_replacement::{LostDeviceActivation, RetirementDecision};
use licoup_endpoint_collaboration_transfer::{
    ActivationReason, CredentialCustody, IdentityActivationRequirement, LostActivationPath,
    RequiredOwner, RetirementRefusal, TargetBinding, VerifiedTargetEvidence,
};

/// The subject, source device and new identity every fixture uses. They are made-up
/// labels: no real device and no protected key is involved.
const SUBJECT: &str = "synthetic-subject";
const SOURCE_DEVICE: &str = "synthetic-source-device";
const TARGET_IDENTITY: &str = "synthetic-new-endpoint";

fn binding() -> TargetBinding {
    TargetBinding {
        subject: SUBJECT.to_string(),
        source_device: SOURCE_DEVICE.to_string(),
        target_identity: TARGET_IDENTITY.to_string(),
    }
}

/// Settled evidence for every required owner, which is the only way to obtain it.
fn verified_target() -> VerifiedTargetEvidence {
    VerifiedTargetEvidence::from_settled_owners(binding(), RequiredOwner::ALL)
        .expect("every required owner settled")
}

#[test]
fn a_failed_or_abandoned_transfer_leaves_the_source_alone() {
    for evidence in [
        // No transfer ran at all.
        None,
        // A transfer ran but one owner never verified. `from_settled_owners`
        // produces no evidence, so the flow sees the same refusal.
        RequiredOwner::ALL
            .into_iter()
            .find(|owner| *owner != RequiredOwner::PlatformCredentials)
            .map(|owner| {
                VerifiedTargetEvidence::from_settled_owners(binding(), [owner])
            })
            .flatten(),
    ] {
        let decision = RetirementDecision::decide_for(evidence, SUBJECT, SOURCE_DEVICE);
        assert_eq!(
            decision,
            RetirementDecision::Refused(RetirementRefusal::NoVerifiedTarget)
        );
        assert!(!decision.is_admitted());
        assert!(decision.binding().is_none());
        assert!(decision.reason().contains("source is preserved"));
    }
}

#[test]
fn partial_owner_evidence_is_not_evidence_at_all() {
    for settled in [
        vec![],
        vec![RequiredOwner::FilesystemPayload],
        vec![
            RequiredOwner::FilesystemPayload,
            RequiredOwner::DatabaseStores,
        ],
        vec![
            RequiredOwner::FilesystemPayload,
            RequiredOwner::PlatformCredentials,
        ],
        vec![RequiredOwner::PlatformCredentials],
    ] {
        assert!(
            VerifiedTargetEvidence::from_settled_owners(binding(), settled.clone()).is_none(),
            "settled owners {settled:?} must not produce verified-target evidence"
        );
    }
    // Every owner, in any order, does.
    let mut reversed = RequiredOwner::ALL;
    reversed.reverse();
    let evidence = VerifiedTargetEvidence::from_settled_owners(binding(), reversed).unwrap();
    assert_eq!(evidence.verified_owners().count(), RequiredOwner::ALL.len());
    assert!(evidence.admits_source_retirement());
}

#[test]
fn only_a_fully_verified_target_admits_source_retirement() {
    let decision =
        RetirementDecision::decide_for(Some(verified_target()), SUBJECT, SOURCE_DEVICE);
    assert!(decision.is_admitted());
    assert_eq!(decision.binding(), Some(&binding()));
    assert!(decision.reason().contains("source may be retired"));
}

#[test]
fn a_verified_target_is_not_spendable_on_another_source() {
    // The same subject's other device, and another subject entirely: neither may
    // be retired on evidence that belongs to the source above.
    for (subject, source_device) in [
        ("synthetic-other-subject", SOURCE_DEVICE),
        (SUBJECT, "synthetic-other-device"),
    ] {
        let decision =
            RetirementDecision::decide_for(Some(verified_target()), subject, source_device);
        assert_eq!(
            decision,
            RetirementDecision::Refused(RetirementRefusal::BindingMismatch),
            "{subject}/{source_device} must not be retired by another source's verified target"
        );
        assert!(!decision.is_admitted());
        assert!(decision.binding().is_none());
        assert!(decision.reason().contains("belongs to another subject or source device"));
    }

    // The source the evidence actually names is still admitted.
    assert!(
        RetirementDecision::decide_for(Some(verified_target()), SUBJECT, SOURCE_DEVICE).is_admitted()
    );
}

#[test]
fn lost_device_activation_needs_no_transfer_history_or_old_acknowledgement() {
    let mut path = LostActivationPath::new(IdentityActivationRequirement::for_inventory(std::iter::empty::<CredentialCustody>()));
    path.report_unavailable("unsynchronized-conversations");
    path.report_unavailable("provider-retained-history");
    path.report_unavailable("unsynchronized-conversations");

    let activation = LostDeviceActivation::from_accepted_authority(path);
    assert!(!activation.waits_for_transfer());
    assert_eq!(
        activation.unavailable_content(),
        ["provider-retained-history", "unsynchronized-conversations"],
        "unavailable content is reported once each, in a stable order"
    );
    assert!(
        activation
            .requirement()
            .reasons()
            .contains(&ActivationReason::BackupPossessionIsNotAuthority),
        "activation is admitted on authority, not on a backup"
    );
    assert!(!activation.requirement().permits_activation());

    // The activation is independent of any retirement decision: a lost device
    // activates whether or not a transfer ever ran.
    assert!(!RetirementDecision::decide_for(None, SUBJECT, SOURCE_DEVICE).is_admitted());
}
