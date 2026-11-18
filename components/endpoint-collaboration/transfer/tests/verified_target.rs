//! Verified-target fixtures.
//!
//! Every label here is synthetic: the subject, the source device and the new
//! endpoint identity are made-up strings. No fixture reads a real device
//! identity, a protected key or any provider value, and none of them performs a
//! retirement or an erase.

use licoup_endpoint_collaboration_transfer::{
    RequiredOwner, RetirementEligibility, RetirementRefusal, TargetBinding,
    VerifiedTargetEvidence,
};

fn binding() -> TargetBinding {
    TargetBinding {
        subject: "synthetic-subject".to_string(),
        source_device: "synthetic-source-device".to_string(),
        target_identity: "synthetic-new-endpoint".to_string(),
    }
}

#[test]
fn evidence_exists_only_when_every_required_owner_settled() {
    // Nothing settled, and every one-owner-missing subset: no partial evidence
    // exists for a caller to round up to "good enough".
    let mut partials = vec![Vec::new()];
    for missing in RequiredOwner::ALL {
        partials.push(
            RequiredOwner::ALL
                .into_iter()
                .filter(|owner| *owner != missing)
                .collect::<Vec<_>>(),
        );
    }
    for settled in partials {
        // Every partial is a *proper* subset of the required set: the empty one
        // and each one-owner-missing one beside it. The property is stated as the
        // missing owner rather than as an arithmetic relation, because the
        // arithmetic holds for the one-owner-missing subsets only and would
        // demand a length no settled set in this list can have.
        assert!(
            RequiredOwner::ALL
                .iter()
                .any(|owner| !settled.contains(owner)),
            "settled owners {settled:?} must leave a required owner unsettled"
        );
        assert!(
            VerifiedTargetEvidence::from_settled_owners(binding(), settled.clone()).is_none(),
            "settled owners {settled:?} must produce no evidence"
        );
    }

    let evidence = VerifiedTargetEvidence::from_settled_owners(binding(), RequiredOwner::ALL)
        .expect("every required owner settled");
    assert_eq!(evidence.verified_owners().count(), RequiredOwner::ALL.len());
}

#[test]
fn the_target_report_names_its_binding_and_verified_owners_and_decides_nothing() {
    let evidence =
        VerifiedTargetEvidence::from_settled_owners(binding(), RequiredOwner::ALL).unwrap();

    // What crosses the client bridge is a report: which subject, which source
    // device, which new identity, and which owners verified. It carries no
    // decision, so showing it to a person grants nothing.
    let report = serde_json::to_value(&evidence).unwrap();
    assert_eq!(report["binding"]["subject"], "synthetic-subject");
    assert_eq!(report["binding"]["sourceDevice"], "synthetic-source-device");
    assert_eq!(report["binding"]["targetIdentity"], "synthetic-new-endpoint");
    let verified = report["verified"].as_array().unwrap();
    assert_eq!(verified.len(), RequiredOwner::ALL.len());
    // Each settled owner is reported once, in the owner order the verification
    // itself follows.
    assert_eq!(
        verified,
        &[
            serde_json::json!("filesystem-payload"),
            serde_json::json!("database-stores"),
            serde_json::json!("platform-credentials"),
        ]
    );

    let mut fields = report
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["binding", "verified"],
        "the report has no admitted/eligible field a client could set"
    );
}

#[test]
fn retirement_is_decided_for_a_named_source_and_refuses_everything_else() {
    // No evidence at all: the ordinary outcome of a failed or abandoned transfer.
    assert_eq!(
        RetirementEligibility::decide_for(None, "synthetic-subject", "synthetic-source-device"),
        RetirementEligibility::Refused(RetirementRefusal::NoVerifiedTarget)
    );

    // Evidence that verified every owner, but not for the source being retired.
    let mismatch = RetirementEligibility::decide_for(
        VerifiedTargetEvidence::from_settled_owners(binding(), RequiredOwner::ALL),
        "synthetic-subject",
        "synthetic-other-device",
    );
    assert_eq!(
        mismatch,
        RetirementEligibility::Refused(RetirementRefusal::BindingMismatch)
    );
    assert!(!mismatch.is_eligible());
    assert!(mismatch.evidence().is_none());

    // The source the evidence names.
    let eligible = RetirementEligibility::decide_for(
        VerifiedTargetEvidence::from_settled_owners(binding(), RequiredOwner::ALL),
        "synthetic-subject",
        "synthetic-source-device",
    );
    assert!(eligible.is_eligible());
    assert!(eligible.evidence().is_some());
    assert!(eligible.evidence().unwrap().admits_source_retirement());
}

#[test]
fn every_refusal_explains_that_the_source_is_preserved() {
    for refusal in [
        RetirementRefusal::NoVerifiedTarget,
        RetirementRefusal::BindingMismatch,
    ] {
        assert!(refusal.reason().starts_with("the source is preserved: "));
    }
}
