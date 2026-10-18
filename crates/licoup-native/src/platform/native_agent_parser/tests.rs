//! The family's own claims.
//!
//! A claim that names an Agent — which parsers exist, which declaration each
//! one reports, what one Agent's parser does with a vendor frame — belongs here
//! where both halves are in view: the parsers in `adapters` and the dispatch
//! enum in `runtime_adapters`. The claims that name no Agent live in
//! `licoup-agent-adapter-sdk`'s own test module.

use super::*;
use crate::platform::runtime_adapters::RuntimeAdapter;
use licoup_agent_adapter_sdk::adapters::ProtocolSignalKind;

const ALL: [RuntimeAdapter; 13] = [
    RuntimeAdapter::Antigravity,
    RuntimeAdapter::ClaudeCode,
    RuntimeAdapter::Codex,
    RuntimeAdapter::Copilot,
    RuntimeAdapter::Cursor,
    RuntimeAdapter::Hermes,
    RuntimeAdapter::KiloCode,
    RuntimeAdapter::KimiCode,
    RuntimeAdapter::OpenClaw,
    RuntimeAdapter::OpenCode,
    RuntimeAdapter::Pi,
    RuntimeAdapter::LicoAgent,
    RuntimeAdapter::DeepSeekHarness,
];

/// The iron rule: one Agent, one declaration, one parser. The dispatch enum and
/// the parser set this host composes are the same thirteen ids, in the packaged
/// order, and each reports the id the enum names.
///
/// This read the crate's own `registry::parser_for` before the parsers became
/// an injected set. It reads the composed set now, through the SDK's
/// string-keyed registry lookup, which is what the claim was always about.
#[test]
fn native_agent_parser_registry_is_bijective_with_packaged_inventory() {
    let set = parser_set();
    let registered: Vec<&'static str> = ALL
        .map(|adapter| {
            licoup_agent_adapter_sdk::registry::parser_for(&set, adapter.id())
                .expect("every dispatched adapter must have a composed parser")
                .id
        })
        .to_vec();
    assert_eq!(registered, PACKAGED_ADAPTER_IDS);
    for adapter in ALL {
        let contract = licoup_agent_adapter_sdk::registry::parser_for(&set, adapter.id())
            .expect("every dispatched adapter must have a composed parser");
        assert!(!contract.framing.is_empty());
        assert_eq!(contract.inventory_json()["adapterId"], adapter.id());
    }
}

/// The parser set the SDK reads is the one this host composes, so the replay
/// arms, the registry and the corpus coverage cannot drift apart.
#[test]
fn native_agent_parser_composes_one_registration_per_packaged_adapter() {
    let set = parser_set();
    assert_eq!(set.registered_ids(), PACKAGED_ADAPTER_IDS);
    assert_eq!(set.all().len(), ALL.len());
}

/// Every composed parser's declaration reports the complete L4 signal set and
/// settles no turn. The conversation layer remains the sole turn authority.
#[test]
fn packaged_adapters_report_l4_facts_without_settling_turns() {
    for adapter in ALL {
        let contract = licoup_agent_adapter_sdk::registry::parser_for(&parser_set(), adapter.id())
            .expect("every dispatched adapter must have a composed parser");
        assert!(!contract.settles_turn, "{} settled a turn", contract.id);
        assert!(
            !contract.has_implicit_turn_timeout,
            "{} declared an implicit turn timeout",
            contract.id
        );
        assert!(contract.emits_all_content, "{} hid content", contract.id);
        assert_eq!(
            contract.reported_signals,
            [
                ProtocolSignalKind::ProtocolFinish,
                ProtocolSignalKind::Eof,
                ProtocolSignalKind::CancelConfirmed,
            ],
            "{} did not report the complete L4 signal set",
            contract.id
        );
    }
}

/// The two protocol-agnostic queries are declared by the SDK and composed here.
/// Until an Agent's crate owns that Agent's answer, the entry answers
/// fail-closed rather than inheriting another Agent's answer.
#[test]
fn native_agent_parser_registrations_declare_their_agent() {
    for registration in parser_set().all() {
        assert!(
            PACKAGED_ADAPTER_IDS.contains(&registration.contract.id),
            "{} is not a packaged adapter",
            registration.contract.id
        );
    }
}

/// The replay arms and the registry are one set: an id in `REGISTRATIONS` with
/// no arm would make a corpus check impossible to run at all.
#[test]
fn native_agent_parser_composes_a_replay_arm_for_every_registration() {
    for adapter in ALL {
        let arm = replay::adapters::replay_arm(adapter.id());
        assert!(
            arm.is_ok(),
            "{} has no composed replay arm: {:?}",
            adapter.id(),
            arm.err()
        );
    }
}

#[test]
fn cursor_parser_streams_only_assistant_frames_and_completes_with_cumulative_result() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new(
        "synthetic-session",
        "exact user prompt",
        EffectiveSettings::default(),
    );
    let init = parser
        .parse_line(br#"{"type":"system","subtype":"init","apiKeySource":"synthetic","cwd":"/tmp","session_id":"synthetic-session","model":"fake-model","permissionMode":"default"}"#)
        .unwrap();
    assert!(init.is_empty());
    // Only the exact user prompt echo acknowledges native delivery, and it
    // must never become assistant output.
    let echo = parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"exact user prompt"}]}}"#)
        .unwrap();
    assert!(
        echo.iter()
            .all(|effect| !matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(echo.iter().any(|effect| matches!(
        effect,
        CursorEffect::Accepted { session_id, turn_id }
            if session_id == "synthetic-session" && turn_id == "cursor-turn"
    )));
    let first = parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","timestamp_ms":1,"message":{"role":"assistant","content":[{"type":"text","text":"hello "}]}}"#)
        .unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::Text { text, .. } if text == "hello "
    )));
    let second = parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","timestamp_ms":2,"message":{"role":"assistant","content":[{"type":"text","text":"world"}]}}"#)
        .unwrap();
    assert!(second.iter().any(|effect| matches!(
        effect,
        CursorEffect::Text { text, .. } if text == "world"
    )));
    // The cumulative result repeats the streamed fragments exactly: no extra
    // suffix, and completion carries the full cumulative reply with the
    // request-id transport turn id.
    let terminal = parser
        .parse_line(br#"{"type":"result","subtype":"success","is_error":false,"session_id":"synthetic-session","request_id":"req-1","result":"hello world"}"#)
        .unwrap();
    assert!(
        terminal
            .iter()
            .all(|effect| !matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(terminal.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome)
            if outcome.output == "hello world" && outcome.turn_id == "req-1"
    )));
}

#[test]
fn cursor_parser_terminal_result_appends_missing_suffix() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let first = parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#)
        .unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::Text { text, .. } if text == "hello"
    )));
    let terminal = parser
        .parse_line(br#"{"type":"result","subtype":"success","is_error":false,"session_id":"synthetic-session","result":"hello world"}"#)
        .unwrap();
    assert!(terminal.iter().any(|effect| matches!(
        effect,
        CursorEffect::Text { text, .. } if text == " world"
    )));
    assert!(terminal.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome) if outcome.output == "hello world"
    )));
}

#[test]
fn cursor_parser_terminal_result_keeps_streamed_text_when_result_diverges() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","message":{"role":"assistant","content":[{"type":"text","text":"hello"},{"type":"text","text":" world"}]}}"#)
        .unwrap();
    let terminal = parser
        .parse_line(
            br#"{"type":"result","subtype":"success","is_error":false,"session_id":"synthetic-session","result":"a different answer"}"#,
        )
        .unwrap();
    assert!(
        terminal
            .iter()
            .all(|effect| !matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(terminal.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome) if outcome.output == "hello world"
    )));
}

#[test]
fn cursor_parser_ignores_a_rewritten_snapshot_and_still_completes() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","timestamp_ms":1,"message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#)
        .unwrap();
    let rewritten = parser
        .parse_line(br#"{"type":"assistant","session_id":"synthetic-session","message":{"role":"assistant","content":[{"type":"text","text":"rewritten"}]}}"#)
        .unwrap();
    assert!(
        rewritten
            .iter()
            .all(|effect| !matches!(effect, CursorEffect::Text { .. }))
    );
    let terminal = parser
        .parse_line(
            br#"{"type":"result","subtype":"success","is_error":false,"session_id":"synthetic-session","result":"hello"}"#,
        )
        .unwrap();
    assert!(terminal.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome) if outcome.output == "hello"
    )));
}

#[test]
fn cursor_parser_accepts_an_explicit_empty_success_result() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let effects = parser
        .parse_line(br#"{"type":"result","subtype":"success","result":""}"#)
        .unwrap();
    assert!(effects.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome) if outcome.output.is_empty()
    )));
}

#[test]
fn cursor_parser_keeps_a_valid_reply_when_is_error_is_explicitly_false() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};

    let mut parser = CursorParser::new(
        "synthetic-session",
        "prompt",
        licoup_agent_cursor::model::EffectiveSettings::default(),
    );
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let effects = parser
        .parse_line(br#"{"type":"result","subtype":"error_during_execution","is_error":false,"session_id":"synthetic-session","result":"usable final reply"}"#)
        .unwrap();
    assert!(effects.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome) if outcome.output == "usable final reply"
    )));
}

#[test]
fn cursor_parser_classifies_an_explicit_terminal_execution_failure() {
    use crate::platform::native_agent_parser::adapters::cursor::{
        CursorParseFailure, CursorParser,
    };
    use licoup_agent_cursor::errors::CursorFailureKind;

    let mut parser = CursorParser::new(
        "synthetic-session",
        "prompt",
        licoup_agent_cursor::model::EffectiveSettings::default(),
    );
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    assert!(matches!(
        parser.parse_line(br#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"synthetic-session","result":"private vendor detail"}"#),
        Err(CursorParseFailure::TurnFailed(
            CursorFailureKind::ExecutionFailed
        ))
    ));
}

#[test]
fn cursor_parser_rejects_missing_or_different_prompt_acknowledgement() {
    use crate::platform::native_agent_parser::adapters::cursor::{
        CursorParseFailure, CursorParser,
    };
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut mismatched = CursorParser::new(
        "synthetic-session",
        "expected prompt",
        EffectiveSettings::default(),
    );
    assert!(matches!(
        mismatched.parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"different prompt"}]}}"#),
        Err(CursorParseFailure::PromptAcknowledgementMismatch)
    ));

    let mut missing = CursorParser::new(
        "synthetic-session",
        "expected prompt",
        EffectiveSettings::default(),
    );
    assert!(matches!(
        missing.parse_line(br#"{"type":"assistant","session_id":"synthetic-session","timestamp_ms":1,"message":{"role":"assistant","content":[{"type":"text","text":"reply"}]}}"#),
        Err(CursorParseFailure::PromptAcknowledgementMissing)
    ));
}

#[test]
fn cursor_parser_preserves_an_explicit_model_selector_over_the_init_label() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new(
        "synthetic-session",
        "prompt",
        EffectiveSettings {
            model: Some("composer-2.5".to_owned()),
            ..EffectiveSettings::default()
        },
    );
    parser
        .parse_line(br#"{"type":"system","subtype":"init","session_id":"synthetic-session","model":"Composer 2.5"}"#)
        .unwrap();
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let completed = parser
        .parse_line(br#"{"type":"result","subtype":"success","session_id":"synthetic-session","result":"ok"}"#)
        .unwrap();
    assert!(completed.iter().any(|effect| matches!(
        effect,
        CursorEffect::Complete(outcome)
            if outcome.effective.model.as_deref() == Some("composer-2.5")
    )));
}

#[test]
fn cursor_parser_projects_each_structured_tool_call_once() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let frame = br#"{"type":"assistant","session_id":"synthetic-session","message":{"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"lico_subagent_delegate","input":{"private":"not projected"}}]}}"#;
    let first = parser.parse_line(frame).unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::Tool { tool_name, .. } if tool_name == "lico_subagent_delegate"
    )));
    assert!(
        !first
            .iter()
            .any(|effect| matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(parser.parse_line(frame).unwrap().is_empty());
}

#[test]
fn cursor_parser_projects_current_mcp_tool_call_frames_once() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let started = br#"{"type":"tool_call","subtype":"started","session_id":"synthetic-session","tool_call":{"toolCallId":"call-1\nopaque","mcpToolCall":{"args":{"toolCallId":"call-1\nopaque","toolName":"lico_subagent_delegate","name":"land.lico.licoup.subagents-lico_subagent_delegate","args":{"private":"not projected"}}}}}"#;
    let completed = br#"{"type":"tool_call","subtype":"completed","session_id":"synthetic-session","tool_call":{"toolCallId":"call-1\nopaque","mcpToolCall":{"args":{"toolCallId":"call-1\nopaque","toolName":"lico_subagent_delegate","name":"land.lico.licoup.subagents-lico_subagent_delegate","args":{"private":"not projected"}}}}}"#;

    let first = parser.parse_line(started).unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::Tool { tool_name, .. } if tool_name == "lico_subagent_delegate"
    )));
    assert!(
        !first
            .iter()
            .any(|effect| matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(parser.parse_line(completed).unwrap().is_empty());
}

#[test]
fn cursor_parser_projects_only_allowlisted_mcp_application_error_codes() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let completed = br#"{"type":"tool_call","subtype":"completed","session_id":"synthetic-session","tool_call":{"toolCallId":"call-1","mcpToolCall":{"args":{"toolCallId":"call-1","toolName":"lico_subagent_delegate","args":{"private":"never projected"}},"result":{"success":{"isError":true,"content":[{"type":"text","text":"subagent_cross_conversation_rejected private provider result"}]}}}}}"#;

    let first = parser.parse_line(completed).unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::ToolError { tool_name, error_code, .. }
            if tool_name == "lico_subagent_delegate"
                && *error_code == "subagent_cross_conversation_rejected"
    )));
    assert!(
        !first
            .iter()
            .any(|effect| matches!(effect, CursorEffect::Text { .. }))
    );
    assert!(parser.parse_line(completed).unwrap().is_empty());
}

#[test]
fn cursor_parser_projects_membership_authorization_mcp_error_codes() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let completed = br#"{"type":"tool_call","subtype":"completed","session_id":"synthetic-session","tool_call":{"toolCallId":"call-1","mcpToolCall":{"args":{"toolCallId":"call-1","toolName":"lico_subagent_delegate","args":{"private":"never projected"}},"result":{"success":{"isError":true,"content":[{"type":"text","text":"caller_membership_not_authorized"}]}}}}}"#;

    let first = parser.parse_line(completed).unwrap();
    assert!(first.iter().any(|effect| matches!(
        effect,
        CursorEffect::ToolError { tool_name, error_code, .. }
            if tool_name == "lico_subagent_delegate"
                && *error_code == "caller_membership_not_authorized"
    )));
}

#[test]
fn cursor_parser_ignores_internal_mcp_catalog_calls() {
    use crate::platform::native_agent_parser::adapters::cursor::{CursorEffect, CursorParser};
    use licoup_agent_cursor::model::EffectiveSettings;

    let mut parser = CursorParser::new("synthetic-session", "prompt", EffectiveSettings::default());
    parser
        .parse_line(br#"{"type":"user","session_id":"synthetic-session","message":{"role":"user","content":[{"type":"text","text":"prompt"}]}}"#)
        .unwrap();
    let effects = parser
        .parse_line(br#"{"type":"tool_call","subtype":"started","session_id":"synthetic-session","tool_call":{"toolCallId":"catalog-1","getMcpToolsToolCall":{"args":{"server":"land.lico.licoup.subagents","toolName":"lico_subagent_delegate"}}}}"#)
        .unwrap();

    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, CursorEffect::Tool { .. }))
    );
}
