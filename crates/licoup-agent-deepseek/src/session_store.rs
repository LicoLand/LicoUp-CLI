//! The Harness's own durable session log, read natively.
//!
//! The installed DeepSeek Harness records each session as an artifact under
//! `<harness home>/sessions/<project>/<session>/session[.vN].jsonl[.zstd]`. The
//! first row is the session header; every later row is one event with a `seq`, a
//! `time` in milliseconds and a `data` object. This module is LicoUp's own
//! reader for that format — the usage half of the client no longer starts a Node
//! worker against the vendor's persistence libraries, and the vendor's
//! libraries are not a runtime dependency of this package.
//!
//! # What this reader folds
//!
//! Usage is a property of what the model was actually asked, so the fold follows
//! the Harness's own meter:
//!
//! - `request/header` replaces the deadline's route and effort defaults. A later
//!   header does not inherit from an earlier one.
//! - `assistant/message` and `assistant/attempt` each report one attempt's
//!   usage. A settlement with no token usage at all does not erase an earlier
//!   settlement of the same attempt.
//! - `llm/retry-started` closes the attempt it names, so the retry's own
//!   settlement is counted separately rather than replacing the attempt it
//!   superseded.
//! - A row below the header's inherited prefix is a fork's copied ancestry, not
//!   consumption this session caused, so it is not counted again.
//!
//! # What this reader refuses
//!
//! The physical format is versioned and the versions are not interchangeable.
//! This reader implements [`CURRENT_FORMAT_VERSION`]; a header that declares any
//! other version is reported as
//! [`SessionReadError::UnsupportedFormatVersion`] with the version it declared,
//! so a caller can tell "this is a generation LicoUp does not read" apart from
//! "this file is not a session log". Reporting a count from a generation whose
//! schema this reader has not been written against would be a guess, and a usage
//! meter may not guess.
//!
//! The compression is a concatenated Zstandard container: one independently
//! decodable, checksummed frame per durable batch. A plain-text artifact and a
//! compressed one fold identically.
//!
//! # The external dependency this reader declares
//!
//! The session log is the vendor's artifact and its framing, versioning and
//! migration chain are owned by the vendor. LicoUp reads the generation it
//! declares here and refuses the rest explicitly; it does not claim to track a
//! format the vendor has not published.

use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

/// The one physical session-log generation this reader is written against.
///
/// The header row declares it, so a reader never has to guess from a file name:
/// the artifact decides whether it is readable, and an artifact of another
/// generation is refused by name rather than folded as if it were this one.
pub const CURRENT_FORMAT_VERSION: u64 = 4;

/// The suffix that marks a Zstandard-framed artifact.
pub const COMPRESSED_SUFFIX: &str = ".zstd";

/// One usage sample, as the Harness recorded it.
///
/// Nothing here is projected or interpreted: `usage` is the vendor's own token
/// object, handed on unchanged, and the route and effort are the values the
/// attempt was actually made with.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageSample {
    /// The event's own sequence number, which is also its position in the log.
    pub seq: u64,
    /// The assistant message identity, when the row carried one.
    pub message_id: Option<String>,
    /// The event's wall-clock time in milliseconds.
    pub time: u64,
    /// The turn the attempt belongs to.
    pub turn: Option<u64>,
    /// The step within the turn.
    pub step: Option<u64>,
    /// The model the attempt was actually made with.
    pub model: Option<String>,
    /// The provider the attempt was actually made with.
    pub provider: Option<String>,
    /// The reasoning effort the attempt was made with, when the route default
    /// applied to it.
    pub effort: Option<String>,
    /// The vendor's own token usage object, when the row reported one.
    pub usage: Option<Value>,
}

/// Why one session artifact could not be read.
#[derive(Debug)]
pub enum SessionReadError {
    /// The artifact could not be opened or read.
    Io(io::Error),
    /// The artifact is not a session log this reader understands.
    Malformed(&'static str),
    /// The header declares a generation this reader is not written against.
    ///
    /// Reported rather than folded: a caller must be able to refuse a session
    /// instead of publishing a count from a schema nobody has read.
    UnsupportedFormatVersion(u64),
}

impl std::fmt::Display for SessionReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "session artifact could not be read: {error}"),
            Self::Malformed(reason) => write!(formatter, "session artifact is malformed: {reason}"),
            Self::UnsupportedFormatVersion(version) => write!(
                formatter,
                "session artifact declares format version {version}, which this reader does not read"
            ),
        }
    }
}

impl std::error::Error for SessionReadError {}

impl From<io::Error> for SessionReadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Read one session artifact and fold its events into usage samples.
///
/// The artifact may be plain text or a concatenated-Zstandard container; the
/// suffix decides, exactly as the Harness's own layout decides. Rows are folded
/// as they arrive, so a long session never has to be held in memory.
pub fn read_usage_samples(path: &Path) -> Result<Vec<UsageSample>, SessionReadError> {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(COMPRESSED_SUFFIX))
    {
        let decoder = zstd::stream::read::Decoder::new(File::open(path)?)
            .map_err(|_| SessionReadError::Malformed("zstandard container could not be decoded"))?;
        fold_rows(BufReader::new(decoder))
    } else {
        fold_rows(BufReader::new(File::open(path)?))
    }
}

/// The header a session artifact declares.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionHeader {
    /// The physical format generation the artifact was written in.
    pub version: u64,
    /// The durable session identity.
    pub id: String,
    /// The exact fork-inherited prefix length, in events.
    pub inherited_event_count: u64,
}

/// Fold one decoded artifact: its header, then its events.
fn fold_rows(reader: impl BufRead) -> Result<Vec<UsageSample>, SessionReadError> {
    let (header, mut fold) = read_header(reader)?;
    fold.inherited = header.inherited_event_count;
    fold.finish()
}

/// Read the header row and hand back the fold primed with its inherited cut.
fn read_header(mut reader: impl BufRead) -> Result<(SessionHeader, Fold), SessionReadError> {
    let Some(row) = next_row(&mut reader)? else {
        return Err(SessionReadError::Malformed(
            "the artifact carries no header row",
        ));
    };
    if row.get("type").and_then(Value::as_str) != Some("session") {
        return Err(SessionReadError::Malformed(
            "the first row is not a session header",
        ));
    }
    let version = row
        .get("version")
        .and_then(Value::as_u64)
        .ok_or(SessionReadError::Malformed(
            "the header carries no format version",
        ))?;
    if version != CURRENT_FORMAT_VERSION {
        return Err(SessionReadError::UnsupportedFormatVersion(version));
    }
    let header = translate_header(&row)?;
    let mut fold = Fold::default();
    fold.absorb_rows(&mut reader)?;
    Ok((header, fold))
}

/// Translate one current physical header into the metadata a reader needs.
fn translate_header(row: &Value) -> Result<SessionHeader, SessionReadError> {
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or(SessionReadError::Malformed(
            "the header carries no session id",
        ))?
        .to_owned();
    // A fork records its inherited prefix beside the header. It is absent on an
    // unseeded session, whose prefix is zero by definition rather than unknown.
    let inherited_event_count = row
        .get("inheritedEventCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(SessionHeader {
        version: CURRENT_FORMAT_VERSION,
        id,
        inherited_event_count,
    })
}

/// The next non-empty row of the artifact, parsed as one JSON value.
fn next_row(reader: &mut impl BufRead) -> Result<Option<Value>, SessionReadError> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return serde_json::from_str(trimmed)
            .map(Some)
            .map_err(|_| SessionReadError::Malformed("a row is not valid JSON"));
    }
}

/// One attempt's route and the samples it settled.
#[derive(Default)]
struct Fold {
    /// The inherited prefix length: rows below it are copied ancestry.
    inherited: u64,
    /// The route the current header declares, replaced on every header.
    route: Option<Route>,
    /// Samples in arrival order.
    samples: Vec<UsageSample>,
    /// The attempt the last sample belongs to, so a repeated settlement of the
    /// same attempt replaces it instead of counting it twice.
    open: Option<OpenAttempt>,
}

/// The route one `request/header` row declares.
#[derive(Clone, Debug, Default)]
struct Route {
    model: Option<String>,
    provider: Option<String>,
    reasoning_effort: Option<String>,
    default_reasoning_effort: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OpenAttempt {
    turn: Option<u64>,
    step: Option<u64>,
    index: usize,
}

impl Fold {
    /// Absorb every event row of the artifact.
    fn absorb_rows(&mut self, reader: &mut impl BufRead) -> Result<(), SessionReadError> {
        while let Some(row) = next_row(reader)? {
            self.absorb(row);
        }
        Ok(())
    }

    /// Fold one event row.
    fn absorb(&mut self, row: Value) {
        let Some(kind) = row.get("type").and_then(Value::as_str) else {
            return;
        };
        let data = row.get("data").cloned().unwrap_or(Value::Null);
        if kind == "request/header" {
            self.route = Some(Route {
                model: text(data.pointer("/header/config/model")),
                provider: text(data.pointer("/header/config/provider")),
                reasoning_effort: text(data.pointer("/header/config/reasoningEffort")),
                default_reasoning_effort: text(
                    data.pointer("/header/adapterDefaults/reasoningEffort"),
                ),
            });
            return;
        }
        let seq = row.get("seq").and_then(Value::as_u64).unwrap_or_default();
        let turn = data.get("turn").and_then(Value::as_u64);
        let step = data.get("step").and_then(Value::as_u64);
        if kind == "llm/retry-started" {
            // The attempt this names is superseded by the retry, so the retry's
            // own settlement is counted separately rather than replacing it.
            if self
                .samples
                .last()
                .is_some_and(|sample| sample.turn == turn && sample.step == step)
            {
                self.open = None;
            }
            return;
        }
        if kind != "assistant/message" && kind != "assistant/attempt" {
            return;
        }
        if seq < self.inherited {
            return;
        }
        let sample = self.sample(kind, &row, &data, seq, turn, step);
        match self.open {
            // A later tokenless settlement does not erase an earlier reported
            // sample of the same attempt.
            Some(open) if open.turn == turn && open.step == step => {
                if sample.usage.is_some() || self.samples[open.index].usage.is_none() {
                    self.samples[open.index] = sample;
                }
            }
            _ => {
                self.samples.push(sample);
                self.open = Some(OpenAttempt {
                    turn,
                    step,
                    index: self.samples.len() - 1,
                });
            }
        }
    }

    /// Build one sample from the row's own facts.
    fn sample(
        &self,
        kind: &str,
        row: &Value,
        data: &Value,
        seq: u64,
        turn: Option<u64>,
        step: Option<u64>,
    ) -> UsageSample {
        let route = self.route.clone().unwrap_or_default();
        // A message may name the route it was actually made on; when it does,
        // that route wins, and the header's effort only applies while the two
        // agree.
        let message = data.get("message");
        let source_model = text(message.and_then(|message| message.pointer("/source/model")))
            .or_else(|| route.model.clone());
        let source_provider = text(message.and_then(|message| message.pointer("/source/provider")))
            .or_else(|| route.provider.clone());
        let same_route = source_model == route.model && source_provider == route.provider;
        let usage = if kind == "assistant/message" {
            data.get("usage")
                .filter(|usage| !usage.is_null())
                .cloned()
                .or_else(|| last_stream_usage(data.get("stream")))
        } else {
            last_stream_usage(data.get("stream"))
        };
        UsageSample {
            seq,
            message_id: text(message.and_then(|message| message.get("id"))),
            time: row.get("time").and_then(Value::as_u64).unwrap_or_default(),
            turn,
            step,
            model: source_model,
            provider: source_provider,
            effort: same_route
                .then(|| {
                    route
                        .reasoning_effort
                        .clone()
                        .or_else(|| route.default_reasoning_effort.clone())
                })
                .flatten(),
            usage,
        }
    }

    /// The folded samples, in arrival order.
    fn finish(self) -> Result<Vec<UsageSample>, SessionReadError> {
        Ok(self.samples)
    }
}

/// The last recorded usage chunk of a stream, which is the vendor's own
/// "settled" value for the attempt.
fn last_stream_usage(stream: Option<&Value>) -> Option<Value> {
    let records = stream?.as_array()?;
    records.iter().rev().find_map(|record| {
        (record.pointer("/chunk/type").and_then(Value::as_str) == Some("usage"))
            .then(|| record.pointer("/chunk/usage").cloned())
            .flatten()
    })
}

/// Read a JSON string field, refusing a value that is not a string.
fn text(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned)
}

/// The session artifacts under one root, keyed by the session directory they
/// belong to, with the newest generation of each directory selected.
///
/// Kept here because "which file is the current generation of this session" is
/// a fact about the vendor's layout, not about the client that asks.
pub fn newest_generation(
    sources: BTreeMap<std::path::PathBuf, String>,
) -> BTreeMap<std::path::PathBuf, String> {
    let mut sessions = BTreeMap::<std::path::PathBuf, (u64, std::path::PathBuf, String)>::new();
    for (path, kind) in sources {
        let Some(version) = generation(&path) else {
            continue;
        };
        let Some(directory) = path.parent() else {
            continue;
        };
        let slot = sessions
            .entry(directory.to_path_buf())
            .or_insert_with(|| (version, path.clone(), kind.clone()));
        if version > slot.0 {
            *slot = (version, path, kind);
        }
    }
    sessions
        .into_values()
        .map(|(_, path, kind)| (path, kind))
        .collect()
}

/// The generation one artifact file name declares, when it declares one.
fn generation(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(COMPRESSED_SUFFIX).unwrap_or(name);
    let name = name.strip_suffix(".jsonl")?;
    match name.strip_prefix("session.v") {
        // An unversioned artifact is the generation the format started at.
        None => name.eq("session").then_some(0),
        Some(version) => version.parse().ok(),
    }
}
