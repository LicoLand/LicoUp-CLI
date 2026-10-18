//! Synthetic fixtures for the remote work control entry.
//!
//! Every fixture here is local and in-memory: a synthetic work owner with a
//! small target tree, synthetic endpoint identities and synthetic request
//! identities. Nothing authenticates a peer, terminates a process or touches a
//! durable store, and the "owner" records what it was asked so a test can prove
//! that a duplicate or refused control never reached it twice.

use std::collections::{BTreeMap, BTreeSet};

use super::authority::{ControlAuthority, EndpointIdentity, VerifiedIngress};
use super::intent::{RedactedDiagnostics, RemoteWorkIntent, StopScope, WorkOwner, WorkTarget};
use super::ledger::{ControlRequest, LedgerRefusal, RemoteControlLedger, RequestId, RequestState};
use super::owner::{LocalWorkOwner, OwnerDisposition, OwnerFailure, OwnerReport, WorkObservation};
use super::{ControlOutcome, ControlRefusal, LateResult, RemoteWorkControl};

/// One question the fixture owner was asked.
#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnerCall {
    Stop {
        target: WorkTarget,
        scope: StopScope,
    },
    Force {
        target: WorkTarget,
        diagnostics: RedactedDiagnostics,
    },
}

/// A synthetic work owner: a set of active targets, an owned-child tree, and a
/// record of every stop it was asked to perform.
#[derive(Default)]
struct FixtureOwner {
    active: BTreeSet<WorkTarget>,
    children: BTreeMap<WorkTarget, Vec<WorkTarget>>,
    calls: Vec<OwnerCall>,
    query_failure: Option<OwnerFailure>,
    effect_failure: Option<OwnerFailure>,
    refusal: Option<String>,
    overreach: Vec<WorkTarget>,
}

impl FixtureOwner {
    fn with_targets(targets: &[WorkTarget]) -> Self {
        Self {
            active: targets.iter().cloned().collect(),
            ..Self::default()
        }
    }

    fn owning_children(mut self, parent: &WorkTarget, children: &[WorkTarget]) -> Self {
        self.active.insert(parent.clone());
        self.children.insert(parent.clone(), children.to_vec());
        for child in children {
            self.active.insert(child.clone());
        }
        self
    }

    /// Every question the owner answers fails this way: the owner cannot even
    /// resolve the target.
    fn failing_queries(mut self, failure: OwnerFailure) -> Self {
        self.query_failure = Some(failure);
        self
    }

    /// Only the effect calls fail; the target still resolves.
    fn failing_effects(mut self, failure: OwnerFailure) -> Self {
        self.effect_failure = Some(failure);
        self
    }

    fn refusing(mut self, reason: &str) -> Self {
        self.refusal = Some(reason.to_owned());
        self
    }

    fn overreaching(mut self, targets: &[WorkTarget]) -> Self {
        self.overreach = targets.to_vec();
        self
    }

    fn is_active(&self, target: &WorkTarget) -> bool {
        self.active.contains(target)
    }

    fn stop_calls(&self) -> usize {
        self.calls
            .iter()
            .filter(|call| matches!(call, OwnerCall::Stop { .. }))
            .count()
    }

    fn force_calls(&self) -> usize {
        self.calls
            .iter()
            .filter(|call| matches!(call, OwnerCall::Force { .. }))
            .count()
    }

    fn last_force_diagnostics(&self) -> Option<&RedactedDiagnostics> {
        self.calls.iter().rev().find_map(|call| match call {
            OwnerCall::Force { diagnostics, .. } => Some(diagnostics),
            OwnerCall::Stop { .. } => None,
        })
    }

    fn subtree(&self, target: &WorkTarget) -> Vec<WorkTarget> {
        let mut selected = vec![target.clone()];
        let mut frontier = vec![target.clone()];
        while let Some(current) = frontier.pop() {
            if let Some(children) = self.children.get(&current) {
                for child in children {
                    if selected.contains(child) {
                        continue;
                    }
                    selected.push(child.clone());
                    frontier.push(child.clone());
                }
            }
        }
        selected.sort();
        selected
    }

    fn perform(&mut self, target: &WorkTarget, scope: &StopScope) -> OwnerReport {
        if !self.active.contains(target) {
            return OwnerReport::not_owned();
        }
        if let Some(reason) = &self.refusal {
            return OwnerReport::new(
                OwnerDisposition::Refused {
                    reason: reason.clone(),
                },
                Vec::new(),
            );
        }
        let mut affected = match scope {
            StopScope::Subtree => self.subtree(target),
            StopScope::OwnedChild(child) => vec![child.clone()],
        };
        affected.extend(self.overreach.iter().cloned());
        affected.sort();
        affected.dedup();
        for item in &affected {
            self.active.remove(item);
        }
        OwnerReport::accepted(affected)
    }
}

impl LocalWorkOwner for FixtureOwner {
    fn owns(&self, target: &WorkTarget) -> Result<bool, OwnerFailure> {
        if let Some(failure) = self.query_failure {
            return Err(failure);
        }
        Ok(self.active.contains(target))
    }

    fn observe(&mut self, target: &WorkTarget) -> Result<WorkObservation, OwnerFailure> {
        if let Some(failure) = self.query_failure {
            return Err(failure);
        }
        let children = self.children.get(target).cloned().unwrap_or_default();
        Ok(WorkObservation::new(
            target.clone(),
            self.active.contains(target),
            children,
        ))
    }

    fn request_stop(
        &mut self,
        target: &WorkTarget,
        scope: StopScope,
    ) -> Result<OwnerReport, OwnerFailure> {
        self.calls.push(OwnerCall::Stop {
            target: target.clone(),
            scope: scope.clone(),
        });
        if let Some(failure) = self.effect_failure {
            return Err(failure);
        }
        Ok(self.perform(target, &scope))
    }

    fn force_stop(
        &mut self,
        target: &WorkTarget,
        diagnostics: &RedactedDiagnostics,
    ) -> Result<OwnerReport, OwnerFailure> {
        self.calls.push(OwnerCall::Force {
            target: target.clone(),
            diagnostics: diagnostics.clone(),
        });
        if let Some(failure) = self.effect_failure {
            return Err(failure);
        }
        Ok(self.perform(target, &StopScope::Subtree))
    }
}

fn target(scope: &str) -> WorkTarget {
    WorkTarget::new(WorkOwner::WorkflowRun, scope)
}

fn child_target(scope: &str) -> WorkTarget {
    WorkTarget::new(WorkOwner::SubagentClaim, scope)
}

fn requester(name: &str) -> EndpointIdentity {
    EndpointIdentity::new(name)
}

fn ingress(name: &str) -> VerifiedIngress {
    VerifiedIngress::verified(
        requester(name),
        super::authority::ReplayIdentity::new("replay-1"),
    )
}

fn stop_request(id: &str, scope: &str) -> ControlRequest {
    ControlRequest::new(
        RequestId::new(id),
        RemoteWorkIntent::Stop {
            target: target(scope),
        },
    )
}

fn control(owner: FixtureOwner) -> RemoteWorkControl<FixtureOwner> {
    let mut control = RemoteWorkControl::new(owner);
    control.grant(&requester("endpoint-b"), ControlAuthority::ForceStop);
    control
}

#[test]
fn a_duplicate_control_is_answered_from_the_record_and_asked_once() {
    let owner = FixtureOwner::with_targets(&[target("run-1")]);
    let mut control = control(owner);

    let first = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the requester holds stop authority");
    assert_eq!(first.state(), RequestState::Accepted);
    assert_eq!(first.affected(), &[target("run-1")]);

    let second = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("a re-delivery is answered, not refused");
    assert_eq!(
        second,
        ControlOutcome::AlreadyAnswered {
            request: RequestId::new("request-1"),
            state: RequestState::Accepted,
            affected: vec![target("run-1")],
        }
    );
    assert_eq!(
        control.owner().stop_calls(),
        1,
        "the owner is asked exactly once for one request identity"
    );
    assert_eq!(control.ledger().generation(), 1);
}

#[test]
fn an_unauthorized_or_unverified_control_leaves_no_record_and_no_effect() {
    let owner = FixtureOwner::with_targets(&[target("run-1")]);
    let mut control = RemoteWorkControl::new(owner);

    assert_eq!(
        control.handle(stop_request("request-1", "run-1"), &ingress("endpoint-b")),
        Err(ControlRefusal::Ungranted {
            requester: "endpoint-b".to_owned(),
            required: ControlAuthority::Stop,
            held: ControlAuthority::None,
        })
    );
    assert_eq!(
        control.handle(
            stop_request("request-2", "run-1"),
            &VerifiedIngress::unverified(
                requester("endpoint-b"),
                super::authority::ReplayIdentity::new("replay-1")
            )
        ),
        Err(ControlRefusal::UnverifiedIngress)
    );

    assert!(control.ledger().admitted().is_empty());
    assert_eq!(control.owner().stop_calls(), 0);
    assert!(control.owner().is_active(&target("run-1")));
}

#[test]
fn an_unverified_ingress_is_refused_even_for_a_granted_requester() {
    let mut control = control(FixtureOwner::with_targets(&[target("run-1")]));

    assert_eq!(
        control.handle(
            stop_request("request-1", "run-1"),
            &VerifiedIngress::unverified(
                requester("endpoint-b"),
                super::authority::ReplayIdentity::new("replay-1")
            )
        ),
        Err(ControlRefusal::UnverifiedIngress),
        "a grant is not a substitute for the verified path"
    );
}

#[test]
fn a_stop_selects_the_subtree_and_leaves_unrelated_work_active() {
    let parent = target("run-1");
    let owned = [child_target("claim-1"), child_target("claim-2")];
    let unrelated = target("run-2");
    let owner = FixtureOwner::with_targets(&[unrelated.clone()]).owning_children(&parent, &owned);
    let mut control = control(owner);

    let outcome = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the stop is admitted");

    assert_eq!(
        outcome.affected(),
        &[
            target("run-1"),
            child_target("claim-1"),
            child_target("claim-2")
        ]
    );
    assert!(!control.owner().is_active(&target("run-1")));
    assert!(!control.owner().is_active(&child_target("claim-1")));
    assert!(
        control.owner().is_active(&unrelated),
        "work outside the selected subtree stays active"
    );
}

#[test]
fn an_owned_child_stop_needs_the_named_parent_to_own_it() {
    let parent = target("run-1");
    let owned = child_target("claim-1");
    let stranger = child_target("claim-9");
    let owner = FixtureOwner::with_targets(&[]).owning_children(&parent, &[owned.clone()]);
    let mut control = control(owner);

    assert_eq!(
        control.handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::StopOwnedChild {
                    parent: parent.clone(),
                    child: stranger.clone(),
                }
            ),
            &ingress("endpoint-b")
        ),
        Err(ControlRefusal::NotOwnedChild {
            parent: parent.clone(),
            child: stranger.clone(),
        })
    );
    assert_eq!(control.owner().stop_calls(), 0);
    assert!(control.ledger().admitted().is_empty());

    let outcome = control
        .handle(
            ControlRequest::new(
                RequestId::new("request-2"),
                RemoteWorkIntent::StopOwnedChild {
                    parent: parent.clone(),
                    child: owned.clone(),
                },
            ),
            &ingress("endpoint-b"),
        )
        .expect("an owned child is selectable");
    assert_eq!(outcome.affected(), &[owned.clone()]);
    assert!(!control.owner().is_active(&owned));
}

#[test]
fn force_control_requires_owned_scope_explicit_authority_and_local_diagnostics() {
    let target = target("run-1");
    let diagnostics = RedactedDiagnostics::new("correlation-1", "remoteForceStopConfirmed")
        .expect("bounded fields");
    let owner = FixtureOwner::with_targets(&[target.clone()]);
    let mut control = RemoteWorkControl::new(owner);
    control.grant(&requester("endpoint-b"), ControlAuthority::Stop);

    // Explicit authority: an ordinary stop grant is not force authority.
    assert_eq!(
        control.handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::ForceStop {
                    target: target.clone()
                }
            )
            .with_diagnostics(diagnostics.clone()),
            &ingress("endpoint-b")
        ),
        Err(ControlRefusal::Ungranted {
            requester: "endpoint-b".to_owned(),
            required: ControlAuthority::ForceStop,
            held: ControlAuthority::Stop,
        })
    );

    control.grant(&requester("endpoint-b"), ControlAuthority::ForceStop);
    assert_eq!(
        control.handle(
            ControlRequest::new(
                RequestId::new("request-2"),
                RemoteWorkIntent::ForceStop {
                    target: target.clone()
                }
            ),
            &ingress("endpoint-b")
        ),
        Err(ControlRefusal::DiagnosticsRequired {
            request: RequestId::new("request-2"),
        }),
        "force control carries the locally produced diagnostics or it does not happen"
    );
    assert_eq!(control.owner().force_calls(), 0);
    assert!(control.ledger().admitted().is_empty());

    let outcome = control
        .handle(
            ControlRequest::new(
                RequestId::new("request-3"),
                RemoteWorkIntent::ForceStop {
                    target: target.clone(),
                },
            )
            .with_diagnostics(diagnostics.clone()),
            &ingress("endpoint-b"),
        )
        .expect("a verified owned scope with diagnostics is force-controllable");
    assert_eq!(outcome.affected(), &[target.clone()]);
    assert_eq!(control.owner().last_force_diagnostics(), Some(&diagnostics));
    assert!(!control.owner().is_active(&target));
}

#[test]
fn a_force_control_on_a_target_this_host_does_not_own_is_refused_before_admission() {
    let owned = target("run-1");
    let stranger = target("run-9");
    let diagnostics =
        RedactedDiagnostics::new("correlation-1", "remoteForceStopConfirmed").expect("bounded");
    let mut control = control(FixtureOwner::with_targets(&[owned]));

    assert_eq!(
        control.handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::ForceStop {
                    target: stranger.clone()
                }
            )
            .with_diagnostics(diagnostics),
            &ingress("endpoint-b")
        ),
        Err(ControlRefusal::TargetNotOwned { target: stranger })
    );
    assert_eq!(control.owner().force_calls(), 0);
    assert!(control.ledger().admitted().is_empty());
}

#[test]
fn an_owner_that_reaches_outside_the_selected_scope_is_visible_as_unknown() {
    let parent = target("run-1");
    let owned = child_target("claim-1");
    let unrelated = target("run-2");
    let owner = FixtureOwner::with_targets(&[unrelated.clone()])
        .owning_children(&parent, &[owned.clone()])
        .overreaching(&[unrelated.clone()]);
    let mut control = control(owner);

    let refusal = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect_err("an owner that reached past its subtree is not a clean stop");

    assert_eq!(
        refusal,
        ControlRefusal::ScopeExceeded {
            request: RequestId::new("request-1"),
            affected: vec![target("run-1"), unrelated.clone(), owned.clone()],
        }
    );
    let entry = control
        .ledger()
        .entry(&RequestId::new("request-1"))
        .expect("the admitted request stays visible");
    assert_eq!(
        entry.state(),
        RequestState::Unknown,
        "the over-reach happened, so the state is unknown rather than accepted"
    );
    assert!(entry.affected().contains(&unrelated));
}

#[test]
fn an_uncertain_owner_leaves_the_effect_unknown_and_a_redrive_asks_nothing_again() {
    let mut control = control(
        FixtureOwner::with_targets(&[target("run-1")]).failing_effects(OwnerFailure::Uncertain),
    );

    assert_eq!(
        control.handle(stop_request("request-1", "run-1"), &ingress("endpoint-b")),
        Err(ControlRefusal::OwnerUnavailable {
            request: RequestId::new("request-1"),
            retryable: true,
        })
    );
    assert_eq!(
        control
            .ledger()
            .entry(&RequestId::new("request-1"))
            .unwrap()
            .state(),
        RequestState::Unknown
    );

    let redrive = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the redrive is answered from the record");
    assert_eq!(
        redrive,
        ControlOutcome::AlreadyAnswered {
            request: RequestId::new("request-1"),
            state: RequestState::Unknown,
            affected: Vec::new(),
        },
        "an uncertain effect is never retried as new work"
    );
    assert_eq!(control.owner().stop_calls(), 1);
}

#[test]
fn a_definite_owner_failure_before_the_effect_is_recorded_as_not_having_happened() {
    let mut control = control(
        FixtureOwner::with_targets(&[target("run-1")]).failing_effects(OwnerFailure::Unavailable),
    );

    assert_eq!(
        control.handle(stop_request("request-1", "run-1"), &ingress("endpoint-b")),
        Err(ControlRefusal::OwnerUnavailable {
            request: RequestId::new("request-1"),
            retryable: false,
        })
    );
    assert_eq!(
        control
            .ledger()
            .entry(&RequestId::new("request-1"))
            .unwrap()
            .state(),
        RequestState::Refused,
        "a definite failure is not an unknown effect"
    );
    assert!(control.owner().is_active(&target("run-1")));
    assert_eq!(control.owner().stop_calls(), 1);
}

#[test]
fn an_owner_that_cannot_resolve_the_target_is_refused_before_admission() {
    let mut control = control(
        FixtureOwner::with_targets(&[target("run-1")]).failing_queries(OwnerFailure::Uncertain),
    );

    assert_eq!(
        control.handle(stop_request("request-1", "run-1"), &ingress("endpoint-b")),
        Err(ControlRefusal::OwnerUnavailable {
            request: RequestId::new("request-1"),
            retryable: true,
        })
    );
    assert!(
        control.ledger().admitted().is_empty(),
        "a request whose selection cannot be resolved leaves no record"
    );
    assert_eq!(control.owner().stop_calls(), 0);
}

#[test]
fn an_owner_refusal_is_recorded_without_claiming_a_stop() {
    let mut control =
        control(FixtureOwner::with_targets(&[target("run-1")]).refusing("owner_policy_refused"));

    let outcome = control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("a refusal is an answer, not a transport failure");

    assert_eq!(
        outcome,
        ControlOutcome::Refused {
            request: RequestId::new("request-1"),
            reason: "owner_policy_refused".to_owned(),
        }
    );
    assert!(!outcome.state().confirms_end());
    assert!(control.owner().is_active(&target("run-1")));
}

#[test]
fn reusing_one_identity_for_other_work_is_refused_and_changes_nothing() {
    let mut control = control(FixtureOwner::with_targets(&[
        target("run-1"),
        target("run-2"),
    ]));
    control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the first request is admitted");

    assert_eq!(
        control.handle(stop_request("request-1", "run-2"), &ingress("endpoint-b")),
        Err(ControlRefusal::ConflictingRequest {
            request: RequestId::new("request-1"),
        })
    );
    assert_eq!(control.owner().stop_calls(), 1);
    assert!(control.owner().is_active(&target("run-2")));
}

#[test]
fn a_late_result_confirms_an_end_without_reissuing_the_request() {
    let mut control = control(FixtureOwner::with_targets(&[target("run-1")]));
    control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the stop is admitted");

    let confirmed = control
        .record_late_result(&RequestId::new("request-1"), LateResult::ObservedEnd)
        .expect("the identity is admitted");
    assert_eq!(confirmed.state(), RequestState::Confirmed);
    assert!(confirmed.state().confirms_end());
    assert_eq!(control.owner().stop_calls(), 1);

    assert_eq!(
        control.record_late_result(&RequestId::new("request-9"), LateResult::ObservedEnd),
        Err(ControlRefusal::RequestNotAdmitted {
            request: RequestId::new("request-9")
        })
    );
}

#[test]
fn an_unconfirmed_end_stays_visible_and_never_becomes_a_confirmed_stop() {
    let mut control = control(FixtureOwner::with_targets(&[target("run-1")]));
    control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the stop is admitted");
    control
        .record_late_result(
            &RequestId::new("request-1"),
            LateResult::EndNotObserved {
                reason: "observationWindowClosed".to_owned(),
            },
        )
        .expect("the identity is admitted");

    assert_eq!(
        control
            .ledger()
            .entry(&RequestId::new("request-1"))
            .unwrap()
            .state(),
        RequestState::Unconfirmed
    );
    assert!(
        !control
            .ledger()
            .entry(&RequestId::new("request-1"))
            .unwrap()
            .state()
            .confirms_end()
    );
    assert_eq!(control.unsettled().len(), 1);

    // A later observed end is accepted, and the earlier unconfirmed state is
    // not what this host publishes afterwards.
    control
        .record_late_result(&RequestId::new("request-1"), LateResult::ObservedEnd)
        .expect("the identity is admitted");
    assert!(control.unsettled().is_empty());
}

#[test]
fn a_restored_ledger_answers_an_answered_identity_and_admits_nothing_new() {
    let mut control = control(FixtureOwner::with_targets(&[target("run-1")]));
    control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the stop is admitted");
    let record = control.ledger().durable_record();
    assert!(record.schema_matches());

    let mut recovered = RemoteWorkControl::restored(
        FixtureOwner::with_targets(&[target("run-1")]),
        RemoteControlLedger::restored(record),
    );
    recovered.grant(&requester("endpoint-b"), ControlAuthority::ForceStop);

    let replay = recovered
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the recovered record answers the re-delivery");
    assert_eq!(
        replay,
        ControlOutcome::AlreadyAnswered {
            request: RequestId::new("request-1"),
            state: RequestState::Accepted,
            affected: vec![target("run-1")],
        }
    );
    assert_eq!(
        recovered.owner().stop_calls(),
        0,
        "recovery does not re-perform an answered request"
    );
}

#[test]
fn an_inspection_is_admitted_answered_and_replayable_but_stops_nothing() {
    let mut control = control(FixtureOwner::with_targets(&[target("run-1")]));

    let outcome = control
        .handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::Inspect {
                    target: target("run-1"),
                },
            ),
            &ingress("endpoint-b"),
        )
        .expect("inspection is granted");
    assert_eq!(outcome.state(), RequestState::Accepted);
    assert!(outcome.observation().unwrap().is_active());
    assert_eq!(control.owner().stop_calls(), 0);

    let replayed = control
        .handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::Inspect {
                    target: target("run-1"),
                },
            ),
            &ingress("endpoint-b"),
        )
        .expect("the second delivery is answered");
    assert_eq!(replayed.state(), RequestState::Accepted);
    assert!(control.owner().is_active(&target("run-1")));
}

#[test]
fn an_inspection_that_observes_the_end_is_a_confirmed_observation() {
    let mut control = control(FixtureOwner::with_targets(&[]));

    let outcome = control
        .handle(
            ControlRequest::new(
                RequestId::new("request-1"),
                RemoteWorkIntent::Inspect {
                    target: target("run-1"),
                },
            ),
            &ingress("endpoint-b"),
        )
        .expect("inspection is granted");
    assert!(!outcome.observation().unwrap().is_active());
    assert_eq!(
        outcome.state(),
        RequestState::Confirmed,
        "only an observation of the end confirms one"
    );
}

#[test]
fn revoking_a_grant_stops_new_controls_but_does_not_erase_what_was_admitted() {
    let mut control = control(FixtureOwner::with_targets(&[
        target("run-1"),
        target("run-2"),
    ]));
    control
        .handle(stop_request("request-1", "run-1"), &ingress("endpoint-b"))
        .expect("the stop is admitted");
    assert!(control.revoke(&requester("endpoint-b")));

    assert_eq!(
        control.handle(stop_request("request-2", "run-2"), &ingress("endpoint-b")),
        Err(ControlRefusal::Ungranted {
            requester: "endpoint-b".to_owned(),
            required: ControlAuthority::Stop,
            held: ControlAuthority::None,
        })
    );
    assert_eq!(
        control
            .ledger()
            .entry(&RequestId::new("request-1"))
            .unwrap()
            .state(),
        RequestState::Accepted,
        "a revocation governs new controls, not the record of an answered one"
    );
}

#[test]
fn the_selection_bound_refuses_rather_than_truncating_a_large_subtree() {
    let parent = target("run-1");
    let children = (0..super::MAX_SELECTED_TARGETS + 4)
        .map(|index| child_target(&format!("claim-{index}")))
        .collect::<Vec<_>>();
    let owner = FixtureOwner::with_targets(&[]).owning_children(&parent, &children);
    let mut control = control(owner);

    assert_eq!(
        control.handle(stop_request("request-1", "run-1"), &ingress("endpoint-b")),
        Err(ControlRefusal::SelectionTooLarge {
            request: RequestId::new("request-1"),
            bound: super::MAX_SELECTED_TARGETS,
        })
    );
    assert_eq!(control.owner().stop_calls(), 0);
}

#[test]
fn every_refusal_has_a_distinct_stable_reason() {
    let reasons = [
        ControlRefusal::UnverifiedIngress.reason(),
        ControlRefusal::Ungranted {
            requester: "endpoint-b".to_owned(),
            required: ControlAuthority::Stop,
            held: ControlAuthority::None,
        }
        .reason(),
        ControlRefusal::DiagnosticsRequired {
            request: RequestId::new("request-1"),
        }
        .reason(),
        ControlRefusal::TargetNotOwned {
            target: target("run-1"),
        }
        .reason(),
        ControlRefusal::NotOwnedChild {
            parent: target("run-1"),
            child: child_target("claim-1"),
        }
        .reason(),
        ControlRefusal::ConflictingRequest {
            request: RequestId::new("request-1"),
        }
        .reason(),
        ControlRefusal::SelectionTooLarge {
            request: RequestId::new("request-1"),
            bound: super::MAX_SELECTED_TARGETS,
        }
        .reason(),
        ControlRefusal::ScopeExceeded {
            request: RequestId::new("request-1"),
            affected: vec![target("run-2")],
        }
        .reason(),
        ControlRefusal::OwnerUnavailable {
            request: RequestId::new("request-1"),
            retryable: true,
        }
        .reason(),
        ControlRefusal::RequestNotAdmitted {
            request: RequestId::new("request-1"),
        }
        .reason(),
    ];
    for reason in reasons {
        assert!(reason.starts_with("endpoint_remote_control_"), "{reason}");
    }
    let mut unique = reasons.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), reasons.len(), "each refusal names one reason");
}

#[test]
fn the_ledger_refusals_are_stable_and_distinct_from_control_refusals() {
    assert_eq!(
        LedgerRefusal::NotAdmitted.reason(),
        "endpoint_remote_control_request_not_admitted"
    );
    assert_eq!(
        LedgerRefusal::IllegalTransition {
            from: RequestState::Confirmed,
            to: RequestState::Unknown,
        }
        .reason(),
        "endpoint_remote_control_state_transition_refused"
    );
}
