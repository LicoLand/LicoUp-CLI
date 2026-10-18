//! The in-flight answer this owner asks for instead of inventing.
//!
//! A change preview has to say what a change would touch *and* whether an
//! already-started run or an already-accepted result sits behind the
//! declaration it replaces. This crate owns neither fact: runs and acceptance
//! belong to the existing work owner. So the question is declared here, the way
//! [`crate::ProjectAuthorityDirectory`] declares the authorization question, and
//! the composition answers it from the owner that already knows.
//!
//! The trait exists so a contract can never be reported fresh by the module that
//! would profit from saying so. A work item whose owner does not answer is
//! [`WorkActivity::Unknown`], which a preview reports as unresolved rather than
//! treating as "nothing is in flight".

use crate::dependency::WorkRef;
use serde::{Deserialize, Serialize};

/// What one existing work owner knows about one work item.
///
/// Three answers and one refusal: the work item has not started, it has a run in
/// flight, an accepted result exists, or the owner cannot answer. There is no
/// variant that guesses.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkActivity {
    /// The owner holds no run and no accepted result for this work item.
    NotStarted,
    /// The owner holds a run that has not settled.
    InFlight,
    /// The owner holds an accepted result for the work item's current
    /// contract.
    Accepted,
    /// The owner cannot answer for this work item, or no owner answered at all.
    Unknown,
}

impl WorkActivity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not-started",
            Self::InFlight => "in-flight",
            Self::Accepted => "accepted",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "not-started" => Some(Self::NotStarted),
            "in-flight" => Some(Self::InFlight),
            "accepted" => Some(Self::Accepted),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Decide what one existing work owner knows about one work item.
///
/// Implementations answer from the owner that holds run and acceptance facts:
/// the durable workflow runtime in the running client, and the synthetic owner a
/// test composes. The answer is read for one work item, so a preview never
/// receives a whole-project status that could hide a single in-flight run.
pub trait WorkActivityDirectory: Send + Sync {
    fn activity(&self, work: &WorkRef) -> WorkActivity;
}

/// A work owner that answers nothing.
///
/// This is the fail-closed answer for a process that has not composed the work
/// owner yet: every affected work item is reported as
/// [`WorkActivity::Unknown`], so a preview never calls a contract fresh because
/// nobody was asked.
pub struct NoWorkActivityDirectory;

impl WorkActivityDirectory for NoWorkActivityDirectory {
    fn activity(&self, _work: &WorkRef) -> WorkActivity {
        WorkActivity::Unknown
    }
}
