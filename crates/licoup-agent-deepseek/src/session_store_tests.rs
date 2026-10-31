//! The moved DeepSeek paths, exercised where the client still composes them.
//!
//! Two halves are proved against the package rather than against a stub: the
//! session artifact the client used to read through a Node worker is now folded
//! by `licoup_agent_deepseek`, and the sample fold still produces the same
//! request records, variants and days this pipeline published before — including
//! the replacement, retry and inherited-prefix rules that decide how many
//! requests a session contains.
//!
//! The artifacts are synthetic and live in a disposable directory. Nothing here
//! reads a real Harness home, a client data root or the network.

use super::super::contract::UsageVariant;
use super::super::window::UsageWindow;
use super::deepseek;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

/// One synthetic session artifact under a disposable root.
fn artifact(name: &str, rows: &[Value], compressed: bool) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "licoup-native-deepseek-{}-{name}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join(if compressed {
        "session.v4.jsonl.zstd"
    } else {
        "session.v4.jsonl"
    });
    let text: Vec<u8> = rows
        .iter()
        .flat_map(|row| {
            let mut line = serde_json::to_vec(row).unwrap();
            line.push(b'\n');
            line
        })
        .collect();
    if compressed {
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&text).unwrap();
        std::fs::write(&file, encoder.finish().unwrap()).unwrap();
    } else {
        std::fs::write(&file, text).unwrap();
    }
    (root, file)
}

fn session_header(inherited: Option<u64>) -> Value {
    let mut header = json!({
        "type": "session",
        "version": 4,
        "id": "synthetic-session",
        "createdAt": 1784109600000_u64,
        "cwd": "/synthetic",
        "isSeeded": false,
        "delegationDepth": 0,
    });
    if let Some(count) = inherited {
        header["inheritedEventCount"] = json!(count);
        header["isSeeded"] = json!(true);
    }
    header
}

fn request_header(seq: u64) -> Value {
    json!({
        "type": "request/header",
        "seq": seq,
        "time": 1784109600000_u64 + seq,
        "data": {"header": {"config": {
            "model": "deepseek-native",
            "provider": "deepseek-official",
            "reasoningEffort": "high",
        }}},
    })
}

/// One assistant settlement, optionally without a usage object.
fn settlement(seq: u64, usage: Option<Value>) -> Value {
    let mut data = json!({
        "turn": 1,
        "step": 0,
        "stream": [],
        "message": {"id": "message-1", "role": "assistant", "content": []},
    });
    if let Some(usage) = usage {
        data["usage"] = usage;
    }
    json!({
        "type": "assistant/message",
        "seq": seq,
        "time": 1784109600000_u64 + seq,
        "data": data,
    })
}

fn calendar() -> UsageWindow {
    UsageWindow::from_params(&json!({
        "now": "2026-07-15T12:00:00Z",
        "historyDays": 1,
        "timezoneOffsetMinutes": 480,
    }))
}

#[test]
fn the_native_reader_folds_replacement_retry_and_inheritance_exactly_once() {
    // Two settlements of one attempt replace rather than add; the recorded retry
    // starts a separately billed attempt; the fork's inherited prefix is not
    // consumption. Only the third settlement and the attempt after the retry
    // are the session's own.
    let (root, file) = artifact(
        "fold",
        &[
            session_header(None),
            request_header(0),
            settlement(1, Some(json!({"inputTokens": 900, "outputTokens": 1}))),
            settlement(2, Some(json!({"inputTokens": 12, "outputTokens": 2}))),
            json!({"type": "llm/retry-started", "seq": 3, "time": 1784109600003_u64,
                   "data": {"turn": 1, "step": 0, "retryId": "retry-1", "retry": 1}}),
            json!({"type": "assistant/attempt", "seq": 4, "time": 1784109600004_u64,
            "data": {"turn": 1, "step": 0, "stream": [
                {"type": "chunk", "chunk": {"type": "usage", "usage": {"inputTokens": 3, "outputTokens": 1}}},
                {"type": "chunk", "chunk": {"type": "usage", "usage": {"inputTokens": 4, "outputTokens": 2}}},
            ]}}),
        ],
        false,
    );
    let parsed = deepseek::Reader::default()
        .parse(&file, 100, &calendar())
        .unwrap();
    assert_eq!(parsed.summary.request_count, 2, "two admitted attempts");
    assert_eq!(parsed.summary.total_tokens, 14 + 6);
    assert_eq!(parsed.session_increment, 1);

    // The fork's inherited prefix is a copied ancestry, and a later header
    // replaces the route it declares rather than inheriting it.
    let (fork_root, fork) = artifact(
        "fork",
        &[
            session_header(Some(3)),
            request_header(0),
            settlement(1, Some(json!({"inputTokens": 900, "outputTokens": 1}))),
            request_header(3),
            settlement(4, Some(json!({"inputTokens": 5, "outputTokens": 1}))),
        ],
        true,
    );
    let parsed = deepseek::Reader::default()
        .parse(&fork, 200, &calendar())
        .unwrap();
    assert_eq!(parsed.summary.request_count, 1);
    assert_eq!(parsed.summary.total_tokens, 6);
    assert_eq!(
        parsed.summary.daily_usage["2026-07-15"].model_variants[&(
            "deepseek-native".to_owned(),
            UsageVariant {
                effort: Some("high".to_owned()),
                fast: None,
            }
        )]
            .total_tokens,
        6,
        "the attempt keeps the route and effort it was actually made with"
    );
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(fork_root).unwrap();
}

#[test]
fn a_generation_the_package_does_not_read_fails_the_session_read() {
    // A generation LicoUp has not been written against is a named read failure
    // rather than a count: the pipeline reports the stage and never a guess.
    let (root, file) = artifact(
        "older",
        &[
            json!({"type": "session", "version": 3, "id": "older", "createdAt": 0,
                   "cwd": "/synthetic", "isSeeded": false, "delegationDepth": 0}),
        ],
        false,
    );
    let failure = deepseek::Reader::default()
        .parse(&file, 10, &calendar())
        .expect_err("an unread generation is refused");
    let named = failure
        .downcast_ref::<deepseek::ReadFailure>()
        .expect("the failure names the read stage");
    assert_eq!(named.0, "session-read");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_newest_generation_is_the_one_session_source() {
    // Several generations of one session are representations of it, so only the
    // newest is read — the same rule the removed Node reader resolved.
    let sources = [
        "session.jsonl.zstd",
        "session.v1.jsonl.zstd",
        "session.v3.jsonl.zstd",
    ]
    .into_iter()
    .map(|name| {
        (
            PathBuf::from("root/project/id").join(name),
            "test".to_owned(),
        )
    })
    .collect();
    let selected = deepseek::canonical_sources(sources);
    assert_eq!(selected.len(), 1);
    assert_eq!(
        selected
            .first_key_value()
            .unwrap()
            .0
            .file_name()
            .and_then(|name| name.to_str()),
        Some("session.v3.jsonl.zstd")
    );
}
