//! The published Copilot card and mechanism dialect, as this package compiles.
//!
//! `a4aa-copilot` owns the Copilot mechanism surface: the agent card, the ACP
//! launch command, the mechanism locations (rules, skills, subagents, MCP) and
//! the adaptation plan. `build.rs` reads that published declaration and emits
//! the constants below, so the launch and mechanism facts cannot drift into a
//! second hand-maintained copy.
//!
//! The dependency sits in the build graph because `a4aa` links the ACP runtime,
//! whose dependency features turn on `serde_json/preserve_order` globally. That
//! would change every JSON object ordering in the client; the build-time bridge
//! keeps the runtime graph (and its ordering contract) unchanged while still
//! sourcing the facts from the published crate.
//!
//! What stays local is the part the upstream card deliberately does not own:
//! the ACP frame parser and its dialect registration, the launch error
//! vocabulary, and the LicoUp package manifest. a4aa's `Dialect` maps mechanism
//! artifacts; it does not parse a transport frame.

include!(concat!(env!("OUT_DIR"), "/copilot_card.rs"));

/// The published card's agent identity.
pub const CARD_ID: &str = UPSTREAM_CARD_ID;
/// The published card's display label.
pub const CARD_LABEL: &str = UPSTREAM_CARD_LABEL;

/// The program and arguments the published card materializes for its primary
/// lane. The host resolves `program` through its normal binary discovery and
/// passes the discovered executable to the shared ACP engine.
pub fn acp_launch() -> (&'static str, &'static [&'static str]) {
    (UPSTREAM_PROGRAM, UPSTREAM_LAUNCH_ARGS)
}

/// The upstream launch arguments, as the shared ACP engine's metadata.
pub fn launch_args() -> &'static [&'static str] {
    UPSTREAM_LAUNCH_ARGS
}

/// Whether the published card's primary lane is ACP.
pub fn is_acp() -> bool {
    UPSTREAM_SURFACE_IS_ACP
}

/// The mechanism locations the published dialect declares.
///
/// They are exposed as facts rather than a copied `Dialect`: LicoUp's host
/// composes them where a mechanism applies, and a `None` is reported as the
/// upstream's own "this surface is unadapted" answer, never guessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MechanismFacts {
    /// Whether the agent stores lifecycle hooks in a shape a4aa could card.
    pub hooks_adapted: bool,
    /// The home-relative skills directory.
    pub skills_dir: Option<&'static str>,
    /// The project-relative subagent directory.
    pub subagents_dir: Option<&'static str>,
    /// The project-relative standing-rules file.
    pub rules_file: Option<&'static str>,
    /// The home-relative MCP document.
    pub mcp_file: Option<&'static str>,
    /// Whether the MCP store is a translation rather than a native shape.
    pub mcp_adapted: bool,
}

/// The mechanism facts of the published Copilot dialect.
pub const MECHANISM: MechanismFacts = MechanismFacts {
    hooks_adapted: !UPSTREAM_HOOKS_NONE,
    skills_dir: UPSTREAM_SKILLS_DIR,
    subagents_dir: UPSTREAM_SUBAGENTS_DIR,
    rules_file: UPSTREAM_RULES_FILE,
    mcp_file: UPSTREAM_MCP_FILE,
    mcp_adapted: UPSTREAM_MCP_ADAPTED,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_card_materializes_the_acp_lane() {
        assert_eq!(CARD_ID, "copilot");
        assert_eq!(CARD_LABEL, "GitHub Copilot");
        assert_eq!(acp_launch().0, "copilot");
        assert_eq!(
            acp_launch().1,
            ["--acp", "--stdio", "--no-auto-update"],
            "the card remains the launch authority"
        );
        assert_eq!(launch_args(), acp_launch().1);
        assert!(is_acp());
    }

    #[test]
    fn the_published_dialect_declares_the_mechanism_surface_without_overclaiming() {
        assert_eq!(MECHANISM.skills_dir, Some(".github/skills"));
        assert_eq!(MECHANISM.subagents_dir, Some(".github/agents"));
        assert_eq!(MECHANISM.rules_file, Some("AGENTS.md"));
        assert_eq!(MECHANISM.mcp_file, Some(".copilot/mcp-config.json"));
        assert!(!MECHANISM.mcp_adapted);
        assert!(
            !MECHANISM.hooks_adapted,
            "the bash-shaped hook store stays unadapted upstream"
        );
    }
}
