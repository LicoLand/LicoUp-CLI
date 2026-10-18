//! The environment port conversation history asks for Codex runtime evidence.
//!
//! Codex `thread/list` reports another app-server's threads as `notLoaded`, so
//! conversation history cannot learn from the protocol whether another Codex
//! process is still running a turn. It asks this port instead of inspecting
//! processes itself: the composition above installs the answer this host can
//! truthfully give, and a program that installs none gets the fail-closed
//! answer rather than a fabricated `running` fact.
//!
//! The port is a value of one `fn` pointer, so history keeps no state the
//! composition did not hand it. A LicoUp-owned turn is still projected by the
//! client controller, so an empty answer never hides this application's work.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Answers which Codex rollout files a running Codex app-server holds open.
pub type OpenCodexRollouts = fn() -> BTreeSet<PathBuf>;

/// The fail-closed answer: no rollout is observed as open.
pub fn no_open_codex_rollouts() -> BTreeSet<PathBuf> {
    BTreeSet::new()
}

/// One process's installed answer.
///
/// The production program has exactly one; tests build their own so the
/// fail-closed default and an installed answer are both observable without
/// depending on test order.
pub struct CodexRolloutPort {
    answer: OnceLock<OpenCodexRollouts>,
}

impl CodexRolloutPort {
    pub const fn new() -> Self {
        Self {
            answer: OnceLock::new(),
        }
    }

    /// Install the composition's answer. One answer per port: a second
    /// installation is refused rather than silently replacing the first.
    pub fn install(&self, answer: OpenCodexRollouts) -> Result<(), &'static str> {
        self.answer
            .set(answer)
            .map_err(|_| "codex rollout observation is already installed")
    }

    /// The installed answer, or the fail-closed one.
    pub fn open_rollouts(&self) -> BTreeSet<PathBuf> {
        self.answer.get().copied().unwrap_or(no_open_codex_rollouts)()
    }
}

impl Default for CodexRolloutPort {
    fn default() -> Self {
        Self::new()
    }
}

static PORT: CodexRolloutPort = CodexRolloutPort::new();

/// The process-wide port the history owner reads.
pub fn port() -> &'static CodexRolloutPort {
    &PORT
}

/// Install the composition's answer for this process.
pub fn install_open_codex_rollouts(answer: OpenCodexRollouts) -> Result<(), &'static str> {
    PORT.install(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_rollout() -> BTreeSet<PathBuf> {
        BTreeSet::from([PathBuf::from("/fixture/sessions/rollout-thread.jsonl")])
    }

    #[test]
    fn an_unanswered_port_reports_no_open_rollouts() {
        let port = CodexRolloutPort::new();
        assert!(port.open_rollouts().is_empty());
    }

    #[test]
    fn an_installed_answer_is_the_only_answer_the_port_gives() {
        let port = CodexRolloutPort::new();
        port.install(one_rollout).unwrap();
        assert_eq!(port.open_rollouts(), one_rollout());
        assert!(port.install(no_open_codex_rollouts).is_err());
        assert_eq!(port.open_rollouts(), one_rollout());
    }
}
