use super::*;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinCatalog {
    #[serde(default)]
    agents: BTreeMap<String, BuiltinAgentRows>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinAgentRows {
    #[serde(default)]
    models: Vec<BuiltinModelRow>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuiltinModelRow {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    reasoning_efforts: Vec<String>,
}

fn builtin_catalog() -> &'static BuiltinCatalog {
    static CATALOG: std::sync::OnceLock<BuiltinCatalog> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("builtin_catalog.json")).unwrap_or(BuiltinCatalog {
            agents: BTreeMap::new(),
        })
    })
}

impl BuiltinModelRow {
    fn matches(&self, model_name: &str) -> bool {
        let normalized = model_name.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return false;
        }
        let tail = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
        self.name.trim().eq_ignore_ascii_case(&normalized)
            || self.name.trim().eq_ignore_ascii_case(tail)
            || self
                .aliases
                .iter()
                .any(|alias| alias.trim().eq_ignore_ascii_case(&normalized))
    }
}

/// Overrides scanned reasoning efforts with the reviewed built-in table for
/// known models. Models missing from the table keep their scanned data and
/// table rows for models absent from the scan are never injected.
pub(super) fn apply_builtin_model_catalog_overlay(
    target: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    sources: &mut BTreeSet<String>,
) {
    let Some(agent_rows) = builtin_catalog().agents.get(target) else {
        return;
    };
    if agent_rows.models.is_empty() {
        return;
    }
    let mut applied = false;
    for entry in entries.values_mut() {
        let Some(row) = agent_rows
            .models
            .iter()
            .find(|row| row.matches(&entry.name))
        else {
            continue;
        };
        entry.reasoning_efforts = row.reasoning_efforts.clone();
        entry.sources.insert("builtin".to_string());
        applied = true;
    }
    if applied {
        sources.insert("builtin".to_string());
    }
}

/// A pinned native alias inherits only the efforts of its admitted model.
pub(super) fn builtin_reasoning_efforts(target: &str, model: &str) -> Vec<String> {
    builtin_catalog()
        .agents
        .get(target)
        .and_then(|agent| {
            agent
                .models
                .iter()
                .find(|row| row.matches(model.trim_end_matches("[1m]")))
        })
        .map(|row| row.reasoning_efforts.clone())
        .unwrap_or_default()
}

pub const BUILTIN_FALLBACK_SOURCE: &str = "builtin-fallback";

/// Cold-start model list when no live scan and no persisted archive exist.
/// Never used once a scan archive is present.
pub fn builtin_cold_start_catalog(port: &AgentTargetPort, target: &str) -> Option<Value> {
    let agent_rows = builtin_catalog().agents.get(target)?;
    if agent_rows.models.is_empty() {
        return None;
    }
    let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
    for row in &agent_rows.models {
        add_model_catalog_entry(
            &mut entries,
            &row.name,
            BUILTIN_FALLBACK_SOURCE,
            row.reasoning_efforts
                .iter()
                .filter(|effort| !effort.trim().is_empty())
                .cloned()
                .collect(),
        );
    }
    if entries.is_empty() {
        return None;
    }
    Some(build_model_catalog(
        port,
        target,
        entries,
        BTreeSet::from([BUILTIN_FALLBACK_SOURCE.to_string()]),
        Vec::new(),
        None,
    ))
}
