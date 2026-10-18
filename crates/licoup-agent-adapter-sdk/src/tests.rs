//! The SDK's own claims: the transition vocabulary reduces the way the
//! conversation authority reads it, the text reconciler emits each byte once,
//! and the parser set answers only from the registrations it was handed.
//!
//! Every claim that names an Agent — which parsers exist, which declaration
//! each one reports, what one Agent's parser does with a vendor frame — is a
//! claim about the composition and is asserted where both halves are in view,
//! in `licoup-native`'s `native_agent_parser`.

use super::*;
use crate::adapters::AdapterContract;
use crate::port::{AdapterParserSet, ParserRegistration};

#[test]
fn adapter_sdk_reconciles_delta_and_cumulative_text_once() {
    let mut reconciler = TextReconciler::default();
    assert_eq!(
        reconciler.observe("reply", TextForm::Delta("你")),
        Ok("你".into())
    );
    assert_eq!(
        reconciler.observe("reply", TextForm::Cumulative("你好")),
        Ok("好".into())
    );
    assert_eq!(
        reconciler.observe("reply", TextForm::Cumulative("你好")),
        Ok(String::new())
    );
    assert_eq!(
        reconciler.observe("reply", TextForm::Cumulative("你")),
        Ok(String::new())
    );
    assert_eq!(
        reconciler.observe("reply", TextForm::Cumulative("你好呀")),
        Ok("呀".into())
    );
    assert_eq!(
        reconciler.observe("reply", TextForm::Cumulative("另一个")),
        Err("native_text_snapshot_diverged")
    );
}

#[test]
fn adapter_sdk_closes_lifecycle_prefix_and_keeps_first_failure() {
    let mut reducer = TransitionReducer::default();
    let stages = reducer.advance(LifecycleStage::Responding);
    assert_eq!(stages.len(), 4);
    assert!(matches!(
        stages[0],
        Transition::Lifecycle(LifecycleStage::Submitted)
    ));
    assert!(matches!(
        stages[3],
        Transition::Lifecycle(LifecycleStage::Responding)
    ));
    assert!(reducer.fail("native", "turn", "first").is_some());
    assert!(reducer.fail("observer", "observe", "later").is_none());
    assert!(reducer.advance(LifecycleStage::Completed).is_empty());
}

#[test]
fn adapter_sdk_rejects_failure_after_terminal_completion() {
    let mut reducer = TransitionReducer::default();
    assert_eq!(reducer.advance(LifecycleStage::Completed).len(), 5);
    assert!(
        reducer
            .fail("late_transport_failure", "observer/read", "late failure")
            .is_none()
    );
}

/// Two synthetic registrations. The claim is about how the SDK reads a set it
/// was handed, so the fixture must carry no Agent's name.
static FIXTURE_REGISTRATIONS: [ParserRegistration; 2] = [
    ParserRegistration::new(
        AdapterContract::new("fixture-alpha", "fixture-lf-ndjson"),
        |outcome| {
            if outcome.failure.is_some() {
                vec![Transition::Failed {
                    code: "fixture".to_owned(),
                    stage: "fixture".to_owned(),
                    message: "fixture".to_owned(),
                }]
            } else {
                vec![Transition::Text {
                    unit_id: "reply".to_owned(),
                    text: outcome.output.to_owned(),
                }]
            }
        },
        |request| !request.session_id.is_empty() && request.location.is_none(),
    ),
    ParserRegistration::unanswered(AdapterContract::new("fixture-beta", "fixture-jsonrpc")),
];

fn fixture_registrations() -> &'static [ParserRegistration] {
    &FIXTURE_REGISTRATIONS
}

fn fixture_set() -> AdapterParserSet {
    AdapterParserSet {
        registrations: fixture_registrations,
        ..AdapterParserSet::unavailable()
    }
}

/// A registration that declares an Agent's adapter declaration but no answer
/// to the two protocol-agnostic queries answers fail-closed, never with the
/// answer of a different registration.
#[test]
fn parser_registration_answers_only_for_the_agent_it_registers() {
    let set = fixture_set();
    assert_eq!(set.registered_ids(), vec!["fixture-alpha", "fixture-beta"]);
    assert_eq!(
        set.contract("fixture-beta")
            .map(|contract| contract.framing),
        Some("fixture-jsonrpc")
    );
    assert!(set.contract("fixture-gamma").is_none());
    assert_eq!(
        set.framing("fixture-gamma"),
        Err("no registered contract for adapter fixture-gamma".to_owned())
    );

    let outcome = crate::port::ExecutionOutcome {
        output: "answer",
        failure: None,
    };
    assert_eq!(
        set.execution_transitions("fixture-alpha", &outcome),
        Some(vec![Transition::Text {
            unit_id: "reply".to_owned(),
            text: "answer".to_owned(),
        }])
    );
    assert_eq!(
        set.execution_transitions("fixture-beta", &outcome),
        Some(Vec::new())
    );
    assert_eq!(set.execution_transitions("fixture-gamma", &outcome), None);

    let request = crate::port::DurableIdentityRequest {
        session_id: "fixture-session",
        location: None,
    };
    assert_eq!(set.valid_identity("fixture-alpha", &request), Some(true));
    assert_eq!(set.valid_identity("fixture-beta", &request), Some(false));
    assert_eq!(set.valid_identity("fixture-gamma", &request), None);

    let located = crate::port::DurableIdentityRequest {
        session_id: "fixture-session",
        location: Some(std::path::Path::new("/fixture/rollout.jsonl")),
    };
    assert_eq!(set.valid_identity("fixture-alpha", &located), Some(false));
}
