use super::*;
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use std::time::Duration;

// The account-scoped native catalog performs a network-backed entitlement
// lookup. Product policy: every model-catalog scan waits up to one minute.
const DEFAULT_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MIN_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 100;
const MAX_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MAX_CURSOR_CLI_MODEL_LOOKUP_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub(super) struct CursorCliCatalogResult {
    pub(super) authoritative: bool,
    pub(super) default_model: Option<String>,
}

pub(super) fn collect_cursor_cli_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> CursorCliCatalogResult {
    let source = "cursor-cli:models";
    if !agent_cli_model_lookup_enabled(params)
        || param_bool(params, "disableCursorCliModelLookup").unwrap_or(false)
    {
        diagnostics.push(json!({"source": source, "status": "disabled"}));
        return CursorCliCatalogResult::default();
    }

    let program = param_string(params, "cursorCliPath")
        .or_else(|| param_string(params, "cursorAgentPath"))
        .map(PathBuf::from)
        .or_else(|| find_binary(&["cursor-agent"]));
    let Some(program) = program else {
        diagnostics.push(json!({"source": source, "status": "binary-unavailable"}));
        return CursorCliCatalogResult::default();
    };
    let timeout_ms = param_u64(params, "cursorCliModelLookupTimeoutMs")
        .or_else(|| param_u64(params, "agentCliModelLookupTimeoutMs"))
        .unwrap_or(DEFAULT_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS)
        .clamp(
            MIN_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS,
            MAX_CURSOR_CLI_MODEL_LOOKUP_TIMEOUT_MS,
        );
    if !crate::domain::targets::scan_paths::discovered_agent_may_execute(&program, true) {
        diagnostics.push(json!({"source": source, "status": "execution-denied"}));
        return CursorCliCatalogResult::default();
    }
    let mut command = Command::new(program);
    command.arg("models");
    // Selected-agent catalog lookup is a user action. The child observes the
    // user shell environment (ADR 0007) so `cursor-agent models` reads the
    // same account catalog it does in the user's terminal.
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    let Ok(output) = run_bounded_command_output(
        &mut command,
        Duration::from_millis(timeout_ms),
        MAX_CURSOR_CLI_MODEL_LOOKUP_OUTPUT_BYTES,
    ) else {
        diagnostics.push(json!({"source": source, "status": "command-failed"}));
        return CursorCliCatalogResult::default();
    };
    if output.timed_out {
        diagnostics.push(json!({"source": source, "status": "timeout"}));
        return CursorCliCatalogResult::default();
    }
    if output.truncated {
        diagnostics.push(json!({"source": source, "status": "output-too-large"}));
        return CursorCliCatalogResult::default();
    }
    if !output.status.is_some_and(|status| status.success()) {
        diagnostics.push(json!({
            "source": source,
            "status": "command-exited",
            "code": output.status.and_then(|status| status.code()),
        }));
        return CursorCliCatalogResult::default();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut official_entries = BTreeMap::new();
    let parsed =
        collect_cursor_model_catalog_from_cli_output(&stdout, source, &mut official_entries);
    if parsed.added == 0 {
        diagnostics.push(json!({"source": source, "status": "empty"}));
        return CursorCliCatalogResult::default();
    }
    *entries = official_entries;
    CursorCliCatalogResult {
        authoritative: true,
        // The CLI's own `(default)` row is the product default (`auto`). A
        // `(current)` row only records the last explicit CLI override and must
        // not shadow it.
        default_model: parsed.default_model.or(parsed.current_model),
    }
}

#[derive(Debug, Default)]
pub(super) struct ParsedCursorCliCatalog {
    pub(super) added: usize,
    pub(super) default_model: Option<String>,
    pub(super) current_model: Option<String>,
}

pub(super) fn collect_cursor_model_catalog_from_cli_output(
    raw: &str,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) -> ParsedCursorCliCatalog {
    let before = entries.len();
    let mut default_model = None;
    let mut current_model = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "Available models" || trimmed.starts_with("Tip:") {
            continue;
        }
        let (model_id, display_name) = trimmed
            .split_once(" - ")
            .map(|(id, label)| (id.trim(), Some(label.trim())))
            .unwrap_or((trimmed, None));
        let is_default = display_name.is_some_and(|label| label.ends_with(" (default)"));
        let is_current = display_name.is_some_and(|label| label.ends_with(" (current)"));
        let display_name = display_name.map(|label| {
            label
                .strip_suffix(" (default)")
                .or_else(|| label.strip_suffix(" (current)"))
                .unwrap_or(label)
        });
        add_model_catalog_entry_with_provider(
            entries,
            model_id,
            display_name,
            None,
            None,
            source,
            BTreeSet::new(),
        );
        if is_default {
            default_model = Some(model_id.to_string());
        }
        if is_current {
            current_model = Some(model_id.to_string());
        }
    }
    ParsedCursorCliCatalog {
        added: entries.len().saturating_sub(before),
        default_model,
        current_model,
    }
}

pub(super) fn remove_cursor_independent_reasoning_efforts(
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    for entry in entries.values_mut() {
        entry.reasoning_efforts.clear();
    }
}
