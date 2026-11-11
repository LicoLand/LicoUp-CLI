//! One replay arm per Agent parser this host composes.
//!
//! Each arm is the only place that knows how to construct that Agent's real
//! parser. An Agent whose parser has moved into its own package also moved the
//! arm that drives it, because an arm is only meaningful beside the parser it
//! constructs; this composition reaches it through the SDK's parser-set port.
//! Thirteen Agents' arms have moved that way — Antigravity, Claude Code, Codex,
//! Copilot, Cursor, DeepSeek Harness, Hermes, Kilo Code, Kimi Code, Lico Agent,
//! OpenClaw, OpenCode and Pi — so each is built by the package that owns its
//! parser.
//!
//! The arms are `pub(in crate::platform)` to this module's parent — it is the
//! only reader, and it hands them to the SDK's harness through the parser set.

use super::FrameReplay;

/// Build the replay arm for one Agent parser this host composes.
///
/// An adapter this host composes no arm for is refused rather than defaulted,
/// so a fixture can never pass against a parser that was never constructed.
pub(in crate::platform) fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    Ok(match adapter_id {
        "antigravity" => licoup_agent_antigravity::replay::replay_arm(adapter_id)?,
        "claude-code" => licoup_agent_claude_code::replay::replay_arm(adapter_id)?,
        "codex" => licoup_agent_codex::replay::replay_arm(adapter_id)?,
        "copilot" => licoup_agent_copilot::replay::replay_arm(adapter_id)?,
        "cursor" => licoup_agent_cursor::replay::replay_arm(adapter_id)?,
        "deepseek-harness" => licoup_agent_deepseek::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the Hermes adapter package, so a
        // regression in that parser fails the package's own corpus as well as
        // this composition's.
        "hermes" => licoup_agent_hermes::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the Kilo Code adapter package, so a
        // regression in that parser fails the package's own corpus as well as
        // this composition's.
        "kilo-code" => licoup_agent_kilo::replay::replay_arm(adapter_id)?,
        "kimi-code" => licoup_agent_kimi::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the Lico Agent adapter package, so a
        // regression in that parser fails the package's own corpus as well as
        // this composition's.
        "lico-agent" => licoup_agent_lico_agent::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the OpenClaw adapter package, so a
        // regression in that parser fails the package's own corpus as well as
        // this composition's.
        "openclaw" => licoup_agent_openclaw::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the OpenCode adapter package, so a
        // regression in that protocol fails the package's own corpus as well as
        // this composition's.
        "opencode" => licoup_agent_opencode::replay::replay_arm(adapter_id)?,
        // The arm moved with the parser into the Pi adapter package, so a
        // regression in that protocol fails the package's own corpus as well as
        // this composition's.
        "pi" => licoup_agent_pi::replay::replay_arm(adapter_id)?,
        other => {
            return Err(format!(
                "no replayable parser is registered for adapter {other}"
            ));
        }
    })
}
