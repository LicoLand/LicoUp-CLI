//! Trusted production evaluation producer for an admitted session.
//!
//! The producer always runs. Only the external process/model observer may be
//! replaced in hermetic tests. Observations are never imported through a
//! public slot or caller-supplied owner id. Production invokes the same
//! admitted PersistentTurn callback bound by `bind_persistent_cognition`.

use std::sync::Arc;

use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityDatasetSplit, ContinuityEffectStatus, ContinuityFailure,
    ContinuityFailureCode, ContinuityFailureStage, ContinuityInterpretationProposal,
    EvaluationCasePolarity, EvaluationExpectedAction, StoredEvaluationCase, StoredEvaluationCorpus,
    StoredEvaluationSession, begin_collection_invocation, load_evaluation_corpus,
    load_evaluation_session,
};
use licoup_conversation::{Conversation, ConversationStore, MembershipStatus, PrincipalKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::domain::agent_intelligence_catalog::qualification::{
    ObservationJudgment, ObservationPolarity, QualificationObservation,
};
use crate::domain::assistant_continuity::cognition::{
    AdmittedTurnRequest, CompleteAdmittedTurn, compose_admitted_turn_params, continuity_failure,
    proposal_has_business_effect,
};
use crate::platform::runtime_adapters::RuntimeAdapterError;

pub const STORED_RECEIPT_COLLECTOR_ID: &str = "collector:stored-receipts";
pub const HERMETIC_EVALUATION_STAND_IN_ID: &str = "collector:hermetic-evaluation-stand-in";
pub const SYNTHETIC_COLLECTOR_ID: &str = "collector:synthetic";
pub const QUALIFICATION_EVALUATION_KIND: &str = "qualification-evaluation";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectReceipt {
    pub effect_id: String,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionReceipt {
    pub collector_id: String,
    pub session_id: String,
    pub conversation_id: String,
    pub responsibility_id: String,
    #[serde(default)]
    pub dataset_id: String,
    #[serde(default)]
    pub corpus_version: String,
    pub effect_receipts: Vec<EffectReceipt>,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CollectedEvaluation {
    pub observations: Vec<QualificationObservation>,
    pub receipt: CollectionReceipt,
}

#[derive(Clone, Debug)]
pub struct EvaluationObserveRequest {
    pub session_id: String,
    pub conversation_id: String,
    pub recipient_membership_id: String,
    pub responsibility_id: String,
    pub identity: ContinuityCandidateIdentity,
    pub policy_revision: String,
    pub collection_effect_id: String,
}

/// External process/model boundary. Production uses the admitted runtime.
/// Hermetic tests replace only this observer.
pub trait ExternalEvaluationObserver: Send + Sync {
    fn observer_id(&self) -> &'static str;
    fn observe(
        &self,
        request: &EvaluationObserveRequest,
    ) -> Result<Vec<QualificationObservation>, ContinuityFailure>;
}

/// Production observer. Invokes the bound admitted PersistentTurn callback on
/// known cases and grades typed outputs. Unbound runtime stays unavailable.
pub struct AdmittedRuntimeEvaluationObserver {
    store: ConversationStore,
    complete_turn: Option<Arc<CompleteAdmittedTurn>>,
}

impl AdmittedRuntimeEvaluationObserver {
    pub fn new(store: ConversationStore, complete_turn: Option<Arc<CompleteAdmittedTurn>>) -> Self {
        Self {
            store,
            complete_turn,
        }
    }
}

impl ExternalEvaluationObserver for AdmittedRuntimeEvaluationObserver {
    fn observer_id(&self) -> &'static str {
        STORED_RECEIPT_COLLECTOR_ID
    }

    fn observe(
        &self,
        request: &EvaluationObserveRequest,
    ) -> Result<Vec<QualificationObservation>, ContinuityFailure> {
        let Some(complete_turn) = &self.complete_turn else {
            return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
        };
        if request.session_id.trim().is_empty()
            || request.collection_effect_id != collection_effect_id(&request.session_id)
            || request.policy_revision.trim().is_empty()
            || request.policy_revision != request.identity.policy_revision
        {
            return Err(observe_failure(ContinuityFailureCode::InvalidRequest));
        }
        let session = load_evaluation_session(&self.store, &request.session_id)?
            .ok_or_else(|| observe_failure(ContinuityFailureCode::InvalidRequest))?;
        if session.consumed
            || session.session_id != request.session_id
            || session.conversation_id != request.conversation_id
            || session.recipient_membership_id != request.recipient_membership_id
            || session.responsibility_id != request.responsibility_id
            || session.identity != request.identity
            || session.policy_revision != request.policy_revision
        {
            return Err(observe_failure(ContinuityFailureCode::IdentityConflict));
        }
        let conversation = self
            .store
            .get(&request.conversation_id)
            .map_err(|_| observe_failure(ContinuityFailureCode::SourceUnavailable))?;
        if conversation.archived {
            return Err(observe_failure(ContinuityFailureCode::ScopeDenied));
        }
        if !recipient_is_current(&conversation, &request.recipient_membership_id) {
            return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
        }
        let corpus = admitted_corpus(&self.store, &session)?;
        begin_collection_invocation(&self.store, &session)?;
        let mut observations = Vec::with_capacity(corpus.cases.len());
        for case in &corpus.cases {
            observations.push(observe_case(
                &self.store,
                complete_turn.as_ref(),
                request,
                &corpus,
                case,
            )?);
        }
        if observations.is_empty() {
            return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
        }
        Ok(observations)
    }
}

#[derive(Clone)]
pub struct HermeticEvaluationObserver {
    observations: Vec<QualificationObservation>,
}

impl HermeticEvaluationObserver {
    pub fn new(observations: Vec<QualificationObservation>) -> Self {
        Self { observations }
    }
}

impl ExternalEvaluationObserver for HermeticEvaluationObserver {
    fn observer_id(&self) -> &'static str {
        HERMETIC_EVALUATION_STAND_IN_ID
    }

    fn observe(
        &self,
        request: &EvaluationObserveRequest,
    ) -> Result<Vec<QualificationObservation>, ContinuityFailure> {
        if request.session_id.trim().is_empty()
            || request.collection_effect_id != collection_effect_id(&request.session_id)
        {
            return Err(observe_failure(ContinuityFailureCode::InvalidRequest));
        }
        if self.observations.is_empty() {
            return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
        }
        Ok(self.observations.clone())
    }
}

pub fn collection_effect_id(session_id: &str) -> String {
    format!("effect:collection:{session_id}")
}

pub fn effect_belongs_to_session(effect_id: &str, session_id: &str) -> bool {
    effect_id == collection_effect_id(session_id)
}

pub fn produce_collected_evaluation(
    session: &StoredEvaluationSession,
    observer: &dyn ExternalEvaluationObserver,
) -> Result<CollectedEvaluation, ContinuityFailure> {
    if observer.observer_id() == SYNTHETIC_COLLECTOR_ID {
        return Err(observe_failure(ContinuityFailureCode::InvalidRequest));
    }
    let effect_id = collection_effect_id(&session.session_id);
    let request = EvaluationObserveRequest {
        session_id: session.session_id.clone(),
        conversation_id: session.conversation_id.clone(),
        recipient_membership_id: session.recipient_membership_id.clone(),
        responsibility_id: session.responsibility_id.clone(),
        identity: session.identity.clone(),
        policy_revision: session.policy_revision.clone(),
        collection_effect_id: effect_id.clone(),
    };
    let observations = observer.observe(&request)?;
    if observations.is_empty() {
        return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
    }
    let receipt = collection_receipt(
        observer.observer_id(),
        session,
        &[EffectReceipt {
            effect_id,
            status: ContinuityEffectStatus::Executed.as_str().to_owned(),
        }],
        &observations,
    );
    Ok(CollectedEvaluation {
        observations,
        receipt,
    })
}

pub fn collection_receipt(
    collector_id: &str,
    session: &StoredEvaluationSession,
    effect_receipts: &[EffectReceipt],
    observations: &[QualificationObservation],
) -> CollectionReceipt {
    let mut effect_receipts = effect_receipts.to_vec();
    effect_receipts.sort_by(|left, right| left.effect_id.cmp(&right.effect_id));
    let mut receipt = CollectionReceipt {
        collector_id: collector_id.to_owned(),
        session_id: session.session_id.clone(),
        conversation_id: session.conversation_id.clone(),
        responsibility_id: session.responsibility_id.clone(),
        dataset_id: session.dataset_id.clone(),
        corpus_version: session.corpus_version.clone(),
        effect_receipts,
        digest: String::new(),
    };
    receipt.digest = collection_digest(&receipt, observations);
    receipt
}

pub fn collection_digest(
    receipt: &CollectionReceipt,
    observations: &[QualificationObservation],
) -> String {
    let effects = receipt
        .effect_receipts
        .iter()
        .map(|item| format!("{}:{}", item.effect_id, item.status))
        .collect::<Vec<_>>()
        .join(",");
    let canonical = format!(
        "collector={}|session={}|conversation={}|responsibility={}|dataset={}|corpus={}|effects={effects}|evidence={}",
        receipt.collector_id,
        receipt.session_id,
        receipt.conversation_id,
        receipt.responsibility_id,
        receipt.dataset_id,
        receipt.corpus_version,
        observation_binding(observations)
    );
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

pub fn validate_collection_receipt(
    receipt: &CollectionReceipt,
    session: &StoredEvaluationSession,
    observations: &[QualificationObservation],
) -> Result<(), ContinuityFailure> {
    if receipt.collector_id == SYNTHETIC_COLLECTOR_ID
        || receipt.collector_id.trim().is_empty()
        || receipt.digest != collection_digest(receipt, observations)
        || receipt.session_id != session.session_id
        || receipt.conversation_id != session.conversation_id
        || receipt.responsibility_id != session.responsibility_id
        || receipt.dataset_id != session.dataset_id
        || receipt.corpus_version != session.corpus_version
        || receipt.effect_receipts.is_empty()
        || receipt
            .effect_receipts
            .iter()
            .any(|item| !effect_belongs_to_session(&item.effect_id, &session.session_id))
    {
        return Err(observe_failure(ContinuityFailureCode::InvalidRequest));
    }
    Ok(())
}

fn admitted_corpus(
    store: &ConversationStore,
    session: &StoredEvaluationSession,
) -> Result<StoredEvaluationCorpus, ContinuityFailure> {
    let Some(corpus) = load_evaluation_corpus(store, &session.conversation_id)? else {
        return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
    };
    if corpus.cases.is_empty() {
        return Err(observe_failure(ContinuityFailureCode::InvalidRequest));
    }
    if session.dataset_id.trim().is_empty()
        || session.corpus_version.trim().is_empty()
        || corpus.dataset_id != session.dataset_id
        || corpus.version_digest != session.corpus_version
        || session.identity.dataset_version != corpus.version_digest
    {
        return Err(observe_failure(ContinuityFailureCode::StaleRevision));
    }
    Ok(corpus)
}

fn observe_case(
    store: &ConversationStore,
    complete_turn: &CompleteAdmittedTurn,
    request: &EvaluationObserveRequest,
    corpus: &StoredEvaluationCorpus,
    case: &StoredEvaluationCase,
) -> Result<QualificationObservation, ContinuityFailure> {
    let mut params = compose_admitted_turn_params(&AdmittedTurnRequest {
        store,
        conversation_id: &request.conversation_id,
        membership_id: &request.recipient_membership_id,
        text: &case.input,
        causation_id: None,
        dispatch_id: None,
        continuity_kind: QUALIFICATION_EVALUATION_KIND,
        goal_id: None,
        goal_revision: None,
        review_policy: None,
        parent_conversation_id: None,
        assembly: None,
        orientation: None,
        include_licoup_guide: false,
    })?;
    params["evaluationSessionId"] = json!(request.session_id);
    params["evaluationCaseId"] = json!(case.case_id);
    params["responsibilityId"] = json!(request.responsibility_id);
    params["policyRevision"] = json!(request.policy_revision);
    params["datasetId"] = json!(corpus.dataset_id);
    params["datasetVersion"] = json!(corpus.version_digest);
    let value = complete_turn(&params).map_err(map_runtime_error)?;
    validate_evaluation_reply(&value, request)?;
    let output = turn_output(&value);
    let proposal = parse_evaluation_proposal(&output)?;
    let took_over = proposal.as_ref().is_some_and(proposal_has_business_effect);
    Ok(QualificationObservation {
        observation_id: format!("obs:{}:{}", case.case_id, request.session_id),
        conversation_family: case.family.clone(),
        split: ContinuityDatasetSplit::Heldout,
        subgroup: case.subgroup.clone(),
        polarity: map_polarity(case.polarity),
        judgment: grade_case(case.expected_action, took_over),
        self_confidence: None,
        hard_invariants: Default::default(),
        economy: None,
        closure_claim: None,
    })
}

fn grade_case(expected: EvaluationExpectedAction, took_over: bool) -> ObservationJudgment {
    match (expected, took_over) {
        (EvaluationExpectedAction::Takeover, true) | (EvaluationExpectedAction::Abstain, false) => {
            ObservationJudgment::Correct
        }
        (EvaluationExpectedAction::Abstain, true) => ObservationJudgment::FalseTakeover,
        (EvaluationExpectedAction::Takeover, false) => ObservationJudgment::MissedCommitment,
    }
}

fn map_polarity(polarity: EvaluationCasePolarity) -> ObservationPolarity {
    match polarity {
        EvaluationCasePolarity::Negative => ObservationPolarity::Negative,
        EvaluationCasePolarity::Positive => ObservationPolarity::Positive,
    }
}

fn parse_evaluation_proposal(
    output: &str,
) -> Result<Option<ContinuityInterpretationProposal>, ContinuityFailure> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = serde_json::from_str::<Value>(trimmed)
        .ok()
        .or_else(|| extract_embedded_json(trimmed))
        .ok_or_else(|| observe_failure(ContinuityFailureCode::InvalidRequest))?;
    let candidate = parsed
        .get("interpretationProposal")
        .cloned()
        .unwrap_or(parsed);
    serde_json::from_value::<ContinuityInterpretationProposal>(candidate)
        .map(Some)
        .map_err(|_| observe_failure(ContinuityFailureCode::InvalidRequest))
}

fn extract_embedded_json(output: &str) -> Option<Value> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&output[start..=end]).ok()
}

fn validate_evaluation_reply(
    value: &Value,
    request: &EvaluationObserveRequest,
) -> Result<(), ContinuityFailure> {
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(observe_failure(ContinuityFailureCode::SourceUnavailable));
    }
    if mismatched_field(value, "conversationId", &request.conversation_id)
        || mismatched_field(value, "evaluationSessionId", &request.session_id)
        || mismatched_field(value, "responsibilityId", &request.responsibility_id)
        || mismatched_field(value, "membershipId", &request.recipient_membership_id)
        || mismatched_field(value, "policyRevision", &request.policy_revision)
    {
        return Err(observe_failure(ContinuityFailureCode::StaleRevision));
    }
    Ok(())
}

fn mismatched_field(value: &Value, key: &str, expected: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|observed| observed != expected)
}

fn turn_output(value: &Value) -> String {
    value
        .get("output")
        .and_then(Value::as_str)
        .or_else(|| value.get("text").and_then(Value::as_str))
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_default()
}

fn map_runtime_error(error: RuntimeAdapterError) -> ContinuityFailure {
    let code = match error {
        RuntimeAdapterError::ConversationDispatchFailed
        | RuntimeAdapterError::ExecutableUnavailable
        | RuntimeAdapterError::RuntimeProfileUnavailable
        | RuntimeAdapterError::UnsupportedAdapter { .. } => {
            ContinuityFailureCode::SourceUnavailable
        }
        RuntimeAdapterError::AgentIdentifierMissing
        | RuntimeAdapterError::MessageMissing
        | RuntimeAdapterError::InvalidRuntimeSetting { .. }
        | RuntimeAdapterError::LegacyLaunchConfiguration => ContinuityFailureCode::InvalidRequest,
        _ => ContinuityFailureCode::SourceUnavailable,
    };
    observe_failure(code)
}

fn recipient_is_current(conversation: &Conversation, membership_id: &str) -> bool {
    conversation.memberships.iter().any(|membership| {
        membership.id == membership_id
            && membership.status == MembershipStatus::Active
            && membership.principal.kind == PrincipalKind::Agent
    })
}

fn observation_binding(observations: &[QualificationObservation]) -> String {
    let mut rows: Vec<String> = observations
        .iter()
        .map(|observation| {
            serde_json::to_string(observation)
                .unwrap_or_else(|_| observation.observation_id.clone())
        })
        .collect();
    rows.sort();
    rows.join(";")
}

fn observe_failure(code: ContinuityFailureCode) -> ContinuityFailure {
    continuity_failure(code, ContinuityFailureStage::ContinuityAdmission)
}
