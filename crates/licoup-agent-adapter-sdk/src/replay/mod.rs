//! Replay of recorded adapter transcripts through the real parser boundary.
//!
//! Every frame of every fixture is fed to the same adapter parser the
//! production driver uses, and the recorded projection must equal what that
//! parser actually produced. Nothing here re-implements vendor framing: a
//! fixture can only pass if the real parser still reports the recorded facts,
//! so a protocol regression in an adapter fails its own corpus.
//!
//! Fail-closed properties enforced below: an absent corpus fails instead of
//! passing; a frame without a recorded projection fails instead of skipping;
//! and projections are recorded by the real parsers, never authored by hand.
//!
//! The parsers and their arms are composed above this crate and arrive through
//! [`AdapterParserSet`]; this module is the harness, the corpus resolution and
//! the properties, and it names no Agent. A composing program runs the corpus
//! checks it owns against its own set — the checks this extraction left in
//! `licoup-native`'s `native_agent_parser::replay`.

pub mod adapters;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::port::AdapterParserSet;

/// One recorded vendor frame, as it crossed the adapter boundary.
pub struct RecordedFrame {
    /// Zero-based position in the recorded transcript.
    pub index: usize,
    /// Wire direction. The corpus records what the agent sent us.
    pub direction: String,
    /// The adapter's own framing, which must equal `AdapterContract::framing`.
    pub channel: String,
    /// The raw vendor frame. PTY-borne frames carry their bytes here verbatim.
    pub payload: String,
}

/// A parser that can be replayed from a recorded transcript.
///
/// An arm constructs the same parser the production driver constructs, feeds
/// it recorded frames exactly as the driver feeds live bytes, and reports the
/// parser's real output. The projection is a direct view of the parser's own
/// report: one entry per real output item, each carrying the parser's variant
/// name under `effect`, or a single `error` entry when the parser rejected the
/// frame. `Err` is reserved for frames the boundary cannot consume at all.
pub trait FrameReplay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String>;
}

/// The scenario classes every registered adapter's corpus must carry.
pub const SCENARIOS: [&str; 5] = [
    "normal-turn",
    "user-cancel",
    "agent-error",
    "streaming-interruption",
    "native-resume",
];

/// Set only by the projection recorder, which rewrites an out-of-tree corpus
/// copy. The replay check itself always reads the repository corpus.
pub const RECORD_ROOT_ENV: &str = "LICO_ADAPTER_REPLAY_RECORD_ROOT";

/// The adapter ids a corpus must cover: every Agent parser the composing
/// program carries, in packaged inventory order.
///
/// This read the packaged driver declaration document before the parsers became
/// an injected set. It reads the composed set now, because a corpus covers
/// parsers: the declaration document and the parser set are held one-to-one by
/// the registry check the composition asserts, and a coverage check that read a
/// document instead would pass while a composed parser had no fixture at all.
pub fn registered_adapters(set: &AdapterParserSet) -> Vec<String> {
    let mut ids: Vec<String> = set
        .registered_ids()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        count,
        "composed Agent parser set contains duplicate adapter id"
    );
    ids
}

pub fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replay-corpus")
}

pub fn fixture_root() -> PathBuf {
    let corpus = corpus_root();
    if corpus.is_dir() {
        corpus
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/test/fixtures/adapter-replay")
    }
}

/// Corpus root used by the recorder only. Absent means the recorder has not
/// been pointed at a corpus copy, which is a usage error rather than a pass.
pub fn recording_root() -> PathBuf {
    match std::env::var(RECORD_ROOT_ENV) {
        Ok(root) if !root.is_empty() => PathBuf::from(root),
        _ => panic!("{RECORD_ROOT_ENV} must name the corpus copy to record projections into"),
    }
}

pub fn fixture_document(root: &Path, adapter: &str, scenario: &str) -> Value {
    let path = root.join(adapter).join(format!("{scenario}.json"));
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "replay fixture missing or unreadable for adapter={adapter} scenario={scenario}: {error}"
        )
    });
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!("replay fixture invalid for adapter={adapter} scenario={scenario}: {error}")
    })
}

pub fn recorded_frames(document: &Value, adapter: &str, scenario: &str) -> Vec<RecordedFrame> {
    let frames = document
        .get("frames")
        .and_then(Value::as_array)
        .filter(|frames| !frames.is_empty())
        .unwrap_or_else(|| {
            panic!("adapter={adapter} scenario={scenario}: frames missing or empty")
        });
    frames
        .iter()
        .enumerate()
        .map(|(position, frame)| {
            let index = frame.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX);
            assert_eq!(
                index, position as u64,
                "adapter={adapter} scenario={scenario} frame={position}: non-contiguous recorded index {index}"
            );
            RecordedFrame {
                index: position,
                direction: frame
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "adapter={adapter} scenario={scenario} frame={position}: direction missing"
                        )
                    })
                    .to_owned(),
                channel: frame
                    .get("channel")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "adapter={adapter} scenario={scenario} frame={position}: channel missing"
                        )
                    })
                    .to_owned(),
                payload: frame
                    .get("payload")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "adapter={adapter} scenario={scenario} frame={position}: payload missing"
                        )
                    })
                    .to_owned(),
            }
        })
        .collect()
}

pub fn recorded_projection(
    frame: &Value,
    adapter: &str,
    scenario: &str,
    position: usize,
) -> Vec<Value> {
    frame
        .get("projection")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "adapter={adapter} scenario={scenario} frame={position}: projection missing; \
                 run the adapter_replay_record_projections recorder against the real parsers"
            )
        })
}

/// Replay one transcript and return the projection drift, if any.
///
/// Every frame is checked before it is fed, so an unrecorded corpus fails
/// before any parser runs rather than after a partial replay.
pub fn replay(
    set: &AdapterParserSet,
    document: &Value,
    adapter: &str,
    scenario: &str,
) -> Result<(), String> {
    if !document
        .get("frames")
        .and_then(Value::as_array)
        .is_some_and(|frames| !frames.is_empty())
    {
        return Err(format!(
            "adapter={adapter} scenario={scenario}: frames missing or empty"
        ));
    }
    let frames = document["frames"].as_array().expect("checked above");
    let framing = adapters::contract_framing(set, adapter)
        .unwrap_or_else(|error| panic!("adapter={adapter} scenario={scenario}: {error}"));
    let mut parser = adapters::replay_for(set, adapter)
        .unwrap_or_else(|error| panic!("adapter={adapter} scenario={scenario}: {error}"));
    for (position, (frame, recorded)) in frames
        .iter()
        .zip(recorded_frames(document, adapter, scenario))
        .enumerate()
    {
        if recorded.channel != framing {
            return Err(format!(
                "adapter={adapter} scenario={scenario} frame={position}: recorded channel {:?} is \
                 not the adapter's real framing {framing:?}",
                recorded.channel
            ));
        }
        let expected = recorded_projection(frame, adapter, scenario, position);
        let actual = parser.feed(&recorded).map_err(|error| {
            format!("adapter={adapter} scenario={scenario} frame={position}: {error}")
        })?;
        if actual != expected {
            return Err(format!(
                "adapter={adapter} scenario={scenario} frame={position}: projection mismatch: \
                 parser produced {actual:?}, corpus recorded {expected:?}"
            ));
        }
    }
    Ok(())
}

/// Replay a whole corpus copy against the composing program's own parsers, and
/// report the coverage it established.
///
/// The composing program owns the call because it owns the arms; the harness
/// owns what the check means, so the claim cannot drift between programs.
pub fn replay_corpus(set: &AdapterParserSet, root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    assert!(
        root.is_dir(),
        "replay corpus absent at {root:?}: a missing transcript corpus is never a pass"
    );
    let adapters = registered_adapters(set);
    assert!(
        !adapters.is_empty(),
        "a corpus check over a program that composes no parser is never a pass"
    );
    let mut coverage = BTreeMap::<String, BTreeSet<String>>::new();
    for adapter in &adapters {
        for scenario in SCENARIOS {
            let document = fixture_document(root, adapter, scenario);
            assert_eq!(document["schemaVersion"], "lico.adapter-transcript.v1");
            assert_eq!(document["adapterId"], adapter.as_str());
            assert_eq!(document["scenario"], scenario);
            assert_eq!(document["provenance"]["redacted"], true);
            assert_eq!(document["invocation"]["readOnly"], true);
            replay(set, &document, adapter, scenario).unwrap_or_else(|error| panic!("{error}"));
            coverage
                .entry(adapter.clone())
                .or_default()
                .insert(scenario.to_owned());
        }
    }
    assert_eq!(coverage.len(), adapters.len());
    for adapter in &adapters {
        assert_eq!(coverage[adapter].len(), SCENARIOS.len(), "{adapter}");
    }
    coverage
}

/// Record the projections of every fixture in a corpus copy from the real
/// parsers, and report how many were recorded.
///
/// An absent fixture is not recorded and not claimed; the replay check still
/// fails closed on a corpus that is missing any composed adapter's fixture.
pub fn record_projections(set: &AdapterParserSet, root: &Path) -> usize {
    let mut recorded = 0;
    for adapter in registered_adapters(set) {
        for scenario in SCENARIOS {
            let path = root.join(&adapter).join(format!("{scenario}.json"));
            if !path.is_file() {
                continue;
            }
            let mut document = fixture_document(root, &adapter, scenario);
            let frames = recorded_frames(&document, &adapter, scenario);
            let mut parser = adapters::replay_for(set, &adapter).unwrap_or_else(|error| {
                panic!("adapter={adapter} scenario={scenario}: {error}");
            });
            let mut projections = Vec::with_capacity(frames.len());
            for frame in &frames {
                projections.push(Value::Array(parser.feed(frame).unwrap_or_else(|error| {
                    panic!(
                        "adapter={adapter} scenario={scenario} frame={}: {error}",
                        frame.index
                    )
                })));
            }
            if let Some(stored) = document.get_mut("frames").and_then(Value::as_array_mut) {
                for (slot, projection) in stored.iter_mut().zip(projections) {
                    slot["projection"] = projection;
                }
            }
            let mut encoded = serde_json::to_string_pretty(&document)
                .expect("recorded fixture must serialize")
                .into_bytes();
            encoded.push(b'\n');
            fs::write(&path, encoded)
                .unwrap_or_else(|error| panic!("cannot write recorded fixture {path:?}: {error}"));
            recorded += 1;
        }
    }
    recorded
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An adapter id no program composes. The harness is neutral about which
    /// Agents exist; every check that needs a real parser is owned by the
    /// program composing one.
    const UNCOMPOSED_ADAPTER: &str = "fixture-adapter";

    #[test]
    #[should_panic(expected = "replay fixture missing or unreadable")]
    fn adapter_replay_fails_closed_on_a_missing_corpus() {
        let _ = fixture_document(
            Path::new("/nonexistent-replay-corpus"),
            UNCOMPOSED_ADAPTER,
            "normal-turn",
        );
    }

    #[test]
    fn adapter_replay_fails_closed_on_a_transcript_without_frames() {
        let error = replay(
            &AdapterParserSet::unavailable(),
            &serde_json::json!({ "frames": [] }),
            UNCOMPOSED_ADAPTER,
            "normal-turn",
        )
        .expect_err("a transcript with no frames is not a passed replay");
        assert!(error.contains("frames missing or empty"), "{error}");
    }

    #[test]
    fn adapter_replay_refuses_to_replay_an_unregistered_adapter() {
        let error = adapters::replay_for(&AdapterParserSet::unavailable(), UNCOMPOSED_ADAPTER)
            .err()
            .expect("an unregistered adapter has no replayable parser");
        assert!(error.contains(UNCOMPOSED_ADAPTER), "{error}");
    }

    #[test]
    fn unavailable_parser_set_reports_no_registration() {
        let set = AdapterParserSet::unavailable();
        assert!(set.registered_ids().is_empty());
        assert!(set.contract(UNCOMPOSED_ADAPTER).is_none());
        assert!(set.registration(UNCOMPOSED_ADAPTER).is_none());
        assert_eq!(
            set.framing(UNCOMPOSED_ADAPTER),
            Err(format!(
                "no registered contract for adapter {UNCOMPOSED_ADAPTER}"
            ))
        );
    }
}
