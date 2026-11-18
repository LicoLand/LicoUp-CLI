//! Scripted Agent. Lookup is exact Event/Part/span identity only.

use std::collections::HashMap;

use licoup_conversation::continuity::{
    ContinuityAgreementProposal, ContinuityFollowThroughKind, ContinuityMatterSubject,
    ContinuitySourceRef, ContinuitySpeechAct,
};

/// One independent semantic axis on an exact source span.
///
/// Axes are not derived from one another. `create_goal` is not implied by
/// `speech_act`, message length, entity count, or model confidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpanAxis {
    pub source_ref: ContinuitySourceRef,
    pub subject: ContinuityMatterSubject,
    pub speech_act: ContinuitySpeechAct,
    pub follow_through: ContinuityFollowThroughKind,
    pub create_goal: bool,
    pub matter_id: Option<String>,
    pub expected_result: Option<String>,
    pub capability_needs: Vec<String>,
    pub uncertainty_reasons: Vec<String>,
    pub requested_reads: Vec<ContinuitySourceRef>,
    pub agreement_proposals: Vec<ContinuityAgreementProposal>,
    pub abstain: bool,
    pub reason_code: String,
}

/// Scripted understanding for one original Event. Multiple spans stay independent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticScript {
    pub event_opaque_id: String,
    pub axes: Vec<SpanAxis>,
    pub fused: bool,
    /// Reported model self-confidence. Never a permission or Goal signal.
    pub model_confidence: Option<u8>,
    pub escalate: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptedAgent {
    by_event: HashMap<String, SemanticScript>,
}

impl ScriptedAgent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, script: SemanticScript) {
        self.by_event.insert(script.event_opaque_id.clone(), script);
    }

    /// Exact Event identity only. No lexical, length, or count lookup.
    pub fn lookup_event(&self, opaque_id: &str) -> Option<&SemanticScript> {
        self.by_event.get(opaque_id)
    }
}
