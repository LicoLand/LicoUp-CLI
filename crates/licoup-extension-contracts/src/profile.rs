//! C09: the five profiles, their method catalogs, and the lifecycle handshake.
//!
//! An extension declares which of the five narrow profiles it serves, in which
//! wire major, and which methods it implements. Nothing else is asked of it. The
//! profiles are narrow on purpose: a universal `call` interface would make every
//! extension responsible for every host feature, and would make "this pack does
//! not do models" indistinguishable from "this pack is broken".
//!
//! A profile decision is local. An extension that declares a profile this host
//! does not know keeps working; an extension whose `agent-execution` major is 2
//! is refused for Agent work and nothing else; an extension that implements
//! `agent.describe` and `agent.execute` but not `agent.event` is refused for
//! Agent work with a refusal that names the missing method. The rest of the
//! client, and every other profile of the same extension, is unaffected.
//!
//! The handshake is three methods and no business call:
//!
//! 1. `extension.initialize` negotiates the host protocol and the profile set.
//! 2. `extension.ready` publishes the fact that the extension is prepared.
//! 3. `extension.shutdown` asks for an ordered exit.
//!
//! An extension with no ordinary business call is never started, so declaring a
//! profile costs nothing until something actually needs it
//! ([`ActivationMode`](licoup_application::ActivationMode)).

use crate::refusal;
use licoup_application::{ApplicationFailure, ContractCompatibility, ContractRange, is_namespaced};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The wire major of every profile this crate publishes.
pub const PROFILE_MAJOR: u32 = 1;

/// The stage every profile decision reports.
const STAGE: &str = "extension/profile";

// The handshake. Required by every profile that is called at all.
pub const METHOD_INITIALIZE: &str = "extension.initialize";
pub const METHOD_READY: &str = "extension.ready";
pub const METHOD_SHUTDOWN: &str = "extension.shutdown";

// C09, Agent execution.
pub const METHOD_AGENT_DESCRIBE: &str = "agent.describe";
pub const METHOD_AGENT_EXECUTE: &str = "agent.execute";
pub const METHOD_AGENT_EVENT: &str = "agent.event";
pub const METHOD_AGENT_CANCEL: &str = "agent.cancel";
pub const METHOD_AGENT_OBSERVE: &str = "agent.observe";
pub const METHOD_AGENT_RESUME: &str = "agent.resume";
pub const METHOD_AGENT_STEER: &str = "agent.steer";
pub const METHOD_AGENT_FORK: &str = "agent.fork";
pub const METHOD_AGENT_RECONCILE: &str = "agent.reconcile";
pub const METHOD_AGENT_HISTORY: &str = "agent.history";
pub const METHOD_AGENT_MODELS: &str = "agent.models";

// C10, model provider.
pub const METHOD_PROVIDER_DESCRIBE: &str = "modelProvider.describe";
pub const METHOD_PROVIDER_MODELS: &str = "modelProvider.models";
pub const METHOD_PROVIDER_STREAM: &str = "modelProvider.stream";
pub const METHOD_PROVIDER_CANCEL: &str = "modelProvider.cancel";
pub const METHOD_PROVIDER_RECONCILE: &str = "modelProvider.reconcile";
pub const METHOD_AUTH_BEGIN: &str = "auth.begin";
pub const METHOD_AUTH_CONTINUE: &str = "auth.continue";
pub const METHOD_AUTH_REFRESH: &str = "auth.refresh";
pub const METHOD_AUTH_REVOKE: &str = "auth.revoke";

// C11, usage and metrics.
pub const METHOD_USAGE_PUBLISH: &str = "usage.publish";
pub const METHOD_USAGE_QUERY: &str = "usage.query";
pub const METHOD_USAGE_DESCRIBE: &str = "usage.describe";

/// The handshake every called profile requires.
pub const BASELINE: &[&str] = &[METHOD_INITIALIZE, METHOD_READY, METHOD_SHUTDOWN];

const AGENT_REQUIRED: &[&str] = &[
    METHOD_INITIALIZE,
    METHOD_READY,
    METHOD_SHUTDOWN,
    METHOD_AGENT_DESCRIBE,
    METHOD_AGENT_EXECUTE,
    METHOD_AGENT_EVENT,
];
const AGENT_OPTIONAL: &[&str] = &[
    METHOD_AGENT_CANCEL,
    METHOD_AGENT_OBSERVE,
    METHOD_AGENT_RESUME,
    METHOD_AGENT_STEER,
    METHOD_AGENT_FORK,
    METHOD_AGENT_RECONCILE,
    METHOD_AGENT_HISTORY,
    METHOD_AGENT_MODELS,
];

const PROVIDER_REQUIRED: &[&str] = &[
    METHOD_INITIALIZE,
    METHOD_READY,
    METHOD_SHUTDOWN,
    METHOD_PROVIDER_DESCRIBE,
    METHOD_PROVIDER_STREAM,
];
const PROVIDER_OPTIONAL: &[&str] = &[
    METHOD_PROVIDER_MODELS,
    METHOD_PROVIDER_CANCEL,
    METHOD_PROVIDER_RECONCILE,
    METHOD_AUTH_BEGIN,
    METHOD_AUTH_CONTINUE,
    METHOD_AUTH_REFRESH,
    METHOD_AUTH_REVOKE,
];

const USAGE_REQUIRED: &[&str] = &[METHOD_INITIALIZE, METHOD_READY, METHOD_SHUTDOWN];
/// A usage source chooses its direction. Concrete providers push, a query-only
/// source is pulled, and neither shape is the right one to impose on the other.
const USAGE_ANY_OF: &[&[&str]] = &[&[METHOD_USAGE_PUBLISH, METHOD_USAGE_QUERY]];
const USAGE_OPTIONAL: &[&str] = &[METHOD_USAGE_DESCRIBE];

const NONE: &[&str] = &[];
const NO_ALTERNATIVES: &[&[&str]] = &[];

/// One of the five profiles the extension platform publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExtensionProfile {
    /// C09: extension lifecycle and Agent execution.
    AgentExecution,
    /// C10: a model provider — protocol, authentication, catalog and stream.
    ModelProvider,
    /// C11: a usage and metric source.
    UsageMetric,
    /// C12: packages and the deployment closure.
    PackageDeployment,
    /// C13: bounded declarative interface contributions.
    DeclarativeUi,
}

impl ExtensionProfile {
    /// Every published profile, in contract order.
    pub const ALL: [Self; 5] = [
        Self::AgentExecution,
        Self::ModelProvider,
        Self::UsageMetric,
        Self::PackageDeployment,
        Self::DeclarativeUi,
    ];

    /// The stable id an extension writes in its manifest.
    pub const fn id(self) -> &'static str {
        match self {
            Self::AgentExecution => "agent-execution",
            Self::ModelProvider => "model-provider",
            Self::UsageMetric => "usage-metric",
            Self::PackageDeployment => "package-deployment",
            Self::DeclarativeUi => "declarative-ui",
        }
    }

    /// The contract this profile makes concrete.
    pub const fn contract(self) -> &'static str {
        match self {
            Self::AgentExecution => "C09",
            Self::ModelProvider => "C10",
            Self::UsageMetric => "C11",
            Self::PackageDeployment => "C12",
            Self::DeclarativeUi => "C13",
        }
    }

    /// The profile this id names, or `None` for an id published by a newer host.
    ///
    /// `None` is not an error: an unknown profile is preserved and ignored, and
    /// it refuses nothing.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.id() == id)
    }

    /// This profile's method catalog.
    pub const fn contract_profile(self) -> ProfileContract {
        match self {
            Self::AgentExecution => ProfileContract {
                profile: self,
                required: AGENT_REQUIRED,
                required_any_of: NO_ALTERNATIVES,
                optional: AGENT_OPTIONAL,
            },
            Self::ModelProvider => ProfileContract {
                profile: self,
                required: PROVIDER_REQUIRED,
                required_any_of: NO_ALTERNATIVES,
                optional: PROVIDER_OPTIONAL,
            },
            Self::UsageMetric => ProfileContract {
                profile: self,
                required: USAGE_REQUIRED,
                required_any_of: USAGE_ANY_OF,
                optional: USAGE_OPTIONAL,
            },
            // C12 and C13 carry declarations, not calls: package facts and UI
            // contributions are data the host reads. A pack that only
            // contributes a settings form is never started at all, which is why
            // it requires no method and cannot fail a method check.
            Self::PackageDeployment | Self::DeclarativeUi => ProfileContract {
                profile: self,
                required: NONE,
                required_any_of: NO_ALTERNATIVES,
                optional: NONE,
            },
        }
    }
}

impl std::fmt::Display for ExtensionProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.id())
    }
}

/// What one profile requires of an extension that claims to serve it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileContract {
    pub profile: ExtensionProfile,
    /// Methods the extension must implement, handshake included.
    pub required: &'static [&'static str],
    /// Groups where implementing *one* member is enough.
    pub required_any_of: &'static [&'static [&'static str]],
    /// Methods that extend the profile when present and cost nothing when
    /// absent. Absence here is never a refusal.
    pub optional: &'static [&'static str],
}

impl ProfileContract {
    /// The contract id, for a refusal or a log line.
    pub const fn contract(&self) -> &'static str {
        self.profile.contract()
    }

    /// Every method this profile names, in catalog order.
    pub fn methods(&self) -> impl Iterator<Item = &'static str> {
        self.required
            .iter()
            .copied()
            .chain(
                self.required_any_of
                    .iter()
                    .flat_map(|group| group.iter().copied()),
            )
            .chain(self.optional.iter().copied())
    }

    /// Decide this profile against the methods one extension implements.
    pub fn check(&self, declared: &DeclaredMethods) -> ProfileAvailability {
        let missing: Vec<&'static str> = self
            .required
            .iter()
            .copied()
            .filter(|method| !declared.implements(method))
            .collect();
        if !missing.is_empty() {
            return ProfileAvailability::MissingRequired { missing };
        }
        let absent: Vec<&'static [&'static str]> = self
            .required_any_of
            .iter()
            .copied()
            .filter(|group| !group.iter().any(|method| declared.implements(method)))
            .collect();
        if !absent.is_empty() {
            return ProfileAvailability::MissingAlternative { groups: absent };
        }
        ProfileAvailability::Available
    }
}

/// The methods one extension says it implements.
///
/// This is a claim, like a capability descriptor, and the host still decides
/// whether the process actually answers. It exists so the *shape* of the claim
/// can be checked before anything is started.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeclaredMethods {
    names: BTreeSet<String>,
}

impl DeclaredMethods {
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }

    pub fn implements(&self, method: &str) -> bool {
        self.names.contains(method)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Declare exactly the baseline profile of a specialist that only streams
    /// text: the handshake plus `describe`, `execute` and `event`.
    ///
    /// This is the whole requirement for the smallest useful Agent, and it is
    /// named here so an SDK author does not have to reconstruct it from the
    /// catalog.
    pub fn minimal_agent() -> Self {
        Self::new(AGENT_REQUIRED.iter().copied())
    }
}

/// Whether one profile can be served by an extension, and if not, what is
/// missing.
///
/// Every non-available outcome is a fact about *this profile*: the extension's
/// other profiles are decided separately, and the client keeps running.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileAvailability {
    Available,
    MissingRequired {
        missing: Vec<&'static str>,
    },
    MissingAlternative {
        groups: Vec<&'static [&'static str]>,
    },
}

impl ProfileAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// The refusal for this profile, or `None` when it can be served.
    ///
    /// The recovery is actionable — install or enable a pack that implements the
    /// missing method — rather than being reported as a malformed request.
    pub fn refusal(
        &self,
        plugin_id: &str,
        profile: ExtensionProfile,
    ) -> Option<ApplicationFailure> {
        let field = match self {
            Self::Available => return None,
            Self::MissingRequired { missing } => missing.join(","),
            Self::MissingAlternative { groups } => groups
                .iter()
                .filter_map(|group| group.first().copied())
                .collect::<Vec<_>>()
                .join(","),
        };
        Some(
            refusal::actionable("extension_profile_unavailable", STAGE, &field)
                .with_presentation_arg("pluginId", plugin_id)
                .with_presentation_arg("profile", profile.id()),
        )
    }
}

/// The outcome of deciding one declared profile against this host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileStatus {
    /// The id is not one of the five published profiles. Preserved and ignored:
    /// it refuses nothing and grants nothing.
    Unpublished,
    /// A different wire major. Only this profile is refused.
    MajorMismatch,
    /// The same major, but the extension needs features from a newer host minor.
    RequiresNewerMinor,
    /// Version fits; this profile's own operations can be served.
    Available,
    /// Version fits; this profile's own operations are refused.
    MissingMethods { missing: Vec<&'static str> },
    /// Version fits; the profile needs one of several alternative methods and
    /// implements none.
    MissingAlternative {
        groups: Vec<&'static [&'static str]>,
    },
}

impl ProfileStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// The local refusal this status produces, or `None` when the profile is
    /// available or unpublished.
    ///
    /// An unpublished profile produces no refusal on purpose: a newer extension
    /// may declare a profile this host has never heard of, and the host keeps the
    /// declaration without acting on it.
    pub fn refusal(
        &self,
        plugin_id: &str,
        profile: ExtensionProfile,
    ) -> Option<ApplicationFailure> {
        match self {
            Self::Unpublished | Self::Available => None,
            Self::MajorMismatch => Some(profile_refusal(
                "extension_profile_major_mismatch",
                plugin_id,
                profile,
                profile.id(),
            )),
            Self::RequiresNewerMinor => Some(profile_refusal(
                "extension_profile_minor_unavailable",
                plugin_id,
                profile,
                profile.id(),
            )),
            Self::MissingMethods { missing } => Some(profile_refusal(
                "extension_profile_unavailable",
                plugin_id,
                profile,
                &missing.join(","),
            )),
            Self::MissingAlternative { groups } => Some(profile_refusal(
                "extension_profile_unavailable",
                plugin_id,
                profile,
                &groups
                    .iter()
                    .filter_map(|group| group.first().copied())
                    .collect::<Vec<_>>()
                    .join(","),
            )),
        }
    }
}

fn profile_refusal(
    code: &str,
    plugin_id: &str,
    profile: ExtensionProfile,
    field: &str,
) -> ApplicationFailure {
    refusal::actionable(code, STAGE, field)
        .with_presentation_arg("pluginId", plugin_id)
        .with_presentation_arg("profile", profile.id())
}

/// One profile an extension declares, in the manifest's own vocabulary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDeclaration {
    pub id: String,
    pub major: u32,
    /// Namespaced capabilities this profile contributes.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl ProfileDeclaration {
    pub fn new(id: impl Into<String>, major: u32) -> Self {
        Self {
            id: id.into(),
            major,
            capabilities: Vec::new(),
        }
    }

    pub fn with_capabilities<S: Into<String>>(
        mut self,
        capabilities: impl IntoIterator<Item = S>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    /// The published profile this id names, if any.
    pub fn profile(&self) -> Option<ExtensionProfile> {
        ExtensionProfile::from_id(&self.id)
    }

    /// Structural validation: the profile majors are majors, and every
    /// capability is namespaced.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.id.is_empty() || self.major < 1 {
            return Err(refusal::new("extension_profile_invalid", STAGE).with_field("profiles.id"));
        }
        for capability in &self.capabilities {
            if !is_namespaced(capability) {
                return Err(refusal::new("extension_profile_invalid", STAGE)
                    .with_field("profiles.capabilities"));
            }
        }
        Ok(())
    }

    /// Decide this declaration against the host's contract range and the methods
    /// the extension implements.
    ///
    /// The version decision comes first because it is cheaper and because a
    /// major mismatch makes the method catalog meaningless: those methods may not
    /// exist in the other major at all.
    pub fn status(&self, host: ContractRange, methods: &DeclaredMethods) -> ProfileStatus {
        let Some(profile) = self.profile() else {
            return ProfileStatus::Unpublished;
        };
        let range = ContractRange {
            major: self.major,
            minimum_minor: 0,
        };
        match range.negotiate(host) {
            ContractCompatibility::MajorMismatch => ProfileStatus::MajorMismatch,
            ContractCompatibility::RequiresNewerMinor => ProfileStatus::RequiresNewerMinor,
            ContractCompatibility::Compatible => match profile.contract_profile().check(methods) {
                ProfileAvailability::Available => ProfileStatus::Available,
                ProfileAvailability::MissingRequired { missing } => {
                    ProfileStatus::MissingMethods { missing }
                }
                ProfileAvailability::MissingAlternative { groups } => {
                    ProfileStatus::MissingAlternative { groups }
                }
            },
        }
    }
}

/// Every published profile contract, in contract order.
pub fn all_contracts() -> [ProfileContract; 5] {
    ExtensionProfile::ALL.map(ExtensionProfile::contract_profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_sources_may_push_or_pull_without_implementing_both() {
        let contract = ExtensionProfile::UsageMetric.contract_profile();
        for direction in [METHOD_USAGE_PUBLISH, METHOD_USAGE_QUERY] {
            let methods = DeclaredMethods::new(BASELINE.iter().copied().chain([direction]));
            assert!(contract.check(&methods).is_available());
            assert!(contract.methods().any(|method| method == direction));
        }
        assert!(
            !contract
                .check(&DeclaredMethods::new(BASELINE.iter().copied()))
                .is_available()
        );
    }

    #[test]
    fn profile_versions_and_missing_methods_refuse_only_their_profile() {
        let host = ContractRange {
            major: 1,
            minimum_minor: 0,
        };
        let methods = DeclaredMethods::minimal_agent();
        assert_eq!(
            ProfileDeclaration::new("agent-execution", 1).status(host, &methods),
            ProfileStatus::Available
        );
        assert_eq!(
            ProfileDeclaration::new("agent-execution", 2).status(host, &methods),
            ProfileStatus::MajorMismatch
        );
        assert_eq!(
            ProfileDeclaration::new("future-profile", 1).status(host, &methods),
            ProfileStatus::Unpublished
        );
        assert!(matches!(
            ProfileDeclaration::new("model-provider", 1).status(host, &methods),
            ProfileStatus::MissingMethods { .. }
        ));
        assert_eq!(
            ProfileDeclaration::new("declarative-ui", 1).status(host, &methods),
            ProfileStatus::Available
        );
    }
}
