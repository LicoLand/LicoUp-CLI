use super::snapshot_cursor::SnapshotCursor;
use super::watermark::WatermarkProjection;
use licoup_statistics::contract::{HistoryUsageSummary, UsageVariant};
use licoup_statistics::variant::UsageRequestContext;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceMetadata {
    pub(super) modified_ns: u64,
    pub(super) size: u64,
    pub(super) file_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct CumulativeTotals {
    pub(super) prompt: u64,
    pub(super) cached: u64,
    pub(super) completion: u64,
}

impl CumulativeTotals {
    pub(super) fn at_least(self, previous: Self) -> bool {
        self.prompt >= previous.prompt
            && self.cached >= previous.cached
            && self.completion >= previous.completion
    }

    pub(super) fn delta(self, previous: Self) -> Self {
        Self {
            prompt: self.prompt.saturating_sub(previous.prompt),
            cached: self.cached.saturating_sub(previous.cached),
            completion: self.completion.saturating_sub(previous.completion),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CachedSource {
    pub(super) modified_ns: u64,
    pub(super) size: u64,
    pub(super) file_id: Option<String>,
    pub(super) parsed_bytes: u64,
    pub(super) append_guard: String,
    pub(super) session_count: u64,
    pub(super) sealed: bool,
    pub(super) request_context: UsageRequestContext,
    /// 1 pending; 2/3 append cursor; 4/5 snapshot cursor. Odd ready states retain a gap.
    pub(super) migration_state: i64,
    pub(super) snapshot_cursor: Option<SnapshotCursor>,
}

#[derive(Clone, Debug)]
pub(super) struct CumulativeSnapshot {
    pub(super) usage_key: String,
    pub(super) session_key: String,
    pub(super) model: Option<String>,
    pub(super) variant: UsageVariant,
    pub(super) first_day: String,
    pub(super) observed_day: String,
    pub(super) totals: CumulativeTotals,
    pub(super) projects_usage: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ParseResult {
    pub(super) summary: HistoryUsageSummary,
    pub(super) parsed_bytes: u64,
    pub(super) cumulative_snapshots: Vec<CumulativeSnapshot>,
    pub(super) session_increment: u64,
    pub(super) request_context: UsageRequestContext,
}

/// Immutable two-phase refresh plan. Built entirely outside any database
/// lease; the apply transaction only revalidates and writes it.
#[derive(Clone, Debug)]
pub(super) struct RefreshPlan {
    pub(super) previous: BTreeMap<String, CachedSource>,
    pub(super) compaction_targets: BTreeSet<String>,
    pub(super) compactions: Vec<(String, u64)>,
    pub(super) sources: Vec<PlannedSource>,
}

#[derive(Clone, Debug)]
pub(super) struct PlannedSource {
    pub(super) key: String,
    pub(super) path: PathBuf,
    pub(super) metadata: SourceMetadata,
    pub(super) action: PlannedSourceAction,
}

#[derive(Clone, Debug)]
pub(super) enum PlannedSourceAction {
    Migrate {
        baseline: Box<ParseResult>,
        tail: Option<Box<ParseResult>>,
        complete: bool,
        append_format: bool,
        append_guard: String,
    },
    Reuse,
    RefreshIdentity {
        parsed: Option<Box<ParseResult>>,
    },
    ReuseSeal {
        session_count: u64,
    },
    Refresh {
        append: bool,
        append_format: bool,
        previous_session_count: u64,
        parsed: Box<ParseResult>,
        projection: WatermarkProjection,
        append_guard: String,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct ScanStats {
    pub(super) discovered_sources: u64,
    pub(super) reused_sources: u64,
    pub(super) identity_refreshed_sources: u64,
    pub(super) identity_skipped_sources: u64,
    pub(super) appended_sources: u64,
    pub(super) replaced_sources: u64,
    pub(super) sealed_sources: u64,
    pub(super) compacted_days: u64,
    pub(super) parsed_bytes: u64,
    pub(super) cache_fresh: bool,
    pub(super) rebuilt: bool,
    pub(super) opened_connections: u64,
    pub(super) leases: u64,
    pub(super) statements: u64,
    pub(super) transaction_millis: u64,
}

impl ScanStats {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "schemaVersion": super::cache::CACHE_SCHEMA_VERSION,
            "parserRevision": super::PARSER_REVISION,
            "fresh": self.cache_fresh,
            "discoveredSources": self.discovered_sources,
            "reusedSources": self.reused_sources,
            "identityRefreshedSources": self.identity_refreshed_sources,
            "identitySkippedSources": self.identity_skipped_sources,
            "appendedSources": self.appended_sources,
            "replacedSources": self.replaced_sources,
            "sealedSources": self.sealed_sources,
            "compactedDays": self.compacted_days,
            "parsedBytes": self.parsed_bytes,
            "rebuilt": self.rebuilt,
            "connectionOpens": self.opened_connections,
            "leases": self.leases,
            "statements": self.statements,
            "transactionMillis": self.transaction_millis
        })
    }
}
