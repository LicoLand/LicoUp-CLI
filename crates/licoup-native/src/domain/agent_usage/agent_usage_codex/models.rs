use super::constants::{CACHE_SCHEMA_VERSION, PARSER_REVISION};
use licoup_statistics::contract::{UsageVariant, number_field};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TokenTotals {
    pub(super) input: u64,
    pub(super) cached: u64,
    pub(super) output: u64,
}

impl TokenTotals {
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        let input = number_field(
            value,
            &[
                "input_tokens",
                "inputTokens",
                "prompt_tokens",
                "promptTokens",
            ],
        );
        let cached = number_field(
            value,
            &[
                "cached_input_tokens",
                "cachedInputTokens",
                "cache_read_input_tokens",
                "cacheReadInputTokens",
            ],
        );
        let output = number_field(
            value,
            &[
                "output_tokens",
                "outputTokens",
                "completion_tokens",
                "completionTokens",
            ],
        );
        if input.is_none() && cached.is_none() && output.is_none() {
            return None;
        }
        let input = input.unwrap_or(0);
        Some(Self {
            input,
            cached: cached.unwrap_or(0).min(input),
            output: output.unwrap_or(0),
        })
    }

    pub(super) fn saturating_delta(self, baseline: Self) -> Self {
        Self {
            input: self.input.saturating_sub(baseline.input),
            cached: self.cached.saturating_sub(baseline.cached),
            output: self.output.saturating_sub(baseline.output),
        }
    }

    pub(super) fn add(self, delta: Self) -> Self {
        Self {
            input: self.input.saturating_add(delta.input),
            cached: self.cached.saturating_add(delta.cached),
            output: self.output.saturating_add(delta.output),
        }
    }

    pub(super) fn is_zero(self) -> bool {
        self.input == 0 && self.cached == 0 && self.output == 0
    }

    pub(super) fn at_least(self, other: Self) -> bool {
        self.input >= other.input && self.cached >= other.cached && self.output >= other.output
    }

    pub(super) fn at_most(self, other: Self) -> bool {
        self.input <= other.input && self.cached <= other.cached && self.output <= other.output
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct ParserState {
    pub(super) session_id: Option<String>,
    pub(super) forked_from_id: Option<String>,
    pub(super) current_model: Option<String>,
    pub(super) current_turn_id: Option<String>,
    pub(super) current_variant: UsageVariant,
    pub(super) pending_context: bool,
    pub(super) raw_totals: Option<TokenTotals>,
    pub(super) counted_totals: Option<TokenTotals>,
    pub(super) has_divergent_totals: bool,
    pub(super) next_event_index: u64,
    pub(super) token_chain_hash: String,
}

#[derive(Clone, Debug)]
pub(super) struct CachedFile {
    pub(super) modified_ns: u64,
    pub(super) size: u64,
    pub(super) file_id: Option<String>,
    pub(super) parsed_bytes: u64,
    pub(super) append_guard: String,
    pub(super) state: ParserState,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ScanStats {
    pub(super) discovered_files: u64,
    pub(super) reused_files: u64,
    pub(super) identity_refreshed_files: u64,
    pub(super) identity_skipped_files: u64,
    pub(super) appended_files: u64,
    pub(super) rescanned_files: u64,
    pub(super) parsed_bytes: u64,
    pub(super) cache_fresh: bool,
    pub(super) refresh_deferred: bool,
}

impl ScanStats {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "schemaVersion": CACHE_SCHEMA_VERSION,
            "parserRevision": PARSER_REVISION,
            "fresh": self.cache_fresh,
            "discoveredFiles": self.discovered_files,
            "reusedFiles": self.reused_files,
            "identityRefreshedFiles": self.identity_refreshed_files,
            "identitySkippedFiles": self.identity_skipped_files,
            "appendedFiles": self.appended_files,
            "rescannedFiles": self.rescanned_files,
            "parsedBytes": self.parsed_bytes,
            "refreshDeferred": self.refresh_deferred
        })
    }
}
