//! The Antigravity package's own claims.
//!
//! A claim that names Antigravity belongs here, where the parser, the ports and
//! the registration behind them are all in view: what one Stop-hook payload
//! resolves to, what one terminal outcome classifies as, what the native receipt
//! writer does with the payload it is handed, and what a port answers before a
//! host installs it. The execution port's own lifecycle claim is about the
//! *uninstalled* process state, so it is asserted in
//! `tests/execution_port_lifecycle.rs`, whose process installs nothing.

use std::path::PathBuf;

use serde_json::json;

use crate::hook::{self, HookOutcome};
use crate::parser::{
    PtyOutputParser, TerminalFacts, classify_terminal, completed_transitions, parse_hook_receipt,
    valid_session_id,
};
use crate::port::turn_event;
use crate::registration::{ADAPTER_ID, FRAMING, REGISTRATION, parser_set};

const RECEIPT_ID: &str = "11111111-2222-3333-4444-555555555555";
const OTHER_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// One throwaway directory per test, removed when the guard drops.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lico-agent-antigravity-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temporary root");
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_package_declares_the_one_adapter_it_carries() {
    assert_eq!(ADAPTER_ID, "antigravity");
    assert_eq!(FRAMING, "pty-hook-json");
    assert_eq!(REGISTRATION.contract.id, ADAPTER_ID);
    assert_eq!(REGISTRATION.contract.framing, FRAMING);
    let set = parser_set();
    assert_eq!(set.registered_ids(), [ADAPTER_ID]);
    assert_eq!(crate::registration::contract(), Some(REGISTRATION.contract));
    // The contract is the parser's own declaration, so a reader that reaches it
    // through the SDK's lookup and a reader that reads the constant cannot
    // disagree.
    assert_eq!(REGISTRATION.contract, crate::parser::CONTRACT);
}

#[test]
fn a_native_conversation_identity_is_the_protocols_own_shape() {
    assert!(valid_session_id(RECEIPT_ID));
    assert!(valid_session_id("abcdefgh"));
    for refused in ["", "short", &"a".repeat(129), "has space", "has/slash"] {
        assert!(!valid_session_id(refused), "accepted {refused:?}");
    }
}

#[test]
fn a_receipt_resolves_a_direct_object_or_an_accepted_alias() {
    for key in [
        "conversationId",
        "conversation_id",
        "sessionId",
        "session_id",
    ] {
        let receipt = format!(r#"{{"{key}":"{RECEIPT_ID}"}}"#);
        assert_eq!(
            parse_hook_receipt(&receipt).as_deref(),
            Some(RECEIPT_ID),
            "alias {key}"
        );
    }
    // The wrapped vendor payload and the vendor environment identifier are
    // compatible inputs, and a direct receipt always wins over the fallback.
    assert_eq!(
        parse_hook_receipt(&format!(
            r#"{{"hookPayload":"{{\"conversationId\":\"{RECEIPT_ID}\"}}","environmentConversationId":""}}"#
        ))
        .as_deref(),
        Some(RECEIPT_ID)
    );
    assert_eq!(
        parse_hook_receipt(&format!(
            r#"{{"conversationId":"{RECEIPT_ID}","environmentConversationId":"{OTHER_ID}"}}"#
        ))
        .as_deref(),
        Some(RECEIPT_ID)
    );
    for malformed in [
        r#"{"conversationId":""}"#,
        r#"{"conversationId":"short"}"#,
        r#"{"conversationId":42}"#,
        r#"{"hookPayload":"not-json"}"#,
        "not json at all",
    ] {
        assert_eq!(
            parse_hook_receipt(malformed),
            None,
            "accepted {malformed:?}"
        );
    }
}

#[test]
fn terminal_classification_reports_timeout_receipt_drift_exit_and_empty_output() {
    let timed_out = classify_terminal(TerminalFacts {
        requested_session: RECEIPT_ID,
        receipt_session: Some(RECEIPT_ID),
        output: "text",
        timed_out: true,
        exit_success: true,
    })
    .unwrap_err();
    assert_eq!(timed_out.code, "antigravity_cli_timeout");

    let missing = classify_terminal(TerminalFacts {
        requested_session: "",
        receipt_session: None,
        output: "text",
        timed_out: false,
        exit_success: true,
    })
    .unwrap_err();
    assert_eq!(missing.code, "antigravity_hook_receipt_missing");

    let drift = classify_terminal(TerminalFacts {
        requested_session: RECEIPT_ID,
        receipt_session: Some(OTHER_ID),
        output: "text",
        timed_out: false,
        exit_success: true,
    })
    .unwrap_err();
    assert_eq!(drift.code, "antigravity_cli_session_drift");

    let failed = classify_terminal(TerminalFacts {
        requested_session: RECEIPT_ID,
        receipt_session: Some(RECEIPT_ID),
        output: "text",
        timed_out: false,
        exit_success: false,
    })
    .unwrap_err();
    assert_eq!(failed.code, "antigravity_cli_turn_failed");

    let empty = classify_terminal(TerminalFacts {
        requested_session: RECEIPT_ID,
        receipt_session: Some(RECEIPT_ID),
        output: "",
        timed_out: false,
        exit_success: true,
    })
    .unwrap_err();
    assert_eq!(empty.code, "antigravity_cli_empty_output");

    let success = classify_terminal(TerminalFacts {
        requested_session: "",
        receipt_session: Some(RECEIPT_ID),
        output: "reply",
        timed_out: false,
        exit_success: true,
    })
    .expect("a bound receipt and a successful process is a completed turn");
    assert_eq!(success.session_id, RECEIPT_ID);
    assert_eq!(success.output, "reply");
}

#[test]
fn the_pty_lane_strips_terminal_control_once() {
    let mut parser = PtyOutputParser::new();
    assert_eq!(parser.push(b"\x1b[31mhello"), Some("hello".to_owned()));
    assert_eq!(parser.push(b"\x1b[0m\n"), Some("\n".to_owned()));
    let (output, tail) = parser.finish();
    assert_eq!(output, "hello");
    assert_eq!(tail, None);
}

#[test]
fn execution_transitions_report_a_reply_or_the_protocols_own_failure() {
    use licoup_agent_adapter_sdk::Transition;
    use licoup_agent_adapter_sdk::port::{ExecutionFailure, ExecutionOutcome};

    let completed = crate::registration::execution_transitions(&ExecutionOutcome {
        output: "reply",
        failure: None,
    });
    assert_eq!(completed, completed_transitions("reply"));
    assert!(matches!(completed.last(), Some(Transition::Lifecycle(_))));

    let failed = crate::registration::execution_transitions(&ExecutionOutcome {
        output: "",
        failure: Some(ExecutionFailure {
            code: "antigravity_cli_timeout",
            stage: "turn/execute",
            message: "timed out",
        }),
    });
    assert!(failed.iter().any(|transition| matches!(
        transition,
        Transition::Failed { code, stage, .. }
            if *code == "antigravity_cli_timeout" && *stage == "turn/execute"
    )));
}

#[test]
fn a_durable_identity_is_accepted_only_by_the_protocols_own_rule() {
    use licoup_agent_adapter_sdk::port::DurableIdentityRequest;

    assert!(crate::registration::valid_identity(
        &DurableIdentityRequest {
            session_id: RECEIPT_ID,
            location: None,
        }
    ));
    assert!(!crate::registration::valid_identity(
        &DurableIdentityRequest {
            session_id: "short",
            location: None,
        }
    ));
    // A recorded location must name the identity itself: a locator never
    // authorizes a resume on its own.
    let named = PathBuf::from(format!("/fixture/conversations/{RECEIPT_ID}.json"));
    assert!(crate::registration::valid_identity(
        &DurableIdentityRequest {
            session_id: RECEIPT_ID,
            location: Some(&named),
        }
    ));
    let other = PathBuf::from(format!("/fixture/conversations/{OTHER_ID}.json"));
    assert!(!crate::registration::valid_identity(
        &DurableIdentityRequest {
            session_id: RECEIPT_ID,
            location: Some(&other),
        }
    ));
}

#[test]
fn the_receipt_hook_writes_one_direct_object_owner_only() {
    let root = TempRoot::new("receipt-direct");
    let receipt = root.join("receipt.json");
    let payload = format!(r#"{{"conversationId":"{RECEIPT_ID}","cwd":"/workspace"}}"#);
    assert_eq!(
        hook::record_with_environment(&receipt, &payload, None).unwrap(),
        HookOutcome::Recorded
    );

    let text = std::fs::read_to_string(&receipt).expect("the receipt is readable");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        json!({ "conversationId": RECEIPT_ID }),
        "the hook writes one direct JSON object, not a wrapped payload"
    );
    assert_eq!(parse_hook_receipt(&text).as_deref(), Some(RECEIPT_ID));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&receipt).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a conversation identity is owner-only, never group or world readable"
        );
    }
}

#[test]
fn the_receipt_hook_reads_the_vendor_environment_as_a_fallback() {
    let root = TempRoot::new("receipt-env");
    let receipt = root.join("receipt.json");
    // The environment is the vendor's own compatibility input. It is read only
    // when the payload carries no identity, which is the order the driver has
    // always applied.
    let payload = r#"{"transcriptPath":"/workspace/transcript"}"#;
    assert_eq!(
        hook::record_with_environment(&receipt, payload, Some(RECEIPT_ID)).unwrap(),
        HookOutcome::Recorded
    );
    assert_eq!(
        parse_hook_receipt(&std::fs::read_to_string(&receipt).unwrap()).as_deref(),
        Some(RECEIPT_ID)
    );
}

#[test]
fn the_receipt_hook_never_erases_a_receipt_a_previous_writer_bound() {
    let root = TempRoot::new("receipt-order");
    let receipt = root.join("receipt.json");
    // A vendor-direct receipt can already exist on this path when the hook runs.
    std::fs::write(&receipt, format!(r#"{{"sessionId":"{OTHER_ID}"}}"#)).unwrap();
    let payload = r#"{"transcriptPath":"/workspace/transcript"}"#;
    assert_eq!(
        hook::record_with_environment(&receipt, payload, None).unwrap(),
        HookOutcome::KeptExisting
    );
    assert_eq!(
        parse_hook_receipt(&std::fs::read_to_string(&receipt).unwrap()).as_deref(),
        Some(OTHER_ID),
        "an identity this run did not resolve must not overwrite one already bound"
    );

    // With no existing receipt there is nothing to keep, and nothing is written.
    let absent = root.join("absent.json");
    assert_eq!(
        hook::record_with_environment(&absent, payload, None).unwrap(),
        HookOutcome::NoIdentity
    );
    assert!(!absent.exists(), "no identity means no receipt file");
}

#[test]
fn the_turn_event_port_emits_nothing_until_the_host_installs_it() {
    // Reached through the installed port rather than a second sink: before an
    // installation every emitter is silent, and the installed answer is the one
    // a host supplied.
    let installed = turn_event::installed();
    turn_event::emit_agent_message_chunk("session", "turn", "text");
    turn_event::emit_agent_message_completed("session", "turn", "text");
    turn_event::emit_agent_processing("session", "turn", "activity", None);
    turn_event::emit_turn_event("turn/started", "session", "turn", json!({}));
    assert_eq!(
        turn_event::installed(),
        installed,
        "emitting never installs a sink of its own"
    );
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    assert!(crate::replay::replay_arm("antigravity").is_ok());
    // An adapter this package does not carry is refused rather than defaulted,
    // so a fixture can never pass against a parser that was never constructed.
    let refused = match crate::replay::replay_arm("cursor") {
        Ok(_) => panic!("a foreign adapter must not get an arm"),
        Err(error) => error,
    };
    assert!(refused.contains("cursor"), "{refused}");
}
