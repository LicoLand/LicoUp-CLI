//! The package's own claims, checked against its own code.
//!
//! Three things are proved here and nowhere else: the parser this package
//! publishes is the parser the SDK's registry finds by this Agent's dispatch id,
//! the replay arm refuses an adapter this package does not carry, and the fold
//! the session reader performs is the fold the client's usage meter depends on.
//!
//! The fixtures are synthetic. Nothing here reads a real Harness session, a real
//! client data root or the network.

use crate::parser::{FrameParser, TurnParser};
use crate::registration::{ADAPTER_ID, CONTRACT, FRAMING};
use crate::session_store::{
    COMPRESSED_SUFFIX, CURRENT_FORMAT_VERSION, SessionReadError, read_usage_samples,
};
use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One synthetic session artifact: a header and the rows a test folds.
fn write_session(directory: &Path, rows: &[Value], compressed: bool) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let name = format!("session.v{CURRENT_FORMAT_VERSION}.jsonl");
    let path = if compressed {
        directory.join(format!("{name}{COMPRESSED_SUFFIX}"))
    } else {
        directory.join(name)
    };
    let text: Vec<u8> = rows
        .iter()
        .flat_map(|row| {
            let mut line = serde_json::to_vec(row).unwrap();
            line.push(b'\n');
            line
        })
        .collect();
    if compressed {
        // One independently decodable, checksummed frame per durable batch,
        // which is the container the Harness writes and this reader declares.
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&text).unwrap();
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    } else {
        std::fs::write(&path, text).unwrap();
    }
    path
}

/// The synthetic header, with the caller's own fields merged over it.
fn header(extra: Value) -> Value {
    let mut value = json!({
        "type": "session",
        "version": CURRENT_FORMAT_VERSION,
        "id": "synthetic-session",
        "createdAt": 1784109600000_u64,
        "cwd": "/synthetic",
        "isSeeded": false,
        "delegationDepth": 0,
    });
    if let (Some(target), Some(source)) = (value.as_object_mut(), extra.as_object()) {
        for (key, field) in source {
            target.insert(key.clone(), field.clone());
        }
    }
    value
}

fn event(seq: u64, kind: &str, data: Value) -> Value {
    json!({
        "type": kind,
        "seq": seq,
        "time": 1784109600000_u64 + seq,
        "data": data,
    })
}

/// The route one synthetic request header declares.
fn header_row(seq: u64, reasoning_effort: Option<&str>, default_effort: Option<&str>) -> Value {
    let mut config = json!({"model": "synthetic-model", "provider": "synthetic-provider"});
    if let Some(effort) = reasoning_effort {
        config["reasoningEffort"] = json!(effort);
    }
    let mut header = json!({"config": config});
    if let Some(effort) = default_effort {
        header["adapterDefaults"] = json!({"reasoningEffort": effort});
    }
    event(seq, "request/header", json!({ "header": header }))
}

/// One assistant settlement row.
fn message_row(seq: u64, usage: Option<Value>, message: Value) -> Value {
    let mut data = json!({"turn": 1, "step": 0, "stream": []});
    data["message"] = message;
    if let Some(usage) = usage {
        data["usage"] = usage;
    }
    event(seq, "assistant/message", data)
}

/// One disposable root per test, so parallel tests never share an artifact.
fn synthetic_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "licoup-deepseek-package-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn the_published_registration_is_the_parser_the_registry_finds() {
    // The contract composition reads is the one the parser reports, reached
    // through the SDK's own lookup rather than from a second copy of the answer.
    assert_eq!(
        crate::registration::contract(),
        Some(CONTRACT),
        "the registry resolves this package's adapter id to this package's contract"
    );
    assert_eq!(CONTRACT.id, ADAPTER_ID);
    assert_eq!(CONTRACT.framing, FRAMING);
    // This Agent's turn stays the client's to settle: the parser reports the
    // protocol's own finish and imposes no timeout of its own.
    assert!(!CONTRACT.settles_turn);
    assert!(!CONTRACT.has_implicit_turn_timeout);
    assert!(CONTRACT.emits_all_content);
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let mut arm =
        crate::replay::replay_arm(ADAPTER_ID).expect("this package carries its own adapter");
    assert!(
        arm.feed(&licoup_agent_adapter_sdk::replay::RecordedFrame {
            index: 0,
            direction: "received".to_owned(),
            channel: "another-channel".to_owned(),
            payload: "{}".to_owned(),
        })
        .is_err()
    );
    assert!(crate::replay::replay_arm("codex").is_err());
}

#[test]
fn one_attempt_is_counted_once_and_a_retry_starts_a_second_attempt() {
    let root = synthetic_root("one-attempt");
    let path = write_session(
        &root,
        &[
            header(json!({})),
            header_row(0, Some("high"), None),
            message_row(
                1,
                Some(json!({"inputTokens": 900, "outputTokens": 1})),
                json!({"id": "message-1"}),
            ),
            // A later settlement of the same attempt replaces the earlier one.
            message_row(
                2,
                Some(json!({"inputTokens": 10, "outputTokens": 1})),
                json!({"id": "message-1"}),
            ),
            message_row(
                3,
                Some(json!({"inputTokens": 12, "outputTokens": 2})),
                json!({"id": "message-1"}),
            ),
            // The recorded retry supersedes the attempt it names, so the
            // attempt that follows is billed separately.
            event(4, "llm/retry-started", json!({"turn": 1, "step": 0})),
            event(
                5,
                "assistant/attempt",
                json!({"turn": 1, "step": 0, "stream": [
                    {"type": "chunk", "chunk": {"type": "usage", "usage": {"inputTokens": 3, "outputTokens": 1}}},
                    {"type": "chunk", "chunk": {"type": "usage", "usage": {"inputTokens": 4, "outputTokens": 2}}},
                ]}),
            ),
        ],
        false,
    );
    let samples = read_usage_samples(&path).unwrap();
    assert_eq!(samples.len(), 2, "two attempts, not four settlements");
    assert_eq!(samples[0].seq, 3);
    assert_eq!(
        samples[0].usage,
        Some(json!({"inputTokens": 12, "outputTokens": 2}))
    );
    assert_eq!(samples[0].effort.as_deref(), Some("high"));
    assert_eq!(samples[1].seq, 5);
    assert_eq!(
        samples[1].usage,
        Some(json!({"inputTokens": 4, "outputTokens": 2})),
        "the last recorded usage chunk is the attempt's own settlement"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_tokenless_settlement_never_erases_a_reported_one() {
    let root = synthetic_root("tokenless");
    let path = write_session(
        &root,
        &[
            header(json!({})),
            header_row(0, None, None),
            message_row(
                1,
                Some(json!({"inputTokens": 4, "outputTokens": 2})),
                json!({"id": "message-1"}),
            ),
            message_row(2, None, json!({"id": "message-1"})),
        ],
        false,
    );
    let samples = read_usage_samples(&path).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(
        samples[0].usage,
        Some(json!({"inputTokens": 4, "outputTokens": 2})),
        "a later tokenless settlement does not erase the reported sample"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_forked_prefix_is_not_billed_again_and_a_later_header_replaces_the_route() {
    let root = synthetic_root("forked-prefix");
    let mut session_header = header(json!({}));
    session_header["inheritedEventCount"] = json!(3);
    let path = write_session(
        &root,
        &[
            session_header,
            header_row(0, Some("high"), None),
            message_row(
                1,
                Some(json!({"inputTokens": 900, "outputTokens": 1})),
                json!({"id": "inherited"}),
            ),
            // A later header does not inherit the route or the effort that came
            // before it.
            header_row(3, None, Some("low")),
            message_row(
                4,
                Some(json!({"inputTokens": 5, "outputTokens": 1})),
                json!({"id": "own"}),
            ),
        ],
        false,
    );
    let samples = read_usage_samples(&path).unwrap();
    assert_eq!(samples.len(), 1, "the inherited prefix is not consumption");
    assert_eq!(samples[0].seq, 4);
    assert_eq!(
        samples[0].effort.as_deref(),
        Some("low"),
        "the replacement header's own default applies"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_real_response_route_wins_and_missing_options_do_not_inherit() {
    let root = synthetic_root("actual-route");
    let path = write_session(
        &root,
        &[
            header(json!({})),
            header_row(0, Some("high"), Some("low")),
            message_row(
                1,
                Some(json!({"inputTokens": 3, "outputTokens": 1})),
                json!({"id": "m", "source": {"model": "actual-other", "provider": "other"}}),
            ),
        ],
        false,
    );
    let samples = read_usage_samples(&path).unwrap();
    assert_eq!(samples[0].model.as_deref(), Some("actual-other"));
    assert_eq!(samples[0].provider.as_deref(), Some("other"));
    assert_eq!(
        samples[0].effort, None,
        "the header's effort belongs to the header's own route"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_zstd_container_and_a_plain_artifact_fold_identically() {
    let root = synthetic_root("container");
    let rows = [
        header(json!({})),
        header_row(0, Some("max"), None),
        message_row(
            1,
            Some(json!({"inputTokens": 70, "cacheReadTokens": 30, "outputTokens": 20})),
            json!({"id": "message-1"}),
        ),
    ];
    let plain = write_session(&root.join("plain"), &rows, false);
    std::fs::create_dir_all(root.join("compressed")).unwrap();
    let compressed = write_session(&root.join("compressed"), &rows, true);
    assert_eq!(
        read_usage_samples(&plain).unwrap(),
        read_usage_samples(&compressed).unwrap(),
        "compression is physical and changes no counted fact"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_generation_this_reader_does_not_read_is_refused_by_name() {
    let root = synthetic_root("generation");
    let mut older = header(json!({}));
    older["version"] = json!(3);
    let path = write_session(&root, &[older], false);
    match read_usage_samples(&path) {
        Err(SessionReadError::UnsupportedFormatVersion(version)) => assert_eq!(version, 3),
        other => panic!("an unread generation must be refused by name, got {other:?}"),
    }
    let bare = root.join("session.v4.jsonl");
    std::fs::write(&bare, b"{\"not\":\"a session\"}\n").unwrap();
    assert!(matches!(
        read_usage_samples(&bare),
        Err(SessionReadError::Malformed(_))
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_parser_attributes_only_the_receipted_turn_until_idle() {
    // The reader and the turn parser are one package, so the two halves are
    // exercised together: the same synthetic frames a live transport emits.
    fn frame(value: Value) -> crate::parser::ProtocolFrame {
        FrameParser
            .parse_line(value.to_string().as_bytes())
            .unwrap()
    }
    let mut parser = TurnParser::new("prompt-1", "session-1");
    for value in [
        json!({"id":"prompt-1","result":{"messageId":"message-1"}}),
        json!({"method":"session.event","params":{"sessionId":"session-1","event":{"type":"agent/inbox/spliced","data":{"inserted":[{"id":"message-1"}]}}}}),
        json!({"method":"session.event","params":{"sessionId":"session-1","event":{"type":"assistant/message","data":{"message":{"content":[{"type":"text","text":"reply"}]}}}}}),
    ] {
        assert!(parser.ingest(frame(value)).unwrap().is_none());
    }
    let result = parser
        .ingest(frame(
            json!({"method":"session.status","params":{"sessionId":"session-1","status":"idle"}}),
        ))
        .unwrap()
        .unwrap();
    assert_eq!(result.turn_id, "message-1");
    assert_eq!(result.output, "reply");
}
