//! Component tests for the four economic adapters.
//!
//! These exercise the adapters against the real owners (the usage ledger, the
//! pricing catalog and the evolution seams) with a temporary state root, so the
//! facts under test are durable ones rather than in-memory stand-ins.

use std::path::PathBuf;
use std::sync::Arc;

use super::budget::{
    BudgetAdapter, BudgetAdmission, BudgetConfiguration, EffectBudgetRequest, EffectSettlement,
    EffectStartEvidence, OrphanReleaseRequest, ReleaseOutcome, ReservationAbsent, ReservationState,
    SettlementOutcome, TokenEstimate,
};
use super::observation::{
    CostUnknownReason, EffectKind, EffectObservation, EffectOutcome, ObservationAdapter,
    ObservedCost, RunLifecycle, RunObservationIdentity,
};
use super::policy::{
    AuthorityContextState, DefaultApplication, EvidenceClaim, MINIMUM_HISTORY_SAMPLES, OptionKey,
    PolicyAdapter, PolicyHistory, PolicyRequest, RevocationReceipt, SuggestionBasis,
    SuggestionOutcome,
};
use super::source::{
    CostProvenance, ImportedUsageMarker, NumberOrigin, ProvenanceUnknownReason, RecordedAccuracy,
    SettleableUsage, SourceAdapter,
};
use super::{AdapterError, AdapterName, CostUsage, FactState};
use crate::domain::workflow_runtime::evolution::{
    AdoptedPlanningDefaultSeam, AgentModelOptionSeam, DefaultEvolutionStrategyPort,
    EvolutionStrategyPort, PlanningScopeSeam, StrategySourceSeam, StrategySuggestion,
};
use crate::domain::workflow_store::StrategyAuthorization;

/// A ranking seam that declines, to exercise the adapter's own honest state.
struct DecliningStrategyPort;

impl EvolutionStrategyPort for DecliningStrategyPort {
    fn suggest_strategy(
        &self,
        _scope: &PlanningScopeSeam,
        _candidates: &[AgentModelOptionSeam],
        _explicit_user_choice: Option<&AgentModelOptionSeam>,
    ) -> Option<StrategySuggestion> {
        None
    }
}

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "licoup-evolution-adapters-{}",
        uuid::Uuid::new_v4()
    ))
}

fn usage(prompt: u64, cached: u64, completion: u64) -> CostUsage {
    CostUsage::checked(prompt, cached, completion).expect("checked usage sample")
}

fn option(agent_id: &str, model_id: &str, thinking: &str) -> AgentModelOptionSeam {
    AgentModelOptionSeam {
        agent_id: agent_id.into(),
        model_id: model_id.into(),
        thinking: thinking.into(),
    }
}

fn observation_of(
    run_id: &str,
    effect_id: &str,
    outcome: EffectOutcome,
    cost: ObservedCost,
) -> EffectObservation {
    EffectObservation {
        run_id: run_id.into(),
        effect_id: effect_id.into(),
        state_id: "state:actor".into(),
        kind: EffectKind::Actor,
        membership_id: Some("membership:worker".into()),
        agent_id: Some("agent-a".into()),
        model: Some("model-a".into()),
        attempt: 1,
        outcome,
        cost,
    }
}

fn register(adapter: &ObservationAdapter, run_id: &str) {
    adapter
        .register_run(&RunObservationIdentity {
            run_id: run_id.into(),
            revision_digest: "revision:1".into(),
            conversation_id: Some("conversation:1".into()),
            assistant_membership_id: Some("membership:assistant".into()),
            lifecycle: RunLifecycle::Running,
        })
        .expect("the ledger owns the run identity");
}

fn budget_request(effect_id: &str, estimate: Option<u64>) -> EffectBudgetRequest {
    EffectBudgetRequest {
        effect_id: effect_id.into(),
        run_id: Some("run:one".into()),
        command_id: Some(format!("command:{effect_id}")),
        estimate: estimate.map(|total_tokens| TokenEstimate { total_tokens }),
        chargeable: true,
        budget: Some(BudgetConfiguration::limit("pool:one", 100)),
    }
}

/// The pool's available tokens, read back through the reconciliation report.
fn available(budget: &BudgetAdapter) -> Option<u64> {
    let report = budget.reconcile(None).expect("admission report");
    report.pools.first().and_then(|pool| pool.available_tokens)
}

fn resolved_authority() -> AuthorityContextState {
    AuthorityContextState::from_authorization(&StrategyAuthorization {
        definition_digest: "revision:1".into(),
        semantics_digest: "semantics:1".into(),
        binding_digest: "binding:1".into(),
        authorization_digest: "authorization:1".into(),
        revision: 3,
        active: true,
    })
}

#[test]
fn failed_cancelled_and_unknown_effects_are_observed_and_unknown_is_never_zero() {
    let root = temp_root();
    let observation = ObservationAdapter::new(&root);
    register(&observation, "run:obs");

    let recorded = observation
        .record(&observation_of(
            "run:obs",
            "effect:ok",
            EffectOutcome::Succeeded,
            ObservedCost::Known {
                usage: usage(6, 2, 4),
            },
        ))
        .expect("a successful effect is observed");
    assert_eq!(recorded.state, FactState::Present);

    let failed = observation
        .record(&observation_of(
            "run:obs",
            "effect:failed",
            EffectOutcome::Failed,
            ObservedCost::Unknown {
                reason: CostUnknownReason::NotReported,
            },
        ))
        .expect("a failed effect is observed too: it still spent what it spent");
    assert_eq!(failed.recorded.usage(), None);
    assert_eq!(failed.state, FactState::Unknown);

    observation
        .record(&observation_of(
            "run:obs",
            "effect:cancelled",
            EffectOutcome::Cancelled,
            ObservedCost::Known {
                usage: usage(4, 0, 2),
            },
        ))
        .expect("a cancelled effect is observed");
    observation
        .record(&observation_of(
            "run:obs",
            "effect:in-doubt",
            EffectOutcome::Unknown,
            ObservedCost::Unknown {
                reason: CostUnknownReason::InDoubt,
            },
        ))
        .expect("an in-doubt effect is observed");

    let facts = observation.meter_run("run:obs").expect("metering facts");
    assert_eq!(facts.state, FactState::Present);
    assert_eq!(facts.effects.len(), 4);
    assert_eq!(facts.totals.known_effects, 2);
    assert_eq!(facts.totals.unknown_effects, 2);
    // The two readable effects sum to 16, and that number is only a lower
    // bound: the run's cost stays unknown until the other two report.
    assert_eq!(facts.totals.read_tokens(), 16);
    let total = facts.total_cost();
    assert_eq!(total.usage(), None);
    assert_eq!(
        total,
        ObservedCost::Unknown {
            reason: CostUnknownReason::SomeEffectsUnknown
        }
    );

    let failed_fact = facts.effect("effect:failed").expect("recorded failure");
    assert_eq!(failed_fact.outcome, Some(EffectOutcome::Failed));
    assert_eq!(
        failed_fact.cost,
        ObservedCost::Unknown {
            reason: CostUnknownReason::NotReported
        }
    );
    let in_doubt = facts.effect("effect:in-doubt").expect("recorded in-doubt");
    assert_eq!(in_doubt.outcome, Some(EffectOutcome::Unknown));
    assert_eq!(
        in_doubt.cost,
        ObservedCost::Unknown {
            reason: CostUnknownReason::InDoubt
        }
    );
    let cancelled = facts.effect("effect:cancelled").expect("recorded cancel");
    assert_eq!(cancelled.cost.usage(), Some(usage(4, 0, 2)));

    // A run nobody recorded is not a run that spent zero.
    let absent = observation.meter_run("run:absent").expect("empty read");
    assert_eq!(absent.state, FactState::NotRecorded);
    assert_eq!(
        absent.total_cost(),
        ObservedCost::Unknown {
            reason: CostUnknownReason::NotRecorded
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn metering_facts_stay_separate_from_the_optional_statistics_projection() {
    let root = temp_root();
    let observation = ObservationAdapter::new(&root);
    register(&observation, "run:stats");

    let mut retried = observation_of(
        "run:stats",
        "effect:rework",
        EffectOutcome::Succeeded,
        ObservedCost::Known {
            usage: usage(3, 0, 1),
        },
    );
    retried.attempt = 2;
    retried.model = Some("model-b".into());
    observation
        .record(&retried)
        .expect("second attempt recorded");
    observation
        .record(&observation_of(
            "run:stats",
            "effect:first",
            EffectOutcome::Succeeded,
            ObservedCost::Known {
                usage: usage(5, 0, 5),
            },
        ))
        .expect("first attempt recorded");

    let statistics = observation
        .statistics("run:stats")
        .expect("the presentation projection");
    assert_eq!(statistics.total_effects, 2);
    assert_eq!(statistics.rework_effects, 1);
    assert_eq!(statistics.succeeded_effects, 2);
    assert_eq!(
        statistics
            .by_model
            .get("model-a")
            .map(|totals| totals.read_tokens()),
        Some(10)
    );

    let facts = observation.meter_run("run:stats").expect("metering facts");
    assert_eq!(facts.rework_effects, 1);

    // The projection is optional: when the owner cannot be read it is simply
    // absent, while the base facts report the failure instead of guessing.
    let blocked_root = temp_root();
    std::fs::create_dir_all(&blocked_root).expect("temporary root");
    std::fs::write(blocked_root.join("agent-usage"), b"blocked").expect("block the ledger path");
    let blocked = ObservationAdapter::new(&blocked_root);
    assert!(blocked.meter_run("run:stats").is_err());
    assert!(blocked.statistics("run:stats").is_none());
    let error = blocked
        .register_run(&RunObservationIdentity {
            run_id: "run:stats".into(),
            revision_digest: "revision:1".into(),
            conversation_id: None,
            assistant_membership_id: None,
            lifecycle: RunLifecycle::Running,
        })
        .expect_err("the ledger cannot be opened");
    assert_eq!(error.adapter, AdapterName::Observation);
    assert_eq!(error.code, "usage_ledger_unavailable");
    assert!(error.retryable);

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(blocked_root);
}

#[test]
fn budget_is_reserved_and_settled_by_effect_id() {
    let root = temp_root();
    let budget = BudgetAdapter::new(&root);

    let admission = budget
        .reserve(&budget_request("effect:one", Some(40)))
        .expect("admitted");
    let reservation = admission.reservation().expect("a reservation").clone();
    assert_eq!(admission.state(), FactState::Present);
    assert!(admission.work_proceeds());
    assert_eq!(reservation.effect_id, "effect:one");
    assert_eq!(reservation.state, ReservationState::Reserved);
    assert_eq!(reservation.reserved_tokens, 40);
    assert_eq!(reservation.available_tokens, Some(60));
    assert!(!reservation.reused);

    // A retry of the same effect id reuses its reservation instead of holding
    // the amount twice.
    let retry = budget
        .reserve(&budget_request("effect:one", Some(40)))
        .expect("reused");
    let reused = retry.reservation().expect("the same reservation");
    assert!(reused.reused);
    assert_eq!(reused.reserved_tokens, 40);
    assert_eq!(available(&budget), Some(60));

    let evidence = SettleableUsage::from_provider_report(usage(20, 5, 5), Some("model-a".into()));
    let settled = budget
        .settle(
            &admission,
            &EffectSettlement::reported(EffectOutcome::Succeeded, evidence),
        )
        .expect("settled");
    assert!(!settled.reconciliation_required());
    assert_eq!(settled.state(), FactState::Present);
    let receipt = settled.receipt().expect("a receipt");
    assert_eq!(receipt.actual_tokens, Some(25));
    assert_eq!(receipt.overage_tokens, 0);
    assert!(!receipt.late);
    assert_eq!(receipt.available_tokens, Some(75));

    // Retrying the reserve for an effect that already settled reports the
    // concluded identity; it is not a budget denial and no gate is invented.
    let concluded = budget
        .reserve(&budget_request("effect:one", Some(40)))
        .expect("the concluded reservation is an answer, not a failure");
    assert!(matches!(
        concluded,
        BudgetAdmission::AlreadyConcluded {
            state: ReservationState::Settled,
            ..
        }
    ));
    assert_eq!(concluded.state(), FactState::Present);
    assert!(!concluded.work_proceeds());
    assert!(concluded.reservation().is_none());
    assert_eq!(available(&budget), Some(75));

    // Settling that identity again reaches the ledger's own idempotent answer
    // instead of a fabricated absence.
    let idempotent = budget
        .settle(
            &concluded,
            &EffectSettlement::reported(
                EffectOutcome::Succeeded,
                SettleableUsage::from_provider_report(usage(20, 5, 5), Some("model-a".into())),
            ),
        )
        .expect("the settled identity is idempotent");
    assert!(matches!(idempotent, SettlementOutcome::Settled { .. }));
    assert_eq!(
        idempotent
            .receipt()
            .and_then(|receipt| receipt.actual_tokens),
        Some(25)
    );

    // An effect that holds nothing settles to an honest absent state: a
    // non-chargeable read is admitted without a reservation.
    let mut free_read_request = budget_request("effect:read", None);
    free_read_request.chargeable = false;
    let free_read = budget
        .reserve(&free_read_request)
        .expect("admitted as a free read");
    assert!(matches!(free_read, BudgetAdmission::FreeRead { .. }));
    let unreserved = budget
        .settle(
            &free_read,
            &EffectSettlement::unknown(EffectOutcome::Failed),
        )
        .expect("no reservation to settle");
    assert!(matches!(
        unreserved,
        SettlementOutcome::NotReserved {
            absence: ReservationAbsent::NeverReserved,
            ..
        }
    ));
    assert_eq!(unreserved.state(), FactState::NotRecorded);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn without_a_configured_budget_no_gate_is_invented_and_cost_is_still_metered() {
    let root = temp_root();
    let budget = BudgetAdapter::new(&root);
    let observation = ObservationAdapter::new(&root);

    let mut request = budget_request("effect:free", Some(40));
    request.budget = None;
    let admission = budget.reserve(&request).expect("nothing to reserve");
    assert!(matches!(admission, BudgetAdmission::NotConfigured { .. }));
    assert_eq!(admission.state(), FactState::NotConfigured);
    assert!(admission.work_proceeds());
    assert_eq!(admission.effect_id(), "");
    assert!(admission.reservation().is_none());

    let report = budget.reconcile(None).expect("admission report");
    assert_eq!(report.state, FactState::NotConfigured);
    assert!(!report.budgets_configured());
    assert!(report.pools.is_empty());
    assert!(report.orphan_candidates.is_empty());
    assert!(!report.stops_new_dispatch);

    let settlement = budget
        .settle(
            &admission,
            &EffectSettlement::unknown(EffectOutcome::Failed),
        )
        .expect("nothing to settle");
    assert!(matches!(
        settlement,
        SettlementOutcome::NotConfigured { .. }
    ));
    assert_eq!(settlement.state(), FactState::NotConfigured);

    // The absence of a budget does not stop the cost from being observed.
    register(&observation, "run:free");
    observation
        .record(&observation_of(
            "run:free",
            "effect:free",
            EffectOutcome::Succeeded,
            ObservedCost::Known {
                usage: usage(8, 0, 4),
            },
        ))
        .expect("observed without a budget");
    let facts = observation.meter_run("run:free").expect("metering facts");
    assert_eq!(facts.totals.read_tokens(), 12);
    assert!(!budget.reconcile(None).expect("report").budgets_configured());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_orphan_reservation_is_released_idempotently_while_a_started_effect_keeps_its_budget() {
    let root = temp_root();
    let budget = BudgetAdapter::new(&root);

    budget
        .reserve(&budget_request("effect:orphan", Some(30)))
        .expect("reserved before the host lost the run");
    let report = budget.reconcile(Some("run:one")).expect("reconcile");
    assert!(report.budgets_configured());
    assert_eq!(report.in_flight_count, 1);
    assert_eq!(report.unknown_in_flight_count, 0);
    assert_eq!(report.orphan_candidates.len(), 1);
    assert_eq!(report.orphan_candidates[0].effect_id, "effect:orphan");
    assert_eq!(report.orphan_candidates[0].reserved_tokens, 30);
    assert_eq!(available(&budget), Some(70));

    // Without evidence that the effect never ran, the amount stays held.
    let refused = budget
        .release_orphan(&OrphanReleaseRequest {
            effect_id: "effect:orphan".into(),
            budget_id: Some("pool:one".into()),
            evidence: EffectStartEvidence::PossiblyStarted,
        })
        .expect("refusal is an answer, not a failure");
    assert!(matches!(refused, ReleaseOutcome::RefusedEarly { .. }));
    assert_eq!(refused.state(), FactState::RefusedEarly);
    assert_eq!(available(&budget), Some(70));

    let released = budget
        .release_orphan(&OrphanReleaseRequest {
            effect_id: "effect:orphan".into(),
            budget_id: Some("pool:one".into()),
            evidence: EffectStartEvidence::NeverStarted,
        })
        .expect("the orphan is given back");
    assert!(matches!(released, ReleaseOutcome::Released { .. }));
    assert_eq!(available(&budget), Some(100));

    let again = budget
        .release_orphan(&OrphanReleaseRequest {
            effect_id: "effect:orphan".into(),
            budget_id: Some("pool:one".into()),
            evidence: EffectStartEvidence::NeverStarted,
        })
        .expect("releasing twice is safe");
    let repeated = match again {
        ReleaseOutcome::Released { receipt } => receipt,
        other => panic!("expected the released state again, got {other:?}"),
    };
    assert_eq!(repeated.reservation_state, ReservationState::Released);
    assert_eq!(repeated.reserved_tokens, 0);
    assert_eq!(available(&budget), Some(100));
    assert_eq!(
        budget
            .reconcile(None)
            .expect("report")
            .pools
            .first()
            .map(|pool| pool.reserved_tokens),
        Some(0)
    );

    // Re-reserving the released identity is reported as concluded, and
    // settling it again answers with the ledger's own already-released fact.
    let concluded_release = budget
        .reserve(&budget_request("effect:orphan", Some(30)))
        .expect("the released identity is an answer");
    assert!(matches!(
        concluded_release,
        BudgetAdmission::AlreadyConcluded {
            state: ReservationState::Released,
            ..
        }
    ));
    assert!(!concluded_release.work_proceeds());
    let after_release = budget
        .settle(
            &concluded_release,
            &EffectSettlement::unknown(EffectOutcome::Failed),
        )
        .expect("the released identity is not settled");
    assert!(matches!(
        after_release,
        SettlementOutcome::NotReserved {
            absence: ReservationAbsent::AlreadyReleased,
            ..
        }
    ));

    // An effect that was started and then reported an unknown outcome keeps its
    // budget: the amount must not be handed back and re-allocated.
    let started = budget
        .reserve(&budget_request("effect:started", Some(30)))
        .expect("admitted");
    let held = budget
        .settle(&started, &EffectSettlement::unknown(EffectOutcome::Failed))
        .expect("recorded as unknown");
    assert!(held.reconciliation_required());
    assert_eq!(held.state(), FactState::Unknown);
    let receipt = held.receipt().expect("a receipt");
    assert_eq!(receipt.actual_tokens, None);
    assert_eq!(receipt.settlement_status.as_deref(), Some("failed"));
    assert_eq!(available(&budget), Some(70));

    // Even "it never started" cannot free an effect whose outcome is in doubt:
    // the durable state outranks the caller's claim.
    let late_claim = budget
        .release_orphan(&OrphanReleaseRequest {
            effect_id: "effect:started".into(),
            budget_id: Some("pool:one".into()),
            evidence: EffectStartEvidence::NeverStarted,
        })
        .expect("refusal is an answer");
    assert!(matches!(late_claim, ReleaseOutcome::RefusedEarly { .. }));
    assert_eq!(available(&budget), Some(70));

    // A late report charges the real number and records the overage, which is
    // the cost fact the policy adapter reads later.
    let late = budget
        .settle(
            &started,
            &EffectSettlement::reported(
                EffectOutcome::Succeeded,
                SettleableUsage::from_provider_report(usage(20, 0, 20), None),
            ),
        )
        .expect("late settlement");
    let late_receipt = late.receipt().expect("a receipt");
    assert!(late_receipt.late);
    assert_eq!(late_receipt.actual_tokens, Some(40));
    assert_eq!(late_receipt.overage_tokens, 10);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_imported_usage_marker_cannot_authorize_budget_settlement() {
    let source = SourceAdapter::new();
    let marker = ImportedUsageMarker {
        source_id: "import:agent-history".into(),
        revision: "r1".into(),
        records: 42,
    };
    assert!(!marker.authorizes_budget_settlement());

    let imported = source.classify(
        RecordedAccuracy::Exact,
        &NumberOrigin::ImportedUsage {
            marker: marker.clone(),
        },
    );
    assert!(matches!(imported, CostProvenance::ImportedUsage { .. }));
    assert_eq!(imported.state(), FactState::Unknown);
    assert!(!imported.authorizes_budget_settlement());
    let refusal = imported
        .settlement_evidence(usage(10, 0, 5))
        .expect_err("an import is not approval to settle");
    assert_eq!(refusal.adapter, AdapterName::Source);
    assert_eq!(refusal.code, "source_not_settleable_imported_usage");
    assert_eq!(refusal.owner_code, None);
    assert!(!refusal.retryable);
    assert_eq!(refusal.recovery, "settle_from_the_effects_own_outcome");

    // An estimate is an admission input, not a settlement input either.
    let estimate = source.catalog_estimate("deepseek-v4-flash");
    assert!(matches!(
        estimate,
        CostProvenance::EstimatedFromCatalog { .. }
    ));
    assert!(!estimate.authorizes_budget_settlement());
    assert_eq!(
        estimate
            .settlement_evidence(usage(10, 0, 5))
            .expect_err("an estimate cannot charge a reservation")
            .code,
        "source_not_settleable_estimate"
    );

    // The effect's own reported outcome is the one that may.
    let reported = source.classify(
        RecordedAccuracy::Exact,
        &NumberOrigin::ProviderReport {
            model_id: Some("model-a".into()),
        },
    );
    assert!(reported.authorizes_budget_settlement());
    let evidence = reported
        .settlement_evidence(usage(10, 0, 5))
        .expect("provider-reported usage settles");
    assert_eq!(evidence.usage(), usage(10, 0, 5));
    assert_eq!(evidence.ledger_accuracy(), "exact");
    assert_eq!(evidence.model_id(), Some("model-a"));

    // Markers that disagree are not resolved in favour of settlement.
    let contradictory = source.classify(
        RecordedAccuracy::Exact,
        &NumberOrigin::Catalog {
            model_id: "model-a".into(),
        },
    );
    assert_eq!(
        contradictory,
        CostProvenance::Unknown {
            reason: ProvenanceUnknownReason::Unattributed
        }
    );
    assert!(!contradictory.authorizes_budget_settlement());
}

#[test]
fn catalog_rates_are_facts_and_a_missing_route_stays_unknown() {
    let source = SourceAdapter::new();

    let published = source
        .catalog_rate("deepseek-v4-flash")
        .expect("the installed catalog publishes this model");
    assert!(published.input > 0.0);
    assert!(published.output > 0.0);
    assert!(matches!(
        source.catalog_estimate("deepseek-v4-flash"),
        CostProvenance::EstimatedFromCatalog { .. }
    ));

    let absent = source.catalog_estimate("licoup-model-with-no-published-route");
    assert_eq!(
        absent,
        CostProvenance::Unknown {
            reason: ProvenanceUnknownReason::CatalogHasNoRoute
        }
    );
    assert_eq!(absent.state(), FactState::Unknown);
    assert!(!absent.authorizes_budget_settlement());
    assert_eq!(
        absent
            .settlement_evidence(usage(1, 0, 1))
            .expect_err("an unknown origin settles nothing")
            .code,
        "source_not_settleable_unknown"
    );
    assert!(
        source
            .catalog_rate("licoup-model-with-no-published-route")
            .is_none()
    );
}

#[test]
fn adopting_a_default_changes_future_work_and_revoking_restores_it() {
    let policy = PolicyAdapter::new();
    let scope = PlanningScopeSeam {
        task_kind: "workflow-turn".into(),
        configuration: "step1".into(),
    };
    let candidates = vec![
        option("agent-a", "model-a", "low"),
        option("agent-b", "model-b", "high"),
    ];

    // Adoption reaches future selection only; this path holds no in-flight
    // binding it could rewrite.
    let first = policy
        .adopt(
            &AdoptedPlanningDefaultSeam {
                scope: scope.clone(),
                source: StrategySourceSeam {
                    source_id: "source:first".into(),
                    revision: "r1".into(),
                },
                selected_option: option("agent-a", "model-a", "low"),
                revocable: true,
                supersedes_revision: None,
            },
            &candidates,
        )
        .expect("adopted for future work");
    assert_eq!(first.revision, 1);
    assert_eq!(first.applies_to, DefaultApplication::FutureWork);
    assert!(!first.applies_to.applies_to_in_flight());
    assert_eq!(first.applies_to.code(), "future-work");

    let second = policy
        .adopt(
            &AdoptedPlanningDefaultSeam {
                scope: scope.clone(),
                source: StrategySourceSeam {
                    source_id: "source:second".into(),
                    revision: "r2".into(),
                },
                selected_option: option("agent-b", "model-b", "high"),
                revocable: true,
                supersedes_revision: Some("r1".into()),
            },
            &candidates,
        )
        .expect("adopted again");
    assert_eq!(second.revision, 2);
    assert_eq!(
        policy.future_default(&scope),
        Some(option("agent-b", "model-b", "high"))
    );

    let suggestion = match policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: None,
            authority: resolved_authority(),
            history: PolicyHistory::default(),
        })
        .expect("a suggestion")
    {
        SuggestionOutcome::Suggested {
            suggestion,
            basis,
            claim,
            ..
        } => {
            assert_eq!(claim, EvidenceClaim::NotMeasured);
            assert!(!claim.claims_measured_improvement());
            assert_eq!(
                basis,
                SuggestionBasis::AdoptedFutureDefault {
                    source: StrategySourceSeam {
                        source_id: "source:second".into(),
                        revision: "r2".into(),
                    },
                    revision: 2,
                }
            );
            suggestion
        }
        other => panic!("expected an adopted default, got {other:?}"),
    };
    assert_eq!(suggestion.candidate, option("agent-b", "model-b", "high"));
    assert!(suggestion.is_default);
    // A suggestion is advice: it never carries execution permission.
    assert!(!suggestion.has_execution_permission);

    // Revoking restores the default that preceded it, not the catalog fallback.
    let revocation = policy.revoke(&scope, "source:second").expect("revoked");
    assert!(revocation.changed);
    assert_eq!(
        revocation.restored,
        Some(option("agent-a", "model-a", "low"))
    );
    assert_eq!(revocation.revision, 3);
    assert_eq!(revocation.applies_to, DefaultApplication::FutureWork);

    let restoring = policy.revoke(&scope, "source:first").expect("revoked");
    assert!(restoring.changed);
    assert_eq!(restoring.restored, None);
    assert_eq!(policy.future_default(&scope), None);

    // Revoking something that was not adopted changes nothing.
    let no_op: RevocationReceipt = policy
        .revoke(&scope, "source:never-adopted")
        .expect("a no-op, not a failure");
    assert!(!no_op.changed);
    assert_eq!(no_op.revision, restoring.revision);
    assert_eq!(policy.revision(), restoring.revision);
}

#[test]
fn one_shared_owner_is_the_only_future_default_fact() {
    // The counterexample this guards against: while the adapter kept its own
    // adoption table, `adapter.adopt` left the owner's `suggest_strategy`
    // answering the catalog fallback for the same scope — two future-default
    // facts. Both entries must now observe the same owner state.
    let owner = Arc::new(DefaultEvolutionStrategyPort::new());
    let policy = PolicyAdapter::new().with_owner(owner.clone());
    let scope = PlanningScopeSeam {
        task_kind: "workflow-turn".into(),
        configuration: "step1".into(),
    };
    let candidates = vec![
        option("agent-a", "model-a", "low"),
        option("agent-b", "model-b", "high"),
    ];
    let adoption = |source_id: &str, revision: &str, selected: AgentModelOptionSeam| {
        AdoptedPlanningDefaultSeam {
            scope: scope.clone(),
            source: StrategySourceSeam {
                source_id: source_id.into(),
                revision: revision.into(),
            },
            selected_option: selected,
            revocable: true,
            supersedes_revision: None,
        }
    };

    // The adapter adopts: the owner's own entry observes the same default.
    policy
        .adopt(
            &adoption("source:first", "r1", option("agent-b", "model-b", "high")),
            &candidates,
        )
        .expect("adopted");
    let owner_view = owner
        .suggest_strategy(&scope, &candidates, None)
        .expect("the owner ranks");
    assert_eq!(owner_view.candidate, option("agent-b", "model-b", "high"));
    assert!(owner_view.is_default);
    assert_eq!(
        owner_view.ranking_basis,
        "adopted-default-comparable-outcome"
    );
    assert_eq!(
        policy.future_default(&scope),
        Some(option("agent-b", "model-b", "high"))
    );

    // The owner adopts directly: the adapter observes the same default.
    owner.adopt_default(adoption(
        "source:second",
        "r2",
        option("agent-a", "model-a", "low"),
    ));
    assert_eq!(
        policy.future_default(&scope),
        Some(option("agent-a", "model-a", "low"))
    );
    let via_adapter = match policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: None,
            authority: resolved_authority(),
            history: PolicyHistory::default(),
        })
        .expect("a suggestion")
    {
        SuggestionOutcome::Suggested {
            suggestion, basis, ..
        } => {
            assert_eq!(
                basis,
                SuggestionBasis::AdoptedFutureDefault {
                    source: StrategySourceSeam {
                        source_id: "source:second".into(),
                        revision: "r2".into(),
                    },
                    revision: 2,
                }
            );
            suggestion
        }
        other => panic!("expected the owner's default, got {other:?}"),
    };
    assert_eq!(via_adapter.candidate, option("agent-a", "model-a", "low"));

    // Repeating the identical adoption is not a new fact: the owner keeps no
    // duplicate entry and the revision does not move.
    let repeated = policy
        .adopt(
            &adoption("source:second", "r2", option("agent-a", "model-a", "low")),
            &candidates,
        )
        .expect("the same adoption again");
    assert_eq!(repeated.revision, 2);
    assert_eq!(owner.revision(), 2);

    // A revoke that matches no adoption changes nothing, including the
    // revision, and leaves the default in force untouched.
    let no_op = owner.revoke_default(&scope, "source:never-adopted");
    assert!(!no_op.changed);
    assert_eq!(owner.revision(), 2);
    assert_eq!(
        policy.future_default(&scope),
        Some(option("agent-a", "model-a", "low"))
    );

    // The adapter revokes: the owner restores the adoption that preceded it.
    let revocation = policy.revoke(&scope, "source:second").expect("revoked");
    assert!(revocation.changed);
    assert_eq!(
        revocation.restored,
        Some(option("agent-b", "model-b", "high"))
    );
    let owner_view = owner
        .suggest_strategy(&scope, &candidates, None)
        .expect("the owner ranks");
    assert_eq!(owner_view.candidate, option("agent-b", "model-b", "high"));
    assert!(owner_view.is_default);

    // The owner revokes directly: the adapter observes the same state.
    let outcome = owner.revoke_default(&scope, "source:first");
    assert!(outcome.changed);
    assert!(outcome.restored.is_none());
    assert_eq!(policy.future_default(&scope), None);
    let fallback = match policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates,
            user_configuration: None,
            authority: resolved_authority(),
            history: PolicyHistory::default(),
        })
        .expect("a suggestion")
    {
        SuggestionOutcome::Suggested {
            suggestion, basis, ..
        } => {
            assert_eq!(
                basis,
                SuggestionBasis::OwnerRanking {
                    ranking_basis: "catalog-fallback".into()
                }
            );
            suggestion
        }
        other => panic!("expected the catalog fallback, got {other:?}"),
    };
    assert_eq!(fallback.candidate, option("agent-a", "model-a", "low"));
    assert!(!fallback.is_default);
}

#[test]
fn suggestions_come_from_real_history_and_never_claim_a_measured_improvement() {
    let root = temp_root();
    let observation = ObservationAdapter::new(&root);
    register(&observation, "run:history");
    for ordinal in 0..MINIMUM_HISTORY_SAMPLES {
        observation
            .record(&observation_of(
                "run:history",
                &format!("effect:ok-{ordinal}"),
                EffectOutcome::Succeeded,
                ObservedCost::Known {
                    usage: usage(4, 0, 2),
                },
            ))
            .expect("observed");
    }
    let mut failed = observation_of(
        "run:history",
        "effect:other",
        EffectOutcome::Failed,
        ObservedCost::Unknown {
            reason: CostUnknownReason::NotReported,
        },
    );
    failed.agent_id = Some("agent-b".into());
    failed.model = Some("model-b".into());
    failed.attempt = 2;
    observation.record(&failed).expect("observed");

    let facts = observation
        .meter_run("run:history")
        .expect("metering facts");
    let mut history = PolicyHistory::from_metering(&facts);
    assert_eq!(history.samples(), MINIMUM_HISTORY_SAMPLES + 1);
    assert_eq!(history.succeeded, MINIMUM_HISTORY_SAMPLES);
    assert_eq!(history.failed, 1);
    assert_eq!(history.rework, 1);
    assert_eq!(
        history
            .option(&OptionKey::of(&option("agent-a", "model-a", "low")))
            .map(|option| option.succeeded),
        Some(MINIMUM_HISTORY_SAMPLES)
    );
    let late = super::budget::SettlementReceipt {
        effect_id: "effect:other".into(),
        budget_id: "pool:one".into(),
        state: FactState::Present,
        reservation_state: ReservationState::Settled,
        reserved_tokens: 4,
        actual_tokens: Some(8),
        overage_tokens: 4,
        late: true,
        available_tokens: Some(90),
        settlement_status: Some("completed".into()),
    };
    history.observe_settlement(&late);
    assert_eq!(history.late_settlements, 1);
    assert_eq!(history.overage_tokens, 4);

    let policy = PolicyAdapter::new();
    let scope = PlanningScopeSeam {
        task_kind: "workflow-turn".into(),
        configuration: "step1".into(),
    };
    let candidates = vec![
        option("agent-a", "model-a", "low"),
        option("agent-b", "model-b", "high"),
    ];
    let outcome = policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: None,
            authority: resolved_authority(),
            history: history.clone(),
        })
        .expect("a suggestion");
    let suggestion = match &outcome {
        SuggestionOutcome::Suggested {
            suggestion,
            basis,
            claim,
            authority,
        } => {
            assert_eq!(
                *basis,
                SuggestionBasis::History {
                    option_samples: MINIMUM_HISTORY_SAMPLES,
                    required_samples: MINIMUM_HISTORY_SAMPLES,
                }
            );
            assert_eq!(*claim, EvidenceClaim::NotMeasured);
            assert_eq!(*authority, resolved_authority());
            suggestion
        }
        other => panic!("expected a history suggestion, got {other:?}"),
    };
    assert_eq!(suggestion.candidate, option("agent-a", "model-a", "low"));
    assert!(!suggestion.has_execution_permission);
    assert!(!suggestion.is_default);
    let rationale = suggestion.rationale.to_lowercase();
    for claim in [
        "improved",
        "better",
        "faster",
        "significant",
        "superior",
        "outperform",
    ] {
        assert!(
            !rationale.contains(claim),
            "a suggestion must not claim a measured benefit: {rationale}"
        );
    }
    assert!(!outcome.claim().claims_measured_improvement());
    assert_eq!(outcome.claim().code(), "not-measured");

    // The user's own configuration outranks everything the adapter knows.
    let user = option("agent-b", "model-b", "high");
    let configured = policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: Some(user.clone()),
            authority: AuthorityContextState::NotEvaluated,
            history: history.clone(),
        })
        .expect("a suggestion");
    match configured {
        SuggestionOutcome::Suggested {
            suggestion, basis, ..
        } => {
            assert_eq!(suggestion.candidate, user);
            assert_eq!(basis, SuggestionBasis::UserConfiguration);
            assert!(!suggestion.has_execution_permission);
        }
        other => panic!("expected the user's configuration, got {other:?}"),
    }

    // A configuration the installed catalog cannot run is reported, not
    // silently replaced.
    let unavailable = policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: Some(option("agent-z", "model-z", "high")),
            authority: resolved_authority(),
            history: history.clone(),
        })
        .expect("a suggestion");
    assert!(matches!(
        unavailable,
        SuggestionOutcome::UserConfigurationNotAvailable { .. }
    ));
    assert_eq!(unavailable.state(), FactState::Unknown);

    // Nothing the host offers means no suggestion, never an invented model.
    let empty = policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: Vec::new(),
            user_configuration: None,
            authority: resolved_authority(),
            history: history.clone(),
        })
        .expect("a suggestion");
    assert_eq!(empty, SuggestionOutcome::NoCandidateCatalog);
    assert_eq!(empty.state(), FactState::NotConfigured);

    // Too little history to propose from is its own honest state.
    let thin_root = temp_root();
    let thin_observation = ObservationAdapter::new(&thin_root);
    register(&thin_observation, "run:thin");
    for ordinal in 0..MINIMUM_HISTORY_SAMPLES - 2 {
        thin_observation
            .record(&observation_of(
                "run:thin",
                &format!("effect:thin-{ordinal}"),
                EffectOutcome::Succeeded,
                ObservedCost::Known {
                    usage: usage(2, 0, 1),
                },
            ))
            .expect("observed");
    }
    let thin = PolicyHistory::from_metering(
        &thin_observation
            .meter_run("run:thin")
            .expect("metering facts"),
    );
    assert_eq!(thin.samples(), MINIMUM_HISTORY_SAMPLES - 2);

    // With too little history the ranking seam's bare fallback is still an
    // answer, and it is reported as a fallback rather than dressed up as advice
    // drawn from outcomes.
    let fallback = policy
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: None,
            authority: resolved_authority(),
            history: thin.clone(),
        })
        .expect("a suggestion");
    match fallback {
        SuggestionOutcome::Suggested {
            suggestion,
            basis,
            claim,
            ..
        } => {
            assert_eq!(suggestion.candidate, option("agent-a", "model-a", "low"));
            assert!(!suggestion.is_default);
            assert_eq!(
                basis,
                SuggestionBasis::OwnerRanking {
                    ranking_basis: "catalog-fallback".into()
                }
            );
            assert_eq!(claim, EvidenceClaim::NotMeasured);
        }
        other => panic!("expected the ranking seam's fallback, got {other:?}"),
    }

    // A ranking seam that declines leaves the honest state: not enough observed
    // outcomes to propose from.
    let declining = PolicyAdapter::new().with_strategy_port(Arc::new(DecliningStrategyPort));
    let insufficient = declining
        .suggest(&PolicyRequest {
            scope: scope.clone(),
            candidates: candidates.clone(),
            user_configuration: None,
            authority: resolved_authority(),
            history: thin,
        })
        .expect("a suggestion");
    match insufficient {
        SuggestionOutcome::InsufficientHistory { samples, required } => {
            assert_eq!(samples, MINIMUM_HISTORY_SAMPLES - 2);
            assert_eq!(required, MINIMUM_HISTORY_SAMPLES);
        }
        other => panic!("expected too little history, got {other:?}"),
    }
    assert_eq!(insufficient.state(), FactState::NotRecorded);
    let _ = std::fs::remove_dir_all(thin_root);

    // Without a resolved authorization context there is nothing to act on:
    // labels are not a context.
    let unresolved = policy
        .suggest(&PolicyRequest {
            scope,
            candidates,
            user_configuration: None,
            authority: AuthorityContextState::NotEvaluated,
            history,
        })
        .expect("a suggestion");
    assert!(matches!(
        unresolved,
        SuggestionOutcome::AuthorityNotResolved { .. }
    ));
    assert!(!AuthorityContextState::NotEvaluated.permits_automatic_suggestion());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn adoption_is_refused_outside_the_catalog_and_when_it_would_not_be_revocable() {
    let policy = PolicyAdapter::new();
    let scope = PlanningScopeSeam {
        task_kind: "workflow-turn".into(),
        configuration: "step1".into(),
    };
    let candidates = vec![option("agent-a", "model-a", "low")];

    let outside = policy
        .adopt(
            &AdoptedPlanningDefaultSeam {
                scope: scope.clone(),
                source: StrategySourceSeam {
                    source_id: "source:1".into(),
                    revision: "r1".into(),
                },
                selected_option: option("agent-z", "model-z", "high"),
                revocable: true,
                supersedes_revision: None,
            },
            &candidates,
        )
        .expect_err("an option the host cannot run is not a default");
    assert_eq!(outside.adapter, AdapterName::Policy);
    assert_eq!(outside.code, "policy_adoption_outside_candidate_catalog");
    assert_eq!(policy.future_default(&scope), None);

    let permanent = policy
        .adopt(
            &AdoptedPlanningDefaultSeam {
                scope: scope.clone(),
                source: StrategySourceSeam {
                    source_id: "source:1".into(),
                    revision: "r1".into(),
                },
                selected_option: option("agent-a", "model-a", "low"),
                revocable: false,
                supersedes_revision: None,
            },
            &candidates,
        )
        .expect_err("a future default must be revocable");
    assert_eq!(permanent.code, "policy_adoption_not_revocable");
    assert_eq!(permanent.recovery, "adopt_a_revocable_default");
    assert_eq!(
        permanent,
        AdapterError::refused(
            AdapterName::Policy,
            "policy_adoption_not_revocable",
            "adopt_a_revocable_default"
        )
    );
}

#[test]
fn an_observation_for_an_unregistered_run_is_a_typed_error_not_a_silent_zero() {
    let root = temp_root();
    let observation = ObservationAdapter::new(&root);
    let error = observation
        .record(&observation_of(
            "run:never-registered",
            "effect:one",
            EffectOutcome::Succeeded,
            ObservedCost::Known {
                usage: usage(1, 0, 1),
            },
        ))
        .expect_err("the ledger owns the run identity and refuses the row");
    assert_eq!(error.adapter, AdapterName::Observation);
    assert_eq!(error.code, "observation_run_not_registered");
    assert_eq!(
        error.owner_code.as_deref(),
        Some("usage_ledger_run_not_found")
    );
    assert!(!error.retryable);
    assert_eq!(error.recovery, "register_run_before_observing");
    assert_eq!(
        error.to_string(),
        "observation_run_not_registered/usage_ledger_run_not_found"
    );

    let _ = std::fs::remove_dir_all(root);
}
