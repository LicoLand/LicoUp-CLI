//! Atomic subagent admission and continuity capability leaves.
//!
//! Subagents require that Member/Profile admission and cwd bind atomically,
//! leaving no half member upon failure.
//!
//! Native fork, resume, and rehydrate are distinct real capabilities;
//! rebuild must not pretend to be exact restore.

use super::{SubagentError, permanent, workspace::SubagentWorkspaceIsolation};
use licoup_agent_runtime::SubagentCapabilities;
use serde::{Deserialize, Serialize};

/// Atomic admission request for a subagent seat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentAdmissionRequest {
    pub conversation_id: String,
    pub caller_membership_id: String,
    pub agent_id: String,
    pub preferred_model: Option<String>,
    pub preferred_reasoning_effort: Option<String>,
    pub preferred_permission_mode: Option<String>,
    pub working_directory: Option<String>,
}

impl SubagentAdmissionRequest {
    pub fn validate(&self) -> Result<(), SubagentError> {
        if self.conversation_id.trim().is_empty()
            || self.caller_membership_id.trim().is_empty()
            || self.agent_id.trim().is_empty()
        {
            return Err(permanent("invalid_request", "schema/validate"));
        }
        if let Some(cwd) = &self.working_directory {
            let _ = SubagentWorkspaceIsolation::new(cwd)?;
        }
        Ok(())
    }
}

/// Distinct continuity modes for subagents.
///
/// Outcome: "Native fork, resume, rehydrate are distinct real capabilities; rebuild must not pretend to be exact restore."
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentContinuityMode {
    /// Exact resume of an existing native session ID and bound working directory.
    ExactResume,
    /// Native branch/fork from an existing session state into a new child session.
    Fork,
    /// Explicit rehydration of context from conversation history after lost session.
    Rehydrate,
    /// Rebuilding a fresh session (clean start; not an exact restore).
    Rebuild,
}

impl SubagentContinuityMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactResume => "exact-resume",
            Self::Fork => "fork",
            Self::Rehydrate => "rehydrate",
            Self::Rebuild => "rebuild",
        }
    }

    /// Whether this mode guarantees exact identity restore of the prior native session.
    /// Rebuild and Rehydrate must NOT claim to be exact restore.
    pub const fn is_exact_restore(self) -> bool {
        matches!(self, Self::ExactResume)
    }
}

/// Extended native continuity capabilities descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentExtendedCapabilities {
    pub exact_resume: bool,
    pub fork: bool,
    pub rehydrate: bool,
    pub rebuild: bool,
}

impl SubagentExtendedCapabilities {
    pub fn from_runtime_capabilities(
        base: &SubagentCapabilities,
        supports_fork: bool,
        supports_rehydrate: bool,
    ) -> Self {
        Self {
            exact_resume: base.exact_resume && base.continue_turn,
            fork: supports_fork,
            rehydrate: supports_rehydrate,
            rebuild: base.create,
        }
    }

    /// Validates whether the requested continuity mode is supported.
    pub fn validate_mode(&self, mode: SubagentContinuityMode) -> Result<(), SubagentError> {
        match mode {
            SubagentContinuityMode::ExactResume => {
                if !self.exact_resume {
                    return Err(permanent(
                        "subagent_exact_resume_unavailable",
                        "capability/admit",
                    ));
                }
            }
            SubagentContinuityMode::Fork => {
                if !self.fork {
                    return Err(permanent("subagent_fork_unavailable", "capability/admit"));
                }
            }
            SubagentContinuityMode::Rehydrate => {
                if !self.rehydrate {
                    return Err(permanent(
                        "subagent_rehydrate_unavailable",
                        "capability/admit",
                    ));
                }
            }
            SubagentContinuityMode::Rebuild => {
                if !self.rebuild {
                    return Err(permanent(
                        "subagent_rebuild_unavailable",
                        "capability/admit",
                    ));
                }
            }
        }
        Ok(())
    }
}
