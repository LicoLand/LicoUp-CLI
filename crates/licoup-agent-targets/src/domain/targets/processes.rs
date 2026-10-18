use super::catalog::TargetDef;
use serde_json::Value;
use std::collections::BTreeSet;
#[cfg(windows)]
use std::process::Command;

#[derive(Clone, Debug)]
pub(super) struct ScanContext {
    running_processes: Option<BTreeSet<String>>,
    pub(super) running_processes_injected: bool,
}

impl ScanContext {
    pub(super) fn from_params(params: &Value) -> Self {
        if let Some(running_processes) = running_process_names_from_params(params) {
            return Self {
                running_processes: Some(running_processes),
                running_processes_injected: true,
            };
        }
        Self {
            running_processes: None,
            running_processes_injected: false,
        }
    }

    /// Capture host process state once, then clone the owned snapshot into
    /// independent target probes. This avoids repeated platform enumeration
    /// without sharing mutable scan state between workers.
    pub(super) fn snapshot_from_params(params: &Value) -> Self {
        let mut context = Self::from_params(params);
        if context.running_processes.is_none() {
            context.running_processes = Some(current_running_process_names());
        }
        context
    }

    fn running_processes(&mut self) -> &BTreeSet<String> {
        self.running_processes
            .get_or_insert_with(current_running_process_names)
    }
}

pub(super) fn target_uses_running_process_detection(target: &str) -> bool {
    matches!(
        target,
        "claude-code"
            | "codex"
            | "code"
            | "cursor"
            | "kilo-code"
            | "kimi-code"
            | "grok"
            | "command-code"
            | "pi"
            | "deepseek-harness"
            | "lico-agent"
            | "codebuddy"
            | "trae-agent"
    )
}

pub(super) fn running_process_for(
    def: &TargetDef,
    scan_context: &mut ScanContext,
) -> Option<String> {
    let running_processes = scan_context.running_processes();
    for name in def.process_names {
        let normalized = normalize_process_name(name);
        if running_processes.contains(&normalized) {
            return Some((*name).to_string());
        }
    }
    None
}

fn running_process_names_from_params(params: &Value) -> Option<BTreeSet<String>> {
    let value = params.get("runningProcessNames")?;
    let mut names = BTreeSet::<String>::new();
    match value {
        Value::Array(items) => {
            for item in items.iter().filter_map(Value::as_str) {
                insert_process_name(&mut names, item);
            }
        }
        Value::String(value) => {
            for item in value.split(',') {
                insert_process_name(&mut names, item);
            }
        }
        _ => {}
    }
    Some(names)
}

#[cfg(windows)]
fn current_running_process_names() -> BTreeSet<String> {
    let Ok(output) = Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .output()
    else {
        return BTreeSet::new();
    };
    if !output.status.success() {
        return BTreeSet::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    tasklist_process_names(&text)
}

#[cfg(not(windows))]
fn current_running_process_names() -> BTreeSet<String> {
    BTreeSet::new()
}

#[cfg(windows)]
fn tasklist_process_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::<String>::new();
    for line in text.lines() {
        if let Some(name) = first_csv_field(line) {
            insert_process_name(&mut names, &name);
        }
    }
    names
}

#[cfg(windows)]
fn first_csv_field(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    value.push('"');
                    chars.next();
                    continue;
                }
                return Some(value);
            }
            value.push(ch);
        }
        return None;
    }
    trimmed.split(',').next().map(str::trim).map(str::to_string)
}

fn insert_process_name(names: &mut BTreeSet<String>, value: &str) {
    let normalized = normalize_process_name(value);
    if normalized.is_empty() {
        return;
    }
    names.insert(normalized.clone());
    if let Some(stem) = normalized.strip_suffix(".exe") {
        names.insert(stem.to_string());
    }
}

fn normalize_process_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn injected_processes_are_normalized_and_do_not_require_host_scanning() {
        let mut context = ScanContext::from_params(&json!({
            "runningProcessNames": [" KIMI.EXE ", "code"]
        }));
        assert!(context.running_processes_injected);
        let names = context.running_processes();
        assert!(names.contains("kimi.exe"));
        assert!(names.contains("kimi"));
        assert!(names.contains("code"));
    }

    #[test]
    fn cloned_process_snapshots_have_independent_owned_state() {
        let original = ScanContext::snapshot_from_params(&json!({
            "runningProcessNames": ["codex"]
        }));
        let mut left = original.clone();
        let right = original;

        left.running_processes
            .as_mut()
            .unwrap()
            .insert("left-only".to_string());

        assert!(left.running_processes().contains("left-only"));
        assert!(!right.running_processes.unwrap().contains("left-only"));
    }
}
