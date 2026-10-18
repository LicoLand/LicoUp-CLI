//! The native Agent parser family: the per-Agent parsers this host still holds,
//! and the composition that injects them — and the Agent packages that own the
//! moved ones — into the adapter SDK.
//!
//! `licoup-agent-adapter-sdk` is the single authority for what every adapter
//! program shares: the byte-line ingress contract, the adapter declaration, the
//! closed transition vocabulary and its arrival-ordered reducer, the
//! delta/cumulative text reconciliation, the process-local driver registry, the
//! registry lookup, the recorded-transcript replay harness and the parser
//! lifecycle machine. None of that is here any more, and none of it is
//! duplicated here.
//!
//! What is here is one Agent each. `adapters` holds the composition's own
//! declarations — the `REGISTRATIONS` list that names every Agent's declaration,
//! all thirteen of them the moved packages' own registrations; `replay` holds
//! the arm that drives each of them from a
//! recorded transcript and the corpus checks that belong to the family; and
//! `tests` holds the family's own claims. Each per-Agent subtree and its arm
//! move to that Agent's crate (`licoup-agent-<agent>`); this root and the
//! composition are what remain, because they are what names thirteen parsers
//! and, later, thirteen crates. Thirteen subtrees have moved — Antigravity's,
//! Claude Code's, Codex's, Copilot's, Cursor's, DeepSeek Harness', Hermes',
//! Kilo Code's, Kimi Code's, Lico Agent's, OpenClaw's, OpenCode's and Pi's:
//! their parsers are reached through the packages that own them and no copy
//! stays here.
//!
//! The SDK's two protocol-agnostic `port::ParserRegistration` queries are
//! answered in `adapters::REGISTRATIONS` by the Agents whose facts a reader
//! actually reaches: Hermes answers `execution_transitions` from its own
//! package, because it reports no transition list of its own and the host's
//! Hermes normalization reads that query; the Agents the Subagent mesh
//! dispatches — Antigravity, Claude Code, Codex and Cursor from their own
//! packages — answer `valid_identity` from their own recorded evidence; and
//! a package entry answers both queries from the package's wire vocabulary.
//! Every other entry leaves `execution_transitions` unanswered because its
//! driver carries the parser's own transition list, and the identity query stays
//! fail-closed for an Agent the mesh never dispatches. No entry inherits a
//! neighbouring Agent's answer.

pub(in crate::platform) mod adapters;
#[cfg(test)]
pub(in crate::platform) mod replay;
#[cfg(test)]
mod tests;

/// Complete packaged inventory. The registry test proves this is bijective
/// with `RuntimeAdapter`; adding an adapter requires adding its parser here.
#[cfg(test)]
const PACKAGED_ADAPTER_IDS: [&str; 13] = [
    "antigravity",
    "claude-code",
    "codex",
    "copilot",
    "cursor",
    "hermes",
    "kilo-code",
    "kimi-code",
    "openclaw",
    "opencode",
    "pi",
    "lico-agent",
    "deepseek-harness",
];

/// The parser set this host injects into the adapter SDK.
///
/// The declarations live in `adapters`, in the one list that names all
/// thirteen parsers — the eleven this tree still holds and the two their
/// packages own; this is the composition's name for the set, so a caller
/// outside the family never names a parser to reach the SDK.
pub(in crate::platform) const fn parser_set() -> licoup_agent_adapter_sdk::port::AdapterParserSet {
    adapters::parser_set()
}
