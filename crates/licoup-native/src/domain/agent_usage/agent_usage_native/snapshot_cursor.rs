//! Numeric cursor for mutable snapshots whose earlier ledger is already sealed.

use licoup_statistics::contract::{
    DailyUsageSummary, HistoryUsageSummary, ModelTokenUsageSummary, UNATTRIBUTED_MODEL,
    UsageVariant,
};
use licoup_statistics::window::UsageWindow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct SnapshotCursor {
    pub(super) day: String,
    pub(super) session_count: u64,
    days: BTreeMap<String, SnapshotDay>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct SnapshotDay {
    totals: ModelTokenUsageSummary,
    explicit_records: u64,
    estimated_records: u64,
    message_count: u64,
    models: Vec<((String, UsageVariant), ModelTokenUsageSummary)>,
}

impl SnapshotCursor {
    pub(super) fn capture(summary: &HistoryUsageSummary, calendar: &UsageWindow) -> Self {
        Self {
            day: calendar.end.clone(),
            session_count: summary.session_count,
            days: summary
                .daily_usage
                .iter()
                .filter(|(day, _)| calendar.contains(day))
                .map(|(day, usage)| {
                    (
                        day.clone(),
                        SnapshotDay {
                            totals: ModelTokenUsageSummary {
                                prompt_tokens: usage.prompt_tokens,
                                cached_input_tokens: usage.cached_input_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: usage.total_tokens,
                                estimated_prompt_tokens: usage.estimated_prompt_tokens,
                                estimated_completion_tokens: usage.estimated_completion_tokens,
                                request_count: usage.request_count,
                                token_unavailable_requests: usage.token_unavailable_requests,
                            },
                            explicit_records: usage.explicit_records,
                            estimated_records: usage.estimated_records,
                            message_count: usage.message_count,
                            models: usage
                                .model_variants
                                .iter()
                                .map(|(key, value)| (key.clone(), *value))
                                .collect(),
                        },
                    )
                })
                .collect(),
        }
    }

    /// A decrease is a rewrite, not evidence of negative or new consumption.
    /// A model-only redistribution retains only the known day-level increment.
    pub(super) fn delta(
        &self,
        previous: &Self,
        calendar: &UsageWindow,
    ) -> (HistoryUsageSummary, bool) {
        let mut summary = HistoryUsageSummary::default();
        let mut gap = previous
            .days
            .keys()
            .any(|day| calendar.contains(day) && !self.days.contains_key(day));
        for (day, current) in &self.days {
            let empty = SnapshotDay::default();
            let old = previous.days.get(day).unwrap_or(&empty);
            if !at_least(current.totals, old.totals)
                || current.explicit_records < old.explicit_records
                || current.estimated_records < old.estimated_records
                || current.message_count < old.message_count
            {
                gap = true;
                continue;
            }
            let increment = current.totals.saturating_sub(old.totals);
            let mut daily = DailyUsageSummary {
                prompt_tokens: increment.prompt_tokens,
                cached_input_tokens: increment.cached_input_tokens,
                completion_tokens: increment.completion_tokens,
                total_tokens: increment.total_tokens,
                estimated_prompt_tokens: increment.estimated_prompt_tokens,
                estimated_completion_tokens: increment.estimated_completion_tokens,
                request_count: increment.request_count,
                token_unavailable_requests: increment.token_unavailable_requests,
                explicit_records: current.explicit_records - old.explicit_records,
                estimated_records: current.estimated_records - old.estimated_records,
                message_count: current.message_count - old.message_count,
                ..Default::default()
            };
            let old_models = old
                .models
                .iter()
                .map(|(key, usage)| (key, *usage))
                .collect::<BTreeMap<_, _>>();
            let current_models = current
                .models
                .iter()
                .map(|(key, usage)| (key, *usage))
                .collect::<BTreeMap<_, _>>();
            let redistributed = old_models.iter().any(|(key, value)| {
                !at_least(current_models.get(key).copied().unwrap_or_default(), *value)
            });
            if redistributed {
                gap = true;
                daily.add_model_variant_totals(
                    UNATTRIBUTED_MODEL.to_owned(),
                    UsageVariant::default(),
                    increment,
                );
            } else {
                for (key, usage) in current_models {
                    let added =
                        usage.saturating_sub(old_models.get(key).copied().unwrap_or_default());
                    if added.has_usage() {
                        daily.add_model_variant_totals(key.0.clone(), key.1.clone(), added);
                    }
                }
            }
            let modeled = daily.model_variants.values().fold(
                ModelTokenUsageSummary::default(),
                |mut total, usage| {
                    total.merge(*usage);
                    total
                },
            );
            if modeled != increment {
                // An inconsistent snapshot cannot establish per-model deltas.
                // Preserve the known source increment without guessing a model.
                gap = true;
                daily.model_usage.clear();
                daily.model_variants.clear();
                daily.add_model_variant_totals(
                    UNATTRIBUTED_MODEL.to_owned(),
                    UsageVariant::default(),
                    increment,
                );
            }
            if increment.has_usage() || daily.message_count > 0 {
                summary.explicit_prompt_tokens +=
                    daily.prompt_tokens - daily.estimated_prompt_tokens;
                summary.explicit_cached_input_tokens += daily.cached_input_tokens;
                summary.explicit_completion_tokens +=
                    daily.completion_tokens - daily.estimated_completion_tokens;
                summary.explicit_total_tokens += daily.total_tokens
                    - daily.estimated_prompt_tokens
                    - daily.estimated_completion_tokens;
                summary.estimated_prompt_tokens += daily.estimated_prompt_tokens;
                summary.estimated_completion_tokens += daily.estimated_completion_tokens;
                summary.estimated_total_tokens +=
                    daily.estimated_prompt_tokens + daily.estimated_completion_tokens;
                summary.explicit_records += daily.explicit_records;
                summary.estimated_records += daily.estimated_records;
                summary.message_count += daily.message_count;
                summary.token_unavailable_records += daily.token_unavailable_requests;
                summary.daily_usage.insert(day.clone(), daily);
            }
        }
        let previous_sessions = if self.day == previous.day {
            previous.session_count
        } else {
            0
        };
        if self.session_count < previous_sessions {
            gap = true;
        } else {
            summary.session_count = self.session_count - previous_sessions;
        }
        (summary, gap)
    }
}

fn at_least(current: ModelTokenUsageSummary, previous: ModelTokenUsageSummary) -> bool {
    let Some(current_estimated_total) = current
        .estimated_prompt_tokens
        .checked_add(current.estimated_completion_tokens)
    else {
        return false;
    };
    let Some(previous_estimated_total) = previous
        .estimated_prompt_tokens
        .checked_add(previous.estimated_completion_tokens)
    else {
        return false;
    };
    current.estimated_prompt_tokens <= current.prompt_tokens
        && current.estimated_completion_tokens <= current.completion_tokens
        && previous.estimated_prompt_tokens <= previous.prompt_tokens
        && previous.estimated_completion_tokens <= previous.completion_tokens
        && current_estimated_total <= current.total_tokens
        && previous_estimated_total <= previous.total_tokens
        && current.total_tokens - current_estimated_total
            >= previous.total_tokens - previous_estimated_total
        && current.cached_input_tokens <= current.prompt_tokens
        && previous.cached_input_tokens <= previous.prompt_tokens
        && current.prompt_tokens - current.cached_input_tokens
            >= previous.prompt_tokens - previous.cached_input_tokens
        && current.prompt_tokens - current.estimated_prompt_tokens
            >= previous.prompt_tokens - previous.estimated_prompt_tokens
        && current.completion_tokens - current.estimated_completion_tokens
            >= previous.completion_tokens - previous.estimated_completion_tokens
        && current.prompt_tokens >= previous.prompt_tokens
        && current.cached_input_tokens >= previous.cached_input_tokens
        && current.completion_tokens >= previous.completion_tokens
        && current.total_tokens >= previous.total_tokens
        && current.estimated_prompt_tokens >= previous.estimated_prompt_tokens
        && current.estimated_completion_tokens >= previous.estimated_completion_tokens
        && current.request_count >= previous.request_count
        && current.token_unavailable_requests >= previous.token_unavailable_requests
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_statistics::contract::{MessageUsage, UsageAccuracy};
    use serde_json::json;

    fn history(prompt: u64, effort: &str, estimated: bool) -> HistoryUsageSummary {
        let mut summary = HistoryUsageSummary::default();
        summary.add(
            MessageUsage {
                prompt_tokens: prompt,
                completion_tokens: 2,
                total_tokens: prompt + 2,
                model: Some("raw-model".to_owned()),
                variant: UsageVariant {
                    effort: Some(effort.to_owned()),
                    fast: Some(false),
                },
                accuracy: if estimated {
                    UsageAccuracy::Estimated
                } else {
                    UsageAccuracy::Exact
                },
                ..Default::default()
            },
            Some("2026-07-15".to_owned()),
        );
        summary.session_count = 1;
        summary
    }

    #[test]
    fn snapshot_cursor_preserves_estimates_requests_and_variant_deltas() {
        let calendar =
            UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}));
        let baseline = history(10, "high", true);
        let cursor = SnapshotCursor::capture(&baseline, &calendar);
        let decoded: SnapshotCursor =
            serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        assert_eq!(decoded, cursor);
        let (unchanged, gap) = cursor.delta(&decoded, &calendar);
        assert!(!gap);
        assert_eq!(unchanged.total_tokens(), 0);
        let mut next = baseline;
        next.merge(&history(5, "low", true));
        let next_cursor = SnapshotCursor::capture(&next, &calendar);
        let (delta, gap) = next_cursor.delta(&cursor, &calendar);
        assert!(!gap);
        assert_eq!(delta.total_tokens(), 7);
        assert_eq!(delta.estimated_total_tokens, 7);
        assert_eq!(delta.estimated_records, 1);
        assert_eq!(delta.daily_usage["2026-07-15"].request_count, 1);
        let models = &delta.daily_usage["2026-07-15"].model_variants;
        assert_eq!(models.len(), 1);
        assert_eq!(models.values().next().unwrap().estimated_prompt_tokens, 5);
        assert_eq!(models.values().next().unwrap().request_count, 1);
    }

    #[test]
    fn snapshot_redistribution_and_truncation_never_turn_positive_buckets_into_new_usage() {
        let calendar =
            UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}));
        let original = SnapshotCursor::capture(&history(100, "high", false), &calendar);
        let redistributed = SnapshotCursor::capture(&history(100, "low", false), &calendar);
        let (delta, gap) = redistributed.delta(&original, &calendar);
        assert!(gap);
        assert_eq!(delta.total_tokens(), 0);
        let grown = SnapshotCursor::capture(&history(110, "low", false), &calendar);
        let (delta, gap) = grown.delta(&original, &calendar);
        assert!(gap);
        assert_eq!(delta.total_tokens(), 10);
        let daily = &delta.daily_usage["2026-07-15"];
        assert_eq!(daily.model_usage[UNATTRIBUTED_MODEL].total_tokens, 10);
        assert_eq!(
            daily
                .model_variants
                .values()
                .map(|usage| usage.total_tokens)
                .sum::<u64>(),
            daily.total_tokens
        );
        let truncated = SnapshotCursor::capture(&history(50, "low", false), &calendar);
        let (delta, gap) = truncated.delta(&grown, &calendar);
        assert!(gap);
        assert_eq!(delta.total_tokens(), 0);
        let resumed = SnapshotCursor::capture(&history(60, "low", false), &calendar);
        let (delta, gap) = resumed.delta(&truncated, &calendar);
        assert!(!gap);
        assert_eq!(delta.total_tokens(), 10);
    }

    #[test]
    fn snapshot_reclassification_is_a_gap_even_when_total_counters_increase() {
        let calendar =
            UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}));
        let original = SnapshotCursor::capture(&history(100, "high", false), &calendar);
        let mut changed = history(90, "high", false);
        changed.merge(&history(20, "high", true));
        let current = SnapshotCursor::capture(&changed, &calendar);
        let (delta, gap) = current.delta(&original, &calendar);
        assert!(gap);
        assert_eq!(delta.total_tokens(), 0);
        changed.merge(&history(5, "high", false));
        let next = SnapshotCursor::capture(&changed, &calendar);
        let (delta, gap) = next.delta(&current, &calendar);
        assert!(!gap);
        assert_eq!(delta.explicit_total_tokens, 7);
        assert_eq!(delta.estimated_total_tokens, 0);
    }
}
