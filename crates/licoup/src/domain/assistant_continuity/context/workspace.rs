//! Shared compose/interpret session. Host wiring in M2 replaces the substitute.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use licoup_conversation::continuity::{
    ContinuityContextCompositionRequest, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityInterpretationProposal,
};

use super::super::cognition::{
    AssemblySnapshot, AssemblySource, ScriptedAgent, UnavailableInterpretationService,
    UnavailableKnowledgeService, continuity_failure,
};
use super::store::FrozenContextStore;

#[derive(Default)]
struct SessionMemory {
    assemblies: HashMap<String, AssemblySnapshot>,
    proposals: HashMap<String, ContinuityInterpretationProposal>,
    replay: HashMap<String, ContinuityInterpretationProposal>,
    cognition: HashMap<String, u32>,
    total_cognition: u32,
}

pub struct ContinuityWorkspace {
    pub store: FrozenContextStore,
    agent: ScriptedAgent,
    knowledge: UnavailableKnowledgeService,
    memory: Mutex<SessionMemory>,
}

fn lock(memory: &Mutex<SessionMemory>) -> MutexGuard<'_, SessionMemory> {
    memory
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ContinuityWorkspace {
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            store: FrozenContextStore::new(),
            agent: ScriptedAgent::new(),
            knowledge: UnavailableKnowledgeService::default(),
            memory: Mutex::new(SessionMemory::default()),
        })
    }

    pub fn new(
        store: FrozenContextStore,
        agent: ScriptedAgent,
        knowledge: UnavailableKnowledgeService,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            agent,
            knowledge,
            memory: Mutex::new(SessionMemory::default()),
        })
    }

    pub fn knowledge(&self) -> UnavailableKnowledgeService {
        self.knowledge.clone()
    }

    pub fn interpreter(self: &Arc<Self>) -> UnavailableInterpretationService {
        UnavailableInterpretationService::new(self.clone(), self.knowledge.clone())
    }

    pub fn remember_assembly(&self, snapshot: AssemblySnapshot) {
        lock(&self.memory)
            .assemblies
            .insert(snapshot.invocation_id.clone(), snapshot);
    }

    pub fn assembly_snapshot(
        &self,
        invocation_id: &str,
    ) -> Result<AssemblySnapshot, ContinuityFailure> {
        lock(&self.memory)
            .assemblies
            .get(invocation_id)
            .cloned()
            .ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::SourceUnavailable,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })
    }

    pub fn merge_reads(
        &self,
        invocation_id: &str,
        reads: Vec<licoup_conversation::continuity::ContinuitySourceRef>,
        records: Vec<super::super::cognition::ContextRecord>,
    ) -> Result<AssemblySnapshot, ContinuityFailure> {
        let mut memory = lock(&self.memory);
        let snapshot = memory.assemblies.get_mut(invocation_id).ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
        snapshot.retrieved_reads.extend(reads);
        snapshot.records.extend(records);
        Ok(snapshot.clone())
    }

    pub fn total_cognition(&self) -> u32 {
        lock(&self.memory).total_cognition
    }
}

impl AssemblySource for ContinuityWorkspace {
    fn assembly(&self, invocation_id: &str) -> Result<AssemblySnapshot, ContinuityFailure> {
        self.assembly_snapshot(invocation_id)
    }

    fn cached_proposal(&self, replay_key: &str) -> Option<ContinuityInterpretationProposal> {
        lock(&self.memory).replay.get(replay_key).cloned()
    }

    fn proposal_for_invocation(
        &self,
        invocation_id: &str,
    ) -> Option<ContinuityInterpretationProposal> {
        lock(&self.memory).proposals.get(invocation_id).cloned()
    }

    fn remember_proposal(
        &self,
        invocation_id: &str,
        replay_key: &str,
        proposal: ContinuityInterpretationProposal,
        cognition_delta: u32,
    ) {
        let mut memory = lock(&self.memory);
        memory
            .proposals
            .insert(invocation_id.to_string(), proposal.clone());
        if !replay_key.is_empty() {
            memory.replay.insert(replay_key.to_string(), proposal);
        }
        let count = memory
            .cognition
            .entry(invocation_id.to_string())
            .or_insert(0);
        *count = count.saturating_add(cognition_delta);
        memory.total_cognition = memory.total_cognition.saturating_add(cognition_delta);
    }

    fn agent(&self) -> &ScriptedAgent {
        &self.agent
    }

    fn cognition_count(&self, invocation_id: &str) -> u32 {
        lock(&self.memory)
            .cognition
            .get(invocation_id)
            .copied()
            .unwrap_or(0)
    }

    fn total_cognition(&self) -> u32 {
        lock(&self.memory).total_cognition
    }
}

pub fn invocation_id(
    request: &ContinuityContextCompositionRequest,
    input_identity: Option<&str>,
    refine: bool,
) -> String {
    format!(
        "invocation:{}:{}:{}:{}:{}:{}:{}",
        request.conversation_id,
        request.recipient_membership_id,
        request.revocation_generation,
        authorization_key(request),
        request.after.as_deref().unwrap_or("none"),
        input_identity.unwrap_or("none"),
        if refine { "refine" } else { "compose" }
    )
}

pub fn replay_key(
    request: &ContinuityContextCompositionRequest,
    input_identity: Option<&str>,
    input_revision: Option<i64>,
) -> String {
    format!(
        "replay:{}:{}:{}:{}:{}:{}:{}",
        request.conversation_id,
        request.recipient_membership_id,
        authorization_key(request),
        request.after.as_deref().unwrap_or("none"),
        input_identity.unwrap_or("none"),
        input_revision.unwrap_or(0),
        request.revocation_generation
    )
}

fn authorization_key(request: &ContinuityContextCompositionRequest) -> String {
    let mut scopes = request
        .authorized_scopes
        .iter()
        .map(|scope| format!("{scope:?}"))
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes.join(",")
}
