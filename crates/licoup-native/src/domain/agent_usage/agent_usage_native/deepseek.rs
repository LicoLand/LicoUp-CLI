//! Exact usage from the installed Harness's read-only session persistence API.
//! The provider owns format migration, compression and inherited-prefix decoding.

use super::models::ParseResult;
use crate::domain::conversation::source_catalog::deepseek_generation;
use anyhow::{Context, Result};
use licoup_statistics::contract::{HistoryUsageSummary, MessageUsage};
use licoup_statistics::variant::{UsageVariant, model_label};
use licoup_statistics::window::UsageWindow;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const READER: &str = include_str!("deepseek_reader.mjs");
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

#[derive(Debug)]
pub(super) struct ReadFailure(pub(super) &'static str);
impl std::fmt::Display for ReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeepSeek Harness usage {} failed", self.0)
    }
}
impl std::error::Error for ReadFailure {}

/// Generations are representations of one session, never additional consumption.
pub(super) fn canonical_sources(sources: BTreeMap<PathBuf, String>) -> BTreeMap<PathBuf, String> {
    let mut sessions = BTreeMap::<PathBuf, (u64, PathBuf, String)>::new();
    for (path, kind) in sources {
        let Some(version) = deepseek_generation(&path) else {
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

#[derive(Default)]
pub(super) struct Reader {
    process: Option<ReaderProcess>,
}

struct ReaderProcess {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Drop for ReaderProcess {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}

impl Reader {
    pub(super) fn parse(
        &mut self,
        path: &Path,
        size: u64,
        calendar: &UsageWindow,
    ) -> Result<ParseResult> {
        if self.process.is_none() {
            self.process = Some(Self::start().context(ReadFailure("reader-start"))?);
        }
        self.read(path, size, calendar)
            .context(ReadFailure("session-read"))
    }

    fn read(&mut self, path: &Path, size: u64, calendar: &UsageWindow) -> Result<ParseResult> {
        let process = self.process.as_mut().expect("reader started");
        let input = process
            .input
            .as_mut()
            .context("DeepSeek Harness reader closed")?;
        serde_json::to_writer(&mut *input, &json!({"path":path}))?;
        input.write_all(b"\n")?;
        input.flush()?;
        let mut line = String::new();
        anyhow::ensure!(
            process.output.read_line(&mut line)? > 0,
            "DeepSeek Harness usage reader stopped"
        );
        let response: ReaderResponse =
            serde_json::from_str(&line).context("DeepSeek Harness reader metadata invalid")?;
        let output = response
            .ok
            .context("DeepSeek Harness usage source could not be decoded")?;
        summarize_samples(output, size, calendar)
    }

    fn start() -> Result<ReaderProcess> {
        let program = licoup_agent_targets::domain::targets::find_binary(&["dsh"])
            .context("DeepSeek Harness reader unavailable")?;
        let node = licoup_agent_targets::domain::targets::find_binary(&["node"])
            .context("DeepSeek Harness Node runtime unavailable")?;
        anyhow::ensure!(
            [&program, &node].into_iter().all(|path| {
                crate::domain::targets::scan_paths::discovered_agent_may_execute(path, true)
            }),
            "DeepSeek Harness reader execution denied"
        );
        let mut child = Command::new(node)
            .args([
                "--input-type=module",
                "--eval",
                READER,
                "--",
                "--licoup-deepseek-usage",
            ])
            .arg(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("DeepSeek Harness reader failed")?;
        let input = child.stdin.take();
        let output = BufReader::new(
            child
                .stdout
                .take()
                .context("DeepSeek Harness reader output unavailable")?,
        );
        Ok(ReaderProcess {
            child,
            input,
            output,
        })
    }
}

#[derive(Deserialize)]
struct ReaderResponse {
    ok: Option<ReaderOutput>,
}

#[derive(Deserialize)]
struct ReaderOutput {
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    time: Value,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
    usage: Option<Value>,
}

#[cfg(test)]
fn parse_samples(bytes: &[u8], size: u64, calendar: &UsageWindow) -> Result<ParseResult> {
    summarize_samples(serde_json::from_slice(bytes)?, size, calendar)
}

fn summarize_samples(
    output: ReaderOutput,
    size: u64,
    calendar: &UsageWindow,
) -> Result<ParseResult> {
    let mut summary = HistoryUsageSummary::default();
    let mut saw_session = false;
    for sample in output.samples {
        let timestamp = sample
            .time
            .as_u64()
            .context("DeepSeek Harness usage timestamp invalid")?;
        let Some(day) = calendar
            .date_key(&timestamp.to_string())
            .filter(|day| calendar.contains(day))
        else {
            continue;
        };
        saw_session = true;
        let model = model_label(&json!({"model":sample.model,"providerId":sample.provider}));
        let variant = UsageVariant::from_metadata(&json!({"reasoningEffort":sample.effort}));
        if let Some(mut usage) = sample.usage.as_ref().and_then(exact_usage) {
            usage.model = model;
            usage.variant = variant;
            summary.add(usage, Some(day));
        } else {
            summary.add_token_unavailable_request_with_variant(Some(day), model, variant);
        }
    }
    summary.session_count = u64::from(saw_session);
    Ok(ParseResult {
        session_increment: summary.session_count,
        summary,
        parsed_bytes: size,
        ..Default::default()
    })
}

/// Harness uses disjoint input/cache buckets; reasoning is a subset of output.
/// Its public session meter aggregates absent optional cache buckets as zero.
/// Retain those reported counts even when a full-call total was not recorded;
/// supplied totals and counts must still satisfy the consistency checks below.
fn exact_usage(value: &Value) -> Option<MessageUsage> {
    let count = |key| {
        value
            .get(key)?
            .as_u64()
            .filter(|count| *count <= MAX_SAFE_INTEGER)
    };
    let input = count("inputTokens")?;
    let output = count("outputTokens")?;
    let optional = |key| {
        if value.get(key).is_some() {
            count(key).map(Some)
        } else {
            Some(None)
        }
    };
    let read = optional("cacheReadTokens")?;
    let write = optional("cacheWriteTokens")?;
    let reasoning = optional("reasoningTokens")?;
    if reasoning.is_some_and(|reasoning| reasoning > output) {
        return None;
    }
    let known_prompt = input
        .checked_add(read.unwrap_or(0))?
        .checked_add(write.unwrap_or(0))?;
    let total = optional("totalTokens")?.or_else(|| known_prompt.checked_add(output))?;
    if total > MAX_SAFE_INTEGER {
        return None;
    }
    let prompt = total.checked_sub(output)?;
    if prompt < known_prompt || (read.is_some() && write.is_some() && prompt != known_prompt) {
        return None;
    }
    Some(MessageUsage {
        prompt_tokens: prompt,
        cached_input_tokens: read.unwrap_or(0),
        completion_tokens: output,
        total_tokens: total,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_disjoint_buckets_include_reasoning_once_and_refuse_inconsistent_totals() {
        let usage = exact_usage(&json!({"inputTokens":70,"cacheReadTokens":30,"outputTokens":20,"reasoningTokens":15,"totalTokens":120})).unwrap();
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.cached_input_tokens,
                usage.completion_tokens,
                usage.total_tokens
            ),
            (100, 30, 20, 120)
        );
        let meter = exact_usage(
            &json!({"inputTokens":70,"cacheReadTokens":30,"outputTokens":20,"reasoningTokens":15}),
        )
        .unwrap();
        assert_eq!(
            (
                meter.prompt_tokens,
                meter.cached_input_tokens,
                meter.completion_tokens,
                meter.total_tokens
            ),
            (100, 30, 20, 120)
        );
        assert_eq!(
            exact_usage(&json!({"inputTokens":70,"outputTokens":20}))
                .unwrap()
                .total_tokens,
            90
        );
        assert!(
            exact_usage(&json!({"inputTokens":70,"outputTokens":20,"cacheReadTokens":-1}))
                .is_none()
        );
        assert!(
            exact_usage(&json!({"inputTokens":70,"outputTokens":20,"cacheWriteTokens":null}))
                .is_none()
        );
        assert!(exact_usage(&json!({"inputTokens":70,"outputTokens":1.5})).is_none());
        assert!(
            exact_usage(&json!({"inputTokens":9007199254740991_u64,"outputTokens":1})).is_none()
        );
        assert!(
            exact_usage(
                &json!({"inputTokens":70,"outputTokens":20,"reasoningTokens":21,"totalTokens":90})
            )
            .is_none()
        );
        assert!(exact_usage(&json!({"inputTokens":70,"cacheReadTokens":30,"cacheWriteTokens":0,"outputTokens":20,"totalTokens":121})).is_none());
        assert_eq!(exact_usage(&json!({"inputTokens":70,"cacheReadTokens":30,"cacheWriteTokens":4,"outputTokens":20})).unwrap().total_tokens,124);
    }

    #[test]
    fn canonical_generation_has_one_stable_session_source() {
        let sources = [
            "session.jsonl.zstd",
            "session.v1.jsonl.zstd",
            "session.v3.jsonl.zstd",
            "session.v03.jsonl.zstd",
            "session.v4.jsonl.zstd.tmp",
        ]
        .into_iter()
        .map(|name| (PathBuf::from("root/project/id").join(name), "test".into()))
        .collect();
        let selected = canonical_sources(sources);
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected.first_key_value().unwrap().0.file_name().unwrap(),
            "session.v3.jsonl.zstd"
        );
    }

    #[test]
    fn exact_snapshot_replacement_and_reopen_keep_tokens_requests_and_effort_once() {
        use super::super::cache::{RefreshStatements, aggregate_usage, open_cache_database};
        let root =
            std::env::temp_dir().join(format!("licoup-harness-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("usage.sqlite");
        let calendar = UsageWindow::from_params(
            &json!({"now":"2026-07-15T12:00:00Z", "historyDays":1, "timezoneOffsetMinutes":480}),
        );
        let initial = json!({"samples":[
            {"time":1784109600000_u64,"model":"deepseek-native","provider":"deepseek-official","effort":"high","usage":{"inputTokens":70,"cacheReadTokens":30,"outputTokens":20,"reasoningTokens":15}},
            {"time":1784109600001_u64,"model":"deepseek-native","provider":"deepseek-official","usage":null}
        ]});
        let parsed = parse_samples(&serde_json::to_vec(&initial).unwrap(), 100, &calendar).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 120);
        assert_eq!(parsed.summary.token_unavailable_records, 1);
        let mut changed = initial.clone();
        changed["samples"].as_array_mut().unwrap().push(json!({"time":1784109600002_u64,"model":"deepseek-native","provider":"deepseek-official","effort":"low","usage":{"inputTokens":4,"outputTokens":2,"totalTokens":6}}));
        let changed =
            parse_samples(&serde_json::to_vec(&changed).unwrap(), 200, &calendar).unwrap();
        let mut connection = open_cache_database(&path).unwrap();
        // Re-reading an unchanged generation and publishing its successor both
        // replace this session's mutable daily row rather than append old usage.
        for summary in [&parsed.summary, &parsed.summary, &changed.summary] {
            let transaction = connection.transaction().unwrap();
            let mut statements = RefreshStatements::prepare(&transaction).unwrap();
            statements
                .replace_rollup("scope", "session", summary)
                .unwrap();
            drop(statements);
            transaction.commit().unwrap();
        }
        drop(connection);
        let mut connection = open_cache_database(&path).unwrap();
        let summary = aggregate_usage(&mut connection, "scope", &calendar).unwrap();
        assert_eq!(summary.total_tokens(), 126);
        assert_eq!(summary.estimated_records, 0);
        assert_eq!(summary.token_unavailable_records, 1);
        let day = &summary.daily_usage["2026-07-15"];
        assert_eq!(day.request_count, 3);
        assert_eq!(
            day.model_variants
                .values()
                .map(|usage| usage.total_tokens)
                .sum::<u64>(),
            126
        );
        assert_eq!(
            day.model_variants
                .values()
                .map(|usage| usage.request_count)
                .sum::<u64>(),
            3
        );
        assert_eq!(
            day.model_variants
                .iter()
                .find(|((_, variant), _)| variant.effort.as_deref() == Some("high"))
                .unwrap()
                .1
                .total_tokens,
            120
        );
        assert_eq!(
            day.model_variants
                .iter()
                .find(|((_, variant), _)| variant.effort.as_deref() == Some("low"))
                .unwrap()
                .1
                .total_tokens,
            6
        );
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn harness_history_roots_honor_explicit_home_without_loading_configuration() {
        use crate::domain::conversation::source_catalog::{HistoryAdapter, history_roots};
        let root = PathBuf::from("fixture").join("harness");
        let roots = history_roots(
            HistoryAdapter::DeepSeekHarness,
            &json!({"homeDir":"fixture-home", "deepseekHarnessHome":root}),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, root.join("sessions"));
        assert_eq!(roots[0].source_kind, "deepseek-harness-session-store");
    }
}
