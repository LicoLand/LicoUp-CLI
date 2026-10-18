use serde_json::Value;

pub const LUNA_RESERVE_MODEL: &str = "gpt-reserve";
const LUNA_RESERVE_BANNER: &str = "luna_reserve";
const RESERVE_LIMIT_ID: &str = "base_model_inference";

pub fn is_luna_model(model: &str) -> bool {
    let model = model.trim();
    model.eq_ignore_ascii_case("gpt-5.6-luna") || model.eq_ignore_ascii_case("gpt-5-6-luna")
}

fn models_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim()) || (is_luna_model(left) && is_luna_model(right))
}

fn field_text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn reserve_limit_snapshot(result: &Value) -> Option<&Value> {
    let limits = result
        .get("rateLimitsByLimitId")
        .or_else(|| result.get("rate_limits_by_limit_id"))?
        .as_object()?;
    limits.iter().find_map(|(key, snapshot)| {
        let is_reserve = key.eq_ignore_ascii_case(RESERVE_LIMIT_ID)
            || key.eq_ignore_ascii_case(LUNA_RESERVE_MODEL)
            || field_text(snapshot, &["limitId", "limit_id"])
                .is_some_and(|value| value.eq_ignore_ascii_case(RESERVE_LIMIT_ID))
            || field_text(snapshot, &["limitName", "limit_name"])
                .is_some_and(|value| value.eq_ignore_ascii_case(LUNA_RESERVE_MODEL));
        is_reserve.then_some(snapshot)
    })
}

/// The account response is the authority for a reserve row in the model
/// catalog. A row is exposed only when ordinary usage is unavailable, the
/// backend advertises the Luna Reserve banner, and the reserve bucket exists.
/// This deliberately does not infer availability from a local percentage alone.
pub fn reserve_model_available(result: &Value) -> bool {
    if result
        .get("ordinaryUsageAllowed")
        .or_else(|| result.get("ordinary_usage_allowed"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return false;
    }

    let Some(banner) = result
        .get("rateLimitUpsell")
        .or_else(|| result.get("rate_limit_upsell"))
    else {
        return false;
    };
    if field_text(banner, &["banner_type", "bannerType"]) != Some(LUNA_RESERVE_BANNER) {
        return false;
    }
    reserve_limit_snapshot(result).is_some()
}

/// Resolve the native fallback model for a selected Luna turn. The stricter
/// model matching remains separate from the catalog visibility rule above.
pub fn authorized_luna_reserve_model(
    result: &Value,
    requested_model: Option<&str>,
) -> Option<String> {
    if result
        .get("ordinaryUsageAllowed")
        .or_else(|| result.get("ordinary_usage_allowed"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return None;
    }

    let banner = result
        .get("rateLimitUpsell")
        .or_else(|| result.get("rate_limit_upsell"))?;
    if field_text(banner, &["banner_type", "bannerType"]) != Some(LUNA_RESERVE_BANNER) {
        return None;
    }

    let reserve_snapshot = reserve_limit_snapshot(result)?;
    let normal_model = field_text(reserve_snapshot, &["normalModelSlug", "normal_model_slug"])
        .or_else(|| field_text(result, &["normalModelSlug", "normal_model_slug"]));
    let expected_model = normal_model.unwrap_or("gpt-5.6-luna");
    if !is_luna_model(expected_model) {
        return None;
    }
    if requested_model.is_none() && normal_model.is_none() {
        return None;
    }
    if let Some(requested_model) = requested_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        && !models_match(requested_model, expected_model)
    {
        return None;
    }

    let blocked_model = field_text(banner, &["blocked_model_slug", "blockedModelSlug"]);
    if let Some(blocked_model) = blocked_model
        && !models_match(blocked_model, expected_model)
    {
        return None;
    }

    Some(LUNA_RESERVE_MODEL.to_owned())
}
