//! The replay arms this host composes, and the corpus checks that drive them.
//!
//! `licoup-agent-adapter-sdk` owns the shared replay harness — corpus
//! resolution, the recorded-frame model, the fail-closed properties and the
//! projection comparison. This tree owns the arms: one per Agent, each
//! constructing the same parser that Agent's production driver constructs.
//!
//! The arms are compiled for the test build only, exactly as they were before
//! the SDK split, because the harness is a test surface and a production build
//! calls no arm. The checks below are the family's own: they are a claim about
//! the composition of the thirteen parsers, so they live where both halves —
//! the parsers and the corpus — are in view, and they keep the crate-test paths
//! the fixture tools filter on.

pub(in crate::platform) mod adapters;

pub(in crate::platform) use licoup_agent_adapter_sdk::replay::FrameReplay;

use licoup_agent_adapter_sdk::replay::{
    fixture_document, fixture_root, record_projections, recording_root, replay, replay_corpus,
};

/// The parser set the family checks drive: the host's registrations and the
/// replay arms that belong to them.
pub(in crate::platform) fn replay_parser_set() -> licoup_agent_adapter_sdk::port::AdapterParserSet {
    licoup_agent_adapter_sdk::port::AdapterParserSet {
        registrations: super::adapters::registrations,
        replay: adapters::replay_arm,
    }
}

/// Projection recorder. Not a check: it rewrites the projections of a corpus
/// copy from the real parsers, and the tool that owns fixture generation runs
/// it before a fixture can be reviewed. It never runs in the normal test set.
#[test]
#[ignore = "recorder: rewrites fixture projections from the real parsers"]
fn adapter_replay_record_projections() {
    let root = recording_root();
    let recorded = record_projections(&replay_parser_set(), &root);
    assert!(
        recorded > 0,
        "no replay fixtures found under {root:?}: nothing was recorded"
    );
    println!("recorded projections for {recorded} fixtures under {root:?}");
}

#[test]
fn adapter_replay_corpus_is_complete_and_frame_deterministic() {
    let _ = replay_corpus(&replay_parser_set(), &fixture_root());
}

#[test]
fn adapter_replay_rejects_a_projection_the_parser_did_not_produce() {
    let root = fixture_root();
    let mut document = fixture_document(&root, "codex", "streaming-interruption");
    document["frames"][1]["projection"] = serde_json::json!([{"effect": "never-recorded-fact"}]);
    let error = replay(
        &replay_parser_set(),
        &document,
        "codex",
        "streaming-interruption",
    )
    .expect_err("a projection the parser does not produce must fail replay");
    assert!(
        error.contains("adapter=codex scenario=streaming-interruption frame=1"),
        "unexpected projection attribution: {error}"
    );
    assert!(
        error.contains("projection mismatch"),
        "unexpected projection failure: {error}"
    );
}

#[test]
fn adapter_replay_attributes_a_drifted_frame_to_its_exact_index() {
    let root = fixture_root();
    let mut document = fixture_document(&root, "cursor", "normal-turn");
    // The terminal frame is the one whose drift can only show on itself: an
    // earlier frame carries the identity and prompt acknowledgement the later
    // frames are checked against, so a parser that rejects it reports the
    // first frame that no longer matches rather than the frame that was edited.
    let drifted = document["frames"]
        .as_array()
        .expect("fixture frames")
        .len()
        .checked_sub(1)
        .expect("fixture has a frame");
    document["frames"][drifted]["payload"] =
        serde_json::json!(r#"{"type":"unknown-vendor-frame"}"#);
    let error = replay(&replay_parser_set(), &document, "cursor", "normal-turn")
        .expect_err("a drifted frame must fail replay");
    assert!(
        error.contains(&format!(
            "adapter=cursor scenario=normal-turn frame={drifted}"
        )),
        "unexpected drift attribution: {error}"
    );
}

#[test]
#[should_panic(expected = "projection missing")]
fn adapter_replay_fails_closed_on_an_unrecorded_frame() {
    let document = serde_json::json!({
        "frames": [{
            "index": 0,
            "direction": "agent-to-client",
            "channel": "stdio-jsonrpc",
            "payload": "{}",
        }],
    });
    let _ = replay(&replay_parser_set(), &document, "codex", "normal-turn");
}
