//! Protocol surface used by the work-context engine.
//!
//! Compared with the production Codex app-server (`thread/resume` vs
//! `thread/start`, identity mismatch fails without start) and Pi session
//! resolver (`session/resume` missing/ambiguous fails closed). Fixtures replay
//! those decisions without launching an agent.

use super::types::{
    CapabilityProfile, ForkInheritance, IsolationReview, IsolationVerdict, NativeControlRequest,
    NativeFidelity, ParallelPolicy, ProtocolEffect, ProtocolFamily, ProtocolMethods,
    SessionPresence, default_snapshot, protocol_methods,
};
use super::{
    NativeCapabilitySnapshot, NativeCapabilitySupport, NativeWorkContextFailure,
    NativeWorkContextKey,
};

pub trait NativeProtocolAdapter: Send + Sync {
    fn family(&self) -> ProtocolFamily;
    fn profile(&self) -> CapabilityProfile;
    fn capabilities(&self) -> NativeCapabilitySnapshot;
    fn methods(&self) -> ProtocolMethods;
    fn fidelity(&self, child_author: &NativeFidelity) -> NativeFidelity;
    fn isolation(&self) -> IsolationReview;
    fn parallel_policy(&self) -> ParallelPolicy;
    fn session_presence(&self, key: &NativeWorkContextKey) -> SessionPresence;
    fn exact_resume(&self, key: &NativeWorkContextKey) -> ProtocolOutcome;
    fn start_new(&self, key: &NativeWorkContextKey) -> ProtocolOutcome;
    fn fork(&self, key: &NativeWorkContextKey, inheritance: &ForkInheritance) -> ProtocolOutcome;
    fn compact(&self, key: &NativeWorkContextKey) -> ProtocolOutcome;
    fn steer(&self, request: &NativeControlRequest) -> ProtocolOutcome;
    fn cancel(&self, request: &NativeControlRequest) -> ProtocolOutcome;
    fn session_conflict_key(&self, key: &NativeWorkContextKey) -> Option<String> {
        let _ = key;
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolOutcome {
    pub method: &'static str,
    pub effect: ProtocolEffect,
    pub cache_miss: bool,
    pub failure: Option<NativeWorkContextFailure>,
}

impl ProtocolOutcome {
    pub fn applied(method: &'static str) -> Self {
        Self {
            method,
            effect: ProtocolEffect::Applied,
            cache_miss: false,
            failure: None,
        }
    }

    pub fn failed(method: &'static str, failure: NativeWorkContextFailure) -> Self {
        Self {
            method,
            effect: ProtocolEffect::None,
            cache_miss: false,
            failure: Some(failure),
        }
    }

    pub fn unknown(method: &'static str, failure: NativeWorkContextFailure) -> Self {
        Self {
            method,
            effect: ProtocolEffect::Unknown,
            cache_miss: false,
            failure: Some(failure),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HermeticProtocol {
    family: ProtocolFamily,
    profile: CapabilityProfile,
    presence: SessionPresence,
    snapshot: NativeCapabilitySnapshot,
    next_resume: Option<ProtocolOutcome>,
    next_cancel: Option<ProtocolOutcome>,
    knowledge_injected: bool,
    session_conflict_key: Option<String>,
}

impl HermeticProtocol {
    pub fn new(family: ProtocolFamily, profile: CapabilityProfile) -> Self {
        Self {
            family,
            profile,
            presence: match profile {
                CapabilityProfile::High => SessionPresence::Present,
                CapabilityProfile::Low => SessionPresence::Present,
            },
            snapshot: default_snapshot(family, profile),
            next_resume: None,
            next_cancel: None,
            knowledge_injected: profile == CapabilityProfile::Low,
            session_conflict_key: None,
        }
    }

    pub fn codex(profile: CapabilityProfile) -> Self {
        Self::new(ProtocolFamily::Codex, profile)
    }

    pub fn pi(profile: CapabilityProfile) -> Self {
        Self::new(ProtocolFamily::Pi, profile)
    }

    pub fn with_presence(mut self, presence: SessionPresence) -> Self {
        self.presence = presence;
        self
    }

    pub fn with_knowledge_injected(mut self, injected: bool) -> Self {
        self.knowledge_injected = injected;
        self
    }

    pub fn with_fork(mut self, support: NativeCapabilitySupport) -> Self {
        self.snapshot.fork = support;
        self
    }

    pub fn with_compact(mut self, support: NativeCapabilitySupport) -> Self {
        self.snapshot.compact = support;
        self
    }

    pub fn with_parallel(mut self, support: NativeCapabilitySupport) -> Self {
        self.snapshot.parallel_contexts = support;
        self
    }

    pub fn with_scripted_resume(mut self, outcome: ProtocolOutcome) -> Self {
        self.next_resume = Some(outcome);
        self
    }

    pub fn with_scripted_cancel(mut self, outcome: ProtocolOutcome) -> Self {
        self.next_cancel = Some(outcome);
        self
    }

    pub fn with_session_conflict_key(mut self, key: impl Into<String>) -> Self {
        self.session_conflict_key = Some(key.into());
        self
    }

    pub fn knowledge_injected(&self) -> bool {
        self.knowledge_injected
    }
}

impl NativeProtocolAdapter for HermeticProtocol {
    fn family(&self) -> ProtocolFamily {
        self.family
    }

    fn profile(&self) -> CapabilityProfile {
        self.profile
    }

    fn capabilities(&self) -> NativeCapabilitySnapshot {
        self.snapshot.clone()
    }

    fn methods(&self) -> ProtocolMethods {
        protocol_methods(self.family)
    }

    fn fidelity(&self, child_author: &NativeFidelity) -> NativeFidelity {
        child_author.clone()
    }

    fn isolation(&self) -> IsolationReview {
        if self.knowledge_injected {
            IsolationReview {
                memory: IsolationVerdict::Inherited,
                workspace: IsolationVerdict::Unknown,
                environment_tools: IsolationVerdict::Unknown,
            }
        } else {
            IsolationReview::unknown()
        }
    }

    fn parallel_policy(&self) -> ParallelPolicy {
        match self.snapshot.parallel_contexts {
            NativeCapabilitySupport::Supported => ParallelPolicy::ParallelSessions,
            _ => ParallelPolicy::HonestQueue,
        }
    }

    fn session_presence(&self, _key: &NativeWorkContextKey) -> SessionPresence {
        self.presence
    }

    fn exact_resume(&self, _key: &NativeWorkContextKey) -> ProtocolOutcome {
        if let Some(scripted) = &self.next_resume {
            return scripted.clone();
        }
        let method = self.methods().exact_resume;
        match self.presence {
            SessionPresence::Present => {
                let mut outcome = ProtocolOutcome::applied(method);
                if self.profile == CapabilityProfile::Low {
                    outcome.cache_miss = true;
                }
                outcome
            }
            SessionPresence::Archived if self.family == ProtocolFamily::Codex => {
                ProtocolOutcome::applied(self.methods().unarchive)
            }
            SessionPresence::Archived | SessionPresence::Lost | SessionPresence::Unknown => {
                ProtocolOutcome::failed(method, super::types::native_binding_lost())
            }
        }
    }

    fn start_new(&self, _key: &NativeWorkContextKey) -> ProtocolOutcome {
        ProtocolOutcome::applied(self.methods().start_new)
    }

    fn fork(&self, _key: &NativeWorkContextKey, inheritance: &ForkInheritance) -> ProtocolOutcome {
        match self.snapshot.fork {
            NativeCapabilitySupport::Supported => {
                if inheritance.memory == IsolationVerdict::Unknown && inheritance.is_isolation() {
                    ProtocolOutcome::failed(
                        self.methods().fork,
                        super::types::isolation_unverified(),
                    )
                } else {
                    ProtocolOutcome::applied(self.methods().fork)
                }
            }
            NativeCapabilitySupport::TemporarilyUnavailable => ProtocolOutcome::failed(
                self.methods().fork,
                super::types::reconciliation_required(),
            ),
            _ => ProtocolOutcome::failed(self.methods().fork, super::unsupported_capability()),
        }
    }

    fn compact(&self, _key: &NativeWorkContextKey) -> ProtocolOutcome {
        match self.snapshot.compact {
            NativeCapabilitySupport::Supported => ProtocolOutcome::applied(self.methods().compact),
            NativeCapabilitySupport::Unverified
            | NativeCapabilitySupport::TemporarilyUnavailable => ProtocolOutcome::failed(
                self.methods().compact,
                super::types::reconciliation_required(),
            ),
            NativeCapabilitySupport::Unsupported => {
                ProtocolOutcome::failed(self.methods().compact, super::unsupported_capability())
            }
        }
    }

    fn steer(&self, _request: &NativeControlRequest) -> ProtocolOutcome {
        match self.snapshot.steer {
            NativeCapabilitySupport::Supported => ProtocolOutcome::applied(self.methods().steer),
            _ => ProtocolOutcome::failed(self.methods().steer, super::unsupported_capability()),
        }
    }

    fn cancel(&self, _request: &NativeControlRequest) -> ProtocolOutcome {
        if let Some(scripted) = &self.next_cancel {
            return scripted.clone();
        }
        match self.snapshot.cancel {
            NativeCapabilitySupport::Supported => ProtocolOutcome::applied(self.methods().cancel),
            _ => ProtocolOutcome::failed(self.methods().cancel, super::unsupported_capability()),
        }
    }

    fn session_conflict_key(&self, _key: &NativeWorkContextKey) -> Option<String> {
        self.session_conflict_key.clone()
    }
}
