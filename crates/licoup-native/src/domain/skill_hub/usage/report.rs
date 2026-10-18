use super::ledger::COLLECTION;
use crate::domain::skill_hub::{
    ClientStateStore, Result, Value, json, sanitize_skill_id, string_param,
};
use anyhow::{anyhow, ensure};
use std::collections::BTreeMap;
use time::{Date, Duration, OffsetDateTime};

const DEFAULT_REPORT_DAYS: i64 = 30;
const MAX_REPORT_DAYS: i64 = 365;

pub(super) fn report(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let (from, to) = report_window(params)?;
    let selected_days = (to - from).whole_days() + 1;
    let agent_filter = string_param(params, &["agent", "agentId"], usize::MAX);
    let skill_filter = string_param(params, &["skill", "skillId"], usize::MAX)
        .map(|value| sanitize_skill_id(&value))
        .transpose()?;
    let document = store.read_collection(COLLECTION)?;
    let mut total = 0_u64;
    let mut by_agent = BTreeMap::<String, u64>::new();
    let mut by_skill = BTreeMap::<String, u64>::new();
    let mut by_day = BTreeMap::<String, u64>::new();
    let mut all_time_total = 0_u64;
    let mut totals_by_skill = BTreeMap::<String, u64>::new();
    for item in document
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some((agent_id, skill_id, day_text, count)) = report_item(item) else {
            continue;
        };
        if agent_filter
            .as_deref()
            .is_some_and(|value| value != agent_id)
            || skill_filter
                .as_deref()
                .is_some_and(|value| value != skill_id)
        {
            continue;
        }
        // All-time totals ignore the window but honor the same filters.
        all_time_total = all_time_total
            .checked_add(count)
            .ok_or_else(|| anyhow!("skill usage report counter overflow"))?;
        add_count(&mut totals_by_skill, skill_id, count)?;
        let Ok(day) = parse_date(day_text) else {
            continue;
        };
        if day < from || day > to {
            continue;
        }
        total = total
            .checked_add(count)
            .ok_or_else(|| anyhow!("skill usage report counter overflow"))?;
        add_count(&mut by_agent, agent_id, count)?;
        add_count(&mut by_skill, skill_id, count)?;
        add_count(&mut by_day, day_text, count)?;
    }
    Ok(json!({
        "ok": true,
        "mode": "local-skill-usage",
        "window": {
            "from": format_date(from)?,
            "to": format_date(to)?,
            "selectedDays": selected_days,
            "defaultDays": DEFAULT_REPORT_DAYS,
            "maxDays": MAX_REPORT_DAYS
        },
        "filters": {"agentId": agent_filter, "skillId": skill_filter},
        "totalInvocations": total,
        "byAgent": counts_as_items("agentId", by_agent),
        "bySkill": counts_as_items("skillId", by_skill),
        "byDay": counts_as_items("date", by_day),
        "allTimeInvocations": all_time_total,
        "totalsBySkill": counts_as_items("skillId", totals_by_skill),
        "collectionSource": "runtime-skill-invocation-events+history-backfill-scan",
        "privacy": "aggregate-only"
    }))
}

fn report_item(item: &Value) -> Option<(&str, &str, &str, u64)> {
    (item.get("kind").and_then(Value::as_str) == Some("skill-usage-day"))
        .then(|| {
            Some((
                item.get("agentId")?.as_str()?,
                item.get("skillId")?.as_str()?,
                item.get("date")?.as_str()?,
                item.get("count")?.as_u64()?,
            ))
        })
        .flatten()
}

fn report_window(params: &Value) -> Result<(Date, Date)> {
    let today = OffsetDateTime::now_utc().date();
    let to = string_param(params, &["to", "endDate"], usize::MAX)
        .map(|value| parse_date(&value))
        .transpose()?
        .unwrap_or(today);
    let explicit_from = string_param(params, &["from", "startDate"], usize::MAX)
        .map(|value| parse_date(&value))
        .transpose()?;
    let selected_days = report_days(params)?;
    ensure!(
        explicit_from.is_none() || selected_days.is_none(),
        "skill usage report accepts either a start date or a day count"
    );
    let days = selected_days.unwrap_or(DEFAULT_REPORT_DAYS);
    let from = explicit_from.unwrap_or(to - Duration::days(days - 1));
    ensure!(
        from <= to,
        "skill usage report start date is after end date"
    );
    ensure!(
        (to - from).whole_days() + 1 <= MAX_REPORT_DAYS,
        "skill usage report window exceeds the bounded limit"
    );
    Ok((from, to))
}

fn report_days(params: &Value) -> Result<Option<i64>> {
    let Some(value) = params.get("days") else {
        return Ok(None);
    };
    let days = value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
        .ok_or_else(|| anyhow!("skill usage report days must be an integer"))?;
    ensure!(
        (1..=MAX_REPORT_DAYS).contains(&days),
        "skill usage report days must be between 1 and 365"
    );
    Ok(Some(days))
}

fn add_count(map: &mut BTreeMap<String, u64>, key: &str, count: u64) -> Result<()> {
    let entry = map.entry(key.to_string()).or_default();
    *entry = entry
        .checked_add(count)
        .ok_or_else(|| anyhow!("skill usage aggregate counter overflow"))?;
    Ok(())
}

fn counts_as_items(key_name: &str, counts: BTreeMap<String, u64>) -> Vec<Value> {
    counts
        .into_iter()
        .map(|(key, count)| json!({(key_name): key, "count": count}))
        .collect()
}

fn parse_date(value: &str) -> Result<Date> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Date::parse(value.trim(), &format).map_err(|_| anyhow!("date must use YYYY-MM-DD"))
}

fn format_date(value: Date) -> Result<String> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Ok(value.format(&format)?)
}
