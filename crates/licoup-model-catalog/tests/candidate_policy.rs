//! Synthetic policy fixtures for non-learning candidate routing.
//!
//! Every fact here is declared by the fixture: no probe runs, no provider is
//! contacted and no catalogue file is read. The fixture installs one candidate
//! fact table for the duration of a decision, so a test states exactly which
//! dimension one candidate fails and asserts that only that candidate is
//! excluded, under only that name, while every other allowed alternative
//! survives in a deterministic order.
//!
//! The fixture states a deviation, never a default: a candidate is observed,
//! credentialed, within quota and answering every requirement until the test
//! says otherwise, so an unexpected exclusion cannot hide behind a fixture
//! convenience.

use licoup_model_catalog::availability::ObservedAvailability;
use licoup_model_catalog::candidate_policy::{
    CandidateDecision, CandidateExclusion, CandidateId, CandidatePolicyPort, CandidateRelation,
    CandidateRequest, CandidateRequirement, CandidateUnavailable, ExcludedCandidate,
    ExclusionCategory, ExclusionCode, QuotaState, RequirementState, select_candidates,
};
use licoup_model_catalog::port::CredentialState;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

const OBSERVED_AT_UNIX_MS: u64 = 1_726_000_000_000;
const PRIMARY_WINDOW: &str = "primary";

thread_local! {
    /// The fact table the port answers for the fixture the current test
    /// installed. Each test runs on its own thread, so one fixture never leaks
    /// into another.
    static INSTALLED: RefCell<Option<Fixture>> = const { RefCell::new(None) };
}

/// The facts the fixture states for one candidate.
#[derive(Clone)]
struct Row {
    id: CandidateId,
    observed_at_unix_ms: Option<u64>,
    credential: CredentialState,
    quota: QuotaState,
    requirement_answers: BTreeMap<CandidateRequirement, RequirementState>,
}

impl Row {
    fn usable(id: CandidateId) -> Self {
        Self {
            id,
            observed_at_unix_ms: Some(OBSERVED_AT_UNIX_MS),
            credential: CredentialState::Present,
            quota: QuotaState::Available {
                window: PRIMARY_WINDOW.to_owned(),
            },
            requirement_answers: BTreeMap::new(),
        }
    }

    fn violating(mut self, requirement: CandidateRequirement, declared: Option<&str>) -> Self {
        self.requirement_answers.insert(
            requirement,
            RequirementState::Violated {
                declared: declared.map(str::to_owned),
            },
        );
        self
    }

    fn unestablished(mut self, requirement: CandidateRequirement) -> Self {
        self.requirement_answers
            .insert(requirement, RequirementState::Unknown);
        self
    }

    fn unobserved(mut self) -> Self {
        self.observed_at_unix_ms = None;
        self
    }

    fn without_credential(mut self) -> Self {
        self.credential = CredentialState::Absent;
        self
    }

    fn with_unestablished_credential(mut self) -> Self {
        self.credential = CredentialState::Unknown;
        self
    }

    fn quota_exhausted(mut self, window: &str) -> Self {
        self.quota = QuotaState::Exhausted {
            window: window.to_owned(),
        };
        self
    }

    fn quota_unknown(mut self) -> Self {
        self.quota = QuotaState::Unknown;
        self
    }
}

/// One synthetic fact table, keyed by the Agent each candidate runs on.
#[derive(Clone, Default)]
struct Fixture {
    rows: BTreeMap<String, Row>,
}

impl Fixture {
    fn new() -> Self {
        Self::default()
    }

    /// A fully usable candidate.
    fn candidate(self, agent: &str) -> Self {
        self.candidate_with(agent, |row| row)
    }

    /// A candidate with exactly the deviations the adjustment states.
    fn candidate_with(mut self, agent: &str, adjust: impl FnOnce(Row) -> Row) -> Self {
        let provider = format!("{agent}-provider");
        let id = CandidateId::new(agent, format!("{agent}-model"), Some(provider.as_str()));
        self.rows.insert(agent.to_owned(), adjust(Row::usable(id)));
        self
    }

    fn id(&self, agent: &str) -> CandidateId {
        self.rows
            .get(agent)
            .unwrap_or_else(|| panic!("the fixture names candidate {agent}"))
            .id
            .clone()
    }

    /// An admitted request over the named allowed candidates, in the named
    /// configured order.
    fn request(&self, allowed: &[&str], preference: &[&str]) -> CandidateRequest {
        let mut request = CandidateRequest::new("attempt-1", "direct");
        request.allowed = allowed.iter().map(|agent| self.id(agent)).collect();
        request.preference = preference.iter().map(|agent| self.id(agent)).collect();
        request
    }

    /// Answer one decision against this fixture and remove it afterwards.
    fn decide(&self, request: &CandidateRequest) -> CandidateDecision {
        self.with_installed(|| select_candidates(&synthetic_port(), request))
    }

    /// Run one call with this fixture installed for the port.
    fn with_installed<R>(&self, run: impl FnOnce() -> R) -> R {
        INSTALLED.with(|slot| *slot.borrow_mut() = Some(self.clone()));
        let result = run();
        INSTALLED.with(|slot| *slot.borrow_mut() = None);
        result
    }
}

fn installed_row(candidate: &CandidateId) -> Option<Row> {
    INSTALLED.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|fixture| fixture.rows.get(&candidate.agent_id))
            .cloned()
    })
}

fn requirement_answer(
    candidate: &CandidateId,
    requirement: &CandidateRequirement,
) -> RequirementState {
    match installed_row(candidate) {
        Some(row) => row
            .requirement_answers
            .get(requirement)
            .cloned()
            .unwrap_or(RequirementState::Satisfied),
        None => RequirementState::Unknown,
    }
}

fn candidate_availability(candidate: &CandidateId) -> ObservedAvailability {
    match installed_row(candidate) {
        Some(Row {
            observed_at_unix_ms: Some(at_unix_ms),
            ..
        }) => ObservedAvailability::Observed {
            at_unix_ms,
            sources: vec!["synthetic-probe".to_owned()],
        },
        _ => ObservedAvailability::Unknown,
    }
}

fn candidate_credential(candidate: &CandidateId) -> CredentialState {
    installed_row(candidate)
        .map(|row| row.credential)
        .unwrap_or(CredentialState::Unknown)
}

fn candidate_quota(candidate: &CandidateId) -> QuotaState {
    installed_row(candidate)
        .map(|row| row.quota)
        .unwrap_or(QuotaState::Unknown)
}

fn synthetic_port() -> CandidatePolicyPort {
    CandidatePolicyPort {
        requirement: requirement_answer,
        availability: candidate_availability,
        credential: candidate_credential,
        quota: candidate_quota,
    }
}

fn ranked_agents(decision: &CandidateDecision) -> Vec<String> {
    decision
        .ranked
        .iter()
        .map(|entry| entry.candidate.agent_id.clone())
        .collect()
}

#[test]
fn a_capability_mismatch_isolates_the_contradicting_candidate() {
    let fixture = Fixture::new()
        .candidate_with("agent-a", |row| {
            row.violating(CandidateRequirement::Tools, Some("false"))
        })
        .candidate("agent-b")
        .candidate("agent-c");
    let mut request = fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-b", "agent-c"],
    );
    request.requirements.push(CandidateRequirement::Tools);

    let decision = fixture.decide(&request);

    assert_eq!(
        decision.excluded,
        vec![ExcludedCandidate {
            candidate: fixture.id("agent-a"),
            exclusion: CandidateExclusion::RequirementViolated {
                requirement: CandidateRequirement::Tools,
                declared: Some("false".to_owned()),
            },
        }],
        "only the contradicting candidate is excluded"
    );
    assert_eq!(
        decision.excluded[0].exclusion.category(),
        ExclusionCategory::Mismatch
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-b", "agent-c"]);
    assert_eq!(decision.selected, Some(fixture.id("agent-b")));
    assert_eq!(
        decision.substitute_for,
        Some(fixture.id("agent-a")),
        "a substitution for the explicit choice is always reported"
    );
    assert_eq!(decision.unavailable, None);
}

#[test]
fn an_unestablished_requirement_is_named_apart_from_a_violated_one() {
    let fixture = Fixture::new()
        .candidate_with("agent-a", |row| {
            row.unestablished(CandidateRequirement::Reasoning)
        })
        .candidate("agent-b");
    let mut request = fixture.request(&["agent-a", "agent-b"], &["agent-a"]);
    request.requirements.push(CandidateRequirement::Reasoning);

    let decision = fixture.decide(&request);

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-a")),
        Some(&CandidateExclusion::RequirementUnknown {
            requirement: CandidateRequirement::Reasoning,
        }),
        "an unestablished fact is not a refusal of the capability"
    );
    assert_ne!(
        ExclusionCode::RequirementUnknown,
        ExclusionCode::RequirementViolated
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-b"]);
}

#[test]
fn a_model_identity_mismatch_is_reported_with_the_declared_identity() {
    let requested = CandidateRequirement::Model("moonshotai/kimi-k3".to_owned());
    let fixture = Fixture::new()
        .candidate_with("agent-a", |row| {
            row.violating(requested.clone(), Some("openai/gpt-5.6-sol"))
        })
        .candidate("agent-b");
    let mut request = fixture.request(&["agent-a", "agent-b"], &["agent-a", "agent-b"]);
    request.requirements.push(requested.clone());

    let decision = fixture.decide(&request);

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-a")),
        Some(&CandidateExclusion::RequirementViolated {
            requirement: requested,
            declared: Some("openai/gpt-5.6-sol".to_owned()),
        })
    );
    assert_eq!(decision.selected, Some(fixture.id("agent-b")));
}

#[test]
fn an_absent_credential_excludes_only_its_candidate_and_names_the_provider() {
    let fixture = Fixture::new()
        .candidate("agent-a")
        .candidate_with("agent-b", Row::without_credential)
        .candidate("agent-c");

    let decision = fixture.decide(&fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-b", "agent-c"],
    ));

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-b")),
        Some(&CandidateExclusion::CredentialAbsent {
            provider_id: Some("agent-b-provider".to_owned()),
        })
    );
    assert_eq!(
        decision.excluded[0].exclusion.category(),
        ExclusionCategory::Credential
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-a", "agent-c"]);
    assert_eq!(decision.selected, Some(fixture.id("agent-a")));
}

#[test]
fn an_unestablished_credential_is_not_reported_as_an_absent_one() {
    let fixture = Fixture::new()
        .candidate("agent-a")
        .candidate_with("agent-b", Row::with_unestablished_credential);

    let decision = fixture.decide(&fixture.request(&["agent-a", "agent-b"], &["agent-a"]));

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-b")),
        Some(&CandidateExclusion::CredentialUnknown {
            provider_id: Some("agent-b-provider".to_owned()),
        }),
        "a host that cannot establish a credential reports no negative claim"
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-a"]);
}

#[test]
fn quota_exhaustion_isolates_its_candidate_and_names_the_window() {
    let fixture = Fixture::new()
        .candidate("agent-a")
        .candidate_with("agent-b", |row| row.quota_exhausted("five-hour"))
        .candidate("agent-c");

    let decision = fixture.decide(&fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-b", "agent-c"],
    ));

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-b")),
        Some(&CandidateExclusion::QuotaExhausted {
            window: "five-hour".to_owned(),
            provider_id: Some("agent-b-provider".to_owned()),
        })
    );
    assert_eq!(
        decision.excluded[0].exclusion.category(),
        ExclusionCategory::Quota
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-a", "agent-c"]);
}

#[test]
fn an_unanswered_quota_source_is_recorded_and_never_read_as_exhaustion() {
    let fixture = Fixture::new().candidate_with("agent-a", Row::quota_unknown);

    let decision = fixture.decide(&fixture.request(&["agent-a"], &["agent-a"]));

    assert!(decision.excluded.is_empty());
    assert_eq!(decision.selected, Some(fixture.id("agent-a")));
    assert_eq!(
        decision
            .selected_candidate()
            .map(|entry| entry.quota.clone()),
        Some(QuotaState::Unknown),
        "no quota source is not a spent window and not a claim of capacity"
    );
    assert_eq!(decision.unavailable, None);
}

#[test]
fn a_model_no_live_source_reported_is_excluded_as_unobserved() {
    let fixture = Fixture::new()
        .candidate_with("agent-a", Row::unobserved)
        .candidate("agent-b");

    let decision = fixture.decide(&fixture.request(&["agent-a", "agent-b"], &["agent-a"]));

    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-a")),
        Some(&CandidateExclusion::AvailabilityUnobserved)
    );
    assert_eq!(
        decision.excluded[0].exclusion.category(),
        ExclusionCategory::Availability
    );
    assert_eq!(decision.selected, Some(fixture.id("agent-b")));
}

#[test]
fn only_the_allowed_alternatives_are_considered() {
    let fixture = Fixture::new()
        .candidate("agent-a")
        .candidate("agent-b")
        .candidate("agent-c")
        .candidate("agent-d");
    let request = fixture.request(&["agent-a", "agent-b"], &["agent-c", "agent-a"]);

    let decision = fixture.decide(&request);

    assert_eq!(
        decision.disallowed_preference,
        vec![fixture.id("agent-c")],
        "a preferred candidate the grants do not allow is reported, never used"
    );
    assert_eq!(ranked_agents(&decision), vec!["agent-a", "agent-b"]);
    assert_eq!(decision.selected, Some(fixture.id("agent-a")));
    assert_eq!(
        decision.substitute_for,
        Some(fixture.id("agent-c")),
        "selecting another candidate in the explicit choice's place is a reported substitution"
    );
    assert!(decision.excluded.is_empty());
    assert!(
        decision
            .ranked
            .iter()
            .all(|entry| request.allowed.contains(&entry.candidate)),
        "no candidate outside the effective grants is ever ranked"
    );
    assert!(!ranked_agents(&decision).contains(&"agent-d".to_owned()));
    assert_eq!(
        decision.ranked[0].relation,
        CandidateRelation::ConfiguredSubstitute { position: 2 },
        "the disallowed configured choice leaves its substitute at its declared step"
    );
    assert_eq!(
        decision.ranked[1].relation,
        CandidateRelation::Alternative,
        "an allowed candidate the request did not prefer keeps its own relation"
    );
}

#[test]
fn a_substitute_is_reported_when_the_explicit_choice_cannot_run() {
    let fixture = Fixture::new()
        .candidate_with("agent-a", Row::without_credential)
        .candidate("agent-b")
        .candidate("agent-c");

    let decision = fixture.decide(&fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-b", "agent-c"],
    ));

    assert_eq!(decision.selected, Some(fixture.id("agent-b")));
    assert_eq!(decision.substitute_for, Some(fixture.id("agent-a")));
    assert_eq!(
        decision.ranked[0].relation,
        CandidateRelation::ConfiguredSubstitute { position: 2 },
        "the first surviving step of the configured path is recommended"
    );
    assert_eq!(
        decision.ranked[0].candidate,
        fixture.id("agent-b"),
        "the excluded explicit choice keeps no ranked relation"
    );
    assert_eq!(
        decision.ranked[1].relation,
        CandidateRelation::ConfiguredSubstitute { position: 3 }
    );
    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-a")),
        Some(&CandidateExclusion::CredentialAbsent {
            provider_id: Some("agent-a-provider".to_owned()),
        })
    );
}

#[test]
fn equally_preferred_candidates_break_their_tie_by_stable_identity() {
    for insertion in [
        ["zeta", "alpha", "mid"],
        ["mid", "zeta", "alpha"],
        ["alpha", "mid", "zeta"],
    ] {
        let mut fixture = Fixture::new();
        for agent in insertion {
            fixture = fixture.candidate(agent);
        }
        let request = fixture.request(&insertion, &[]);
        let first = fixture.decide(&request);
        let second = fixture.decide(&request);

        assert_eq!(
            ranked_agents(&first),
            vec!["alpha", "mid", "zeta"],
            "equally preferred candidates order by stable identity, not insertion"
        );
        assert_eq!(
            first, second,
            "two decisions over the same facts are identical"
        );
        assert_eq!(
            first
                .ranked
                .iter()
                .map(|entry| entry.rank)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}

#[test]
fn a_substitute_step_keeps_its_declared_position() {
    let fixture = Fixture::new()
        .candidate("agent-a")
        .candidate("agent-b")
        .candidate("agent-c");

    let decision = fixture.decide(&fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-a", "agent-b", "agent-c"],
    ));

    assert_eq!(
        ranked_agents(&decision),
        vec!["agent-a", "agent-b", "agent-c"]
    );
    assert_eq!(decision.ranked[0].relation, CandidateRelation::Requested);
    assert_eq!(
        decision.ranked[1].relation,
        CandidateRelation::ConfiguredSubstitute { position: 2 },
        "a repeated declared choice occupies one step of the configured path"
    );
    assert_eq!(
        decision.ranked[2].relation,
        CandidateRelation::ConfiguredSubstitute { position: 3 }
    );
}

#[test]
fn the_first_blocker_in_the_fixed_precedence_is_reported() {
    let fixture = Fixture::new().candidate_with("agent-a", |row| {
        row.violating(CandidateRequirement::Tools, Some("false"))
            .without_credential()
    });
    let mut request = fixture.request(&["agent-a"], &["agent-a"]);
    request.requirements.push(CandidateRequirement::Tools);

    let decision = fixture.decide(&request);
    assert_eq!(
        decision.exclusion_for(&fixture.id("agent-a")),
        Some(&CandidateExclusion::RequirementViolated {
            requirement: CandidateRequirement::Tools,
            declared: Some("false".to_owned()),
        }),
        "requirements are decided before the credential"
    );

    let fixture = Fixture::new().candidate_with("agent-a", |row| {
        row.violating(
            CandidateRequirement::Model("moonshotai/kimi-k3".to_owned()),
            None,
        )
        .violating(CandidateRequirement::Tools, Some("false"))
    });
    let mut request = fixture.request(&["agent-a"], &["agent-a"]);
    request
        .requirements
        .push(CandidateRequirement::Model("moonshotai/kimi-k3".to_owned()));
    request.requirements.push(CandidateRequirement::Tools);

    assert_eq!(
        fixture
            .decide(&request)
            .exclusion_for(&fixture.id("agent-a"))
            .map(|exclusion| exclusion.code()),
        Some(ExclusionCode::RequirementViolated),
        "the request's requirements are decided in their declared order"
    );
}

#[test]
fn an_all_excluded_request_reports_every_distinct_reason() {
    let fixture = Fixture::new()
        .candidate_with("agent-a", |row| {
            row.violating(CandidateRequirement::Tools, Some("false"))
        })
        .candidate_with("agent-b", Row::without_credential)
        .candidate_with("agent-c", |row| row.quota_exhausted("weekly"));
    let mut request = fixture.request(
        &["agent-a", "agent-b", "agent-c"],
        &["agent-a", "agent-b", "agent-c"],
    );
    request.requirements.push(CandidateRequirement::Tools);

    let decision = fixture.decide(&request);

    assert_eq!(decision.selected, None);
    assert!(!decision.is_available());
    assert!(decision.ranked.is_empty());
    assert_eq!(decision.excluded.len(), 3);
    assert_eq!(
        decision.unavailable,
        Some(CandidateUnavailable::EveryCandidateExcluded {
            codes: vec![
                ExclusionCode::RequirementViolated,
                ExclusionCode::CredentialAbsent,
                ExclusionCode::QuotaExhausted,
            ],
        }),
        "the summary names every distinct reason, sorted"
    );
    assert_eq!(
        decision.substitute_for, None,
        "nothing was selected, so nothing was substituted"
    );
}

#[test]
fn a_request_whose_grants_allow_nothing_reports_no_allowed_candidate() {
    let fixture = Fixture::new().candidate("agent-a").candidate("agent-b");
    let request = fixture.request(&[], &["agent-a", "agent-b"]);

    let decision = fixture.decide(&request);

    assert_eq!(
        decision.unavailable,
        Some(CandidateUnavailable::NoAllowedCandidate)
    );
    assert_eq!(
        decision.disallowed_preference,
        vec![fixture.id("agent-a"), fixture.id("agent-b")]
    );
    assert_eq!(decision.selected, None);
    assert_eq!(decision.substitute_for, None);
    assert!(decision.ranked.is_empty());
    assert!(decision.excluded.is_empty());
}

#[test]
fn an_unanswered_port_never_selects_a_candidate() {
    let fixture = Fixture::new().candidate("agent-a").candidate("agent-b");

    let mut request = fixture.request(&["agent-a", "agent-b"], &["agent-a"]);
    request.requirements.push(CandidateRequirement::Tools);
    let decision =
        fixture.with_installed(|| select_candidates(&CandidatePolicyPort::unavailable(), &request));

    assert_eq!(decision.selected, None);
    assert!(decision.ranked.is_empty());
    assert_eq!(
        decision.unavailable,
        Some(CandidateUnavailable::EveryCandidateExcluded {
            codes: vec![ExclusionCode::RequirementUnknown],
        })
    );

    // Without a requirement, the unanswered catalogue gate reports itself
    // rather than an empty available catalogue.
    let plain = fixture.request(&["agent-a", "agent-b"], &["agent-a"]);
    let decision =
        fixture.with_installed(|| select_candidates(&CandidatePolicyPort::default(), &plain));
    assert_eq!(
        decision.unavailable,
        Some(CandidateUnavailable::EveryCandidateExcluded {
            codes: vec![ExclusionCode::AvailabilityUnobserved],
        })
    );
    assert!(
        decision
            .excluded
            .iter()
            .all(|excluded| { excluded.exclusion == CandidateExclusion::AvailabilityUnobserved })
    );
}

#[test]
fn a_surviving_candidate_reports_its_observation_time_and_quota() {
    let fixture = Fixture::new().candidate("agent-a");

    let decision = fixture.decide(&fixture.request(&["agent-a"], &["agent-a"]));

    assert_eq!(decision.request_id, "attempt-1");
    assert_eq!(decision.scope, "direct");
    let entry = decision.selected_candidate().expect("one survivor");
    assert_eq!(entry.rank, 1);
    assert_eq!(entry.observed_at_unix_ms, Some(OBSERVED_AT_UNIX_MS));
    assert_eq!(
        entry.quota,
        QuotaState::Available {
            window: PRIMARY_WINDOW.to_owned(),
        }
    );
}

#[test]
fn every_exclusion_has_a_stable_code_and_category() {
    let cases = [
        (
            CandidateExclusion::RequirementViolated {
                requirement: CandidateRequirement::Tools,
                declared: Some("false".to_owned()),
            },
            ExclusionCode::RequirementViolated,
            ExclusionCategory::Mismatch,
        ),
        (
            CandidateExclusion::RequirementUnknown {
                requirement: CandidateRequirement::Reasoning,
            },
            ExclusionCode::RequirementUnknown,
            ExclusionCategory::Mismatch,
        ),
        (
            CandidateExclusion::AvailabilityUnobserved,
            ExclusionCode::AvailabilityUnobserved,
            ExclusionCategory::Availability,
        ),
        (
            CandidateExclusion::CredentialAbsent {
                provider_id: Some("provider-a".to_owned()),
            },
            ExclusionCode::CredentialAbsent,
            ExclusionCategory::Credential,
        ),
        (
            CandidateExclusion::CredentialUnknown { provider_id: None },
            ExclusionCode::CredentialUnknown,
            ExclusionCategory::Credential,
        ),
        (
            CandidateExclusion::QuotaExhausted {
                window: "weekly".to_owned(),
                provider_id: Some("provider-a".to_owned()),
            },
            ExclusionCode::QuotaExhausted,
            ExclusionCategory::Quota,
        ),
    ];

    let mut names = BTreeSet::new();
    for (exclusion, code, category) in cases {
        assert_eq!(exclusion.code(), code);
        assert_eq!(exclusion.category(), category);
        assert!(!code.as_str().is_empty());
        names.insert(code.as_str());
    }
    assert_eq!(names.len(), 6, "each exclusion keeps its own name");
}
