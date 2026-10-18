use super::*;

/// The native catalog owns ordering and provider attribution. Renderers preserve
/// this sequence, including within each provider group.
pub(super) fn ordered_model_entries(
    port: &AgentTargetPort,
    target: &str,
    entries: BTreeMap<String, ModelCatalogEntry>,
) -> Vec<ModelCatalogEntry> {
    let mut rows = entries
        .into_values()
        .map(|mut entry| {
            if entry.provider_id.is_none() && entry.provider.is_none() {
                if let Some(provider) = inferred_model_provider(&entry.name) {
                    entry.provider_id = Some(provider.clone());
                    entry.provider = provider_label_from_provider_id(&provider);
                    entry.provider_inferred = true;
                }
            }
            entry
                .reasoning_efforts
                .sort_by_key(|effort| effort_rank(effort));
            let provider = entry
                .provider_id
                .as_deref()
                .or(entry.provider.as_deref())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mut version = model_version(&entry.name);
            if version.is_empty() {
                version = model_version(&entry.display_name);
            }
            let intelligence =
                (port.agent_model_max_intelligence)(target, &entry.name).unwrap_or_default();
            (entry, provider, version, intelligence)
        })
        .collect::<Vec<_>>();
    rows.sort_by(
        |(left, left_provider, left_version, left_score),
         (right, right_provider, right_version, right_score)| {
            // Ephemeral rows are live availability signals (for example
            // Codex's quota reserve model). Keep them at the top of the
            // picker while they are present so a bounded popup cannot hide
            // the only model the user can currently use.
            right
                .ephemeral
                .cmp(&left.ephemeral)
                .then_with(|| {
                    official_provider_rank(target, left_provider)
                        .cmp(&official_provider_rank(target, right_provider))
                })
                .then_with(|| left_provider.cmp(right_provider))
                .then_with(|| right_version.cmp(left_version))
                .then_with(|| right_score.cmp(left_score))
                .then_with(|| model_tier(&right.name).cmp(&model_tier(&left.name)))
                .then_with(|| left.name.cmp(&right.name))
        },
    );
    rows.into_iter().map(|(entry, _, _, _)| entry).collect()
}

pub(super) fn inferred_model_provider(model: &str) -> Option<String> {
    let lower = model.trim().to_ascii_lowercase();
    if let Some((provider, _)) = lower.split_once('/') {
        return sanitize_option_name(provider);
    }
    let name = lower.split(['-', ' ', '[']).next().unwrap_or_default();
    Some(
        match name {
            "gpt" | "chatgpt" | "o1" | "o3" | "o4" => "openai",
            "claude" | "fable" | "opus" | "sonnet" | "haiku" | "opusplan" => "anthropic",
            "gemini" | "gemma" => "google",
            "kimi" | "moonshot" | "k2" | "k3" => "moonshot",
            "deepseek" => "deepseek",
            "grok" => "xai",
            "qwen" => "alibaba",
            "glm" => "zai",
            "llama" => "meta",
            "mistral" | "devstral" => "mistral",
            _ => return None,
        }
        .to_string(),
    )
}

fn official_provider_rank(target: &str, provider: &str) -> u8 {
    let official = match target {
        "codex" => &["openai", "chatgpt"][..],
        "claude-code" => &["anthropic", "claude"][..],
        "kimi-code" => &["kimi-code", "moonshot", "kimi"][..],
        "deepseek-harness" => &["deepseek", "deepseek-official"][..],
        "antigravity" => {
            return match provider {
                "google" | "gemini" => 0,
                "anthropic" | "claude" => 1,
                "openai" => 2,
                _ => 3,
            };
        }
        _ => &[][..],
    };
    if official.contains(&provider) { 0 } else { 3 }
}

fn model_version(model: &str) -> Vec<u32> {
    // Read only the first numeric run: context windows and effort suffixes do
    // not raise a model's version. Both 4.8 and native 4-8 IDs are admitted.
    let model = model.split(['[', '(']).next().unwrap_or(model);
    if model.to_ascii_lowercase().starts_with("gpt-oss") {
        return Vec::new();
    }
    let tail = model.rsplit('/').next().unwrap_or(model);
    let Some(start) = tail.find(|ch: char| ch.is_ascii_digit()) else {
        return Vec::new();
    };
    tail[start..]
        .split(['.', '-', ' '])
        .take_while(|part| !part.is_empty())
        .map_while(|part| part.parse().ok())
        .collect()
}

fn model_tier(model: &str) -> u8 {
    let lower = model.to_ascii_lowercase();
    for (name, rank) in [
        ("fable", 9),
        ("astra", 9),
        ("opus", 8),
        ("sol", 8),
        ("sonnet", 7),
        ("terra", 7),
        ("luna", 6),
        ("haiku", 5),
    ] {
        if lower
            .split(['-', ' ', '[', '/'])
            .any(|part| part == name || (name == "opus" && part == "opusplan"))
        {
            return rank;
        }
    }
    0
}

fn effort_rank(effort: &str) -> u8 {
    match effort.trim().to_ascii_lowercase().as_str() {
        "none" | "disabled" | "off" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" | "enabled" => 4,
        "xhigh" | "extra_high" | "extra high" => 5,
        "max" => 6,
        "ultra" => 7,
        _ => 8,
    }
}
