use super::parameters::text_param;
use super::paths::{expand_home, expand_home_from, home_dir};
use licoup_foundation::platform::paths::portable_data_dir;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

pub(crate) const COPILOT_CHAT_SESSIONS_KEY: &str = "github.copilot-chat.chatsessions";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryAdapter {
    Antigravity,
    ClaudeCode,
    Code,
    Codex,
    Copilot,
    Cursor,
    DeepSeekHarness,
    Hermes,
    KiloCode,
    KimiCode,
    OpenClaw,
    OpenCode,
    Pi,
    LicoAgent,
    WorkBuddy,
    CodeBuddy,
    TraeWork,
    TraeAgent,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryRoot {
    pub(crate) path: PathBuf,
    pub(crate) source_kind: String,
    pub(crate) explicitly_selected: bool,
}

impl HistoryAdapter {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::ClaudeCode => "claude-code",
            Self::Code => "code",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::DeepSeekHarness => "deepseek-harness",
            Self::Hermes => "hermes",
            Self::KiloCode => "kilo-code",
            Self::KimiCode => "kimi-code",
            Self::OpenClaw => "openclaw",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::LicoAgent => "lico-agent",
            Self::WorkBuddy => "workbuddy",
            Self::CodeBuddy => "codebuddy",
            Self::TraeWork => "trae-work",
            Self::TraeAgent => "trae-agent",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity IDE",
            Self::ClaudeCode => "Claude Code CLI",
            Self::Code => "Visual Studio Code IDE",
            Self::Codex => "Codex CLI",
            Self::Copilot => "GitHub Copilot Plugin",
            Self::Cursor => "Cursor CLI",
            Self::DeepSeekHarness => "DeepSeek Harness CLI",
            Self::Hermes => "Hermes Agent CLI",
            Self::KiloCode => "Kilo Code CLI",
            Self::KimiCode => "Kimi Code CLI",
            Self::OpenClaw => "OpenClaw CLI",
            Self::OpenCode => "OpenCode CLI",
            Self::Pi => "Pi Agent CLI",
            Self::LicoAgent => "Lico Agent CLI",
            Self::WorkBuddy => "WorkBuddy Desktop",
            Self::CodeBuddy => "CodeBuddy CLI",
            Self::TraeWork => "Trae Work Desktop",
            Self::TraeAgent => "Trae Agent CLI",
        }
    }

    pub(crate) fn accepts_file(self, path: &Path, extension: &str) -> bool {
        if self == Self::KimiCode {
            return extension == "jsonl"
                && path.file_name().and_then(|value| value.to_str()) == Some("wire.jsonl")
                && path
                    .components()
                    .any(|component| component.as_os_str() == "agents");
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .map(|name| name.ends_with(".backup") || name == "codebase-external.sqlite")
            .unwrap_or(false)
        {
            return false;
        }
        match self {
            Self::DeepSeekHarness => deepseek_generation(path).is_some(),
            Self::Codex => matches!(extension, "jsonl" | "ndjson" | "json" | "md"),
            Self::ClaudeCode => matches!(extension, "jsonl" | "json" | "md" | "txt"),
            Self::Code => matches!(
                extension,
                "jsonl"
                    | "ndjson"
                    | "json"
                    | "md"
                    | "txt"
                    | "log"
                    | "sqlite"
                    | "sqlite3"
                    | "db"
                    | "vscdb"
            ),
            Self::Antigravity => matches!(
                extension,
                "jsonl"
                    | "ndjson"
                    | "json"
                    | "md"
                    | "txt"
                    | "log"
                    | "sqlite"
                    | "sqlite3"
                    | "db"
                    | "vscdb"
            ),
            Self::Cursor => matches!(
                extension,
                "jsonl" | "ndjson" | "json" | "sqlite" | "sqlite3" | "db" | "vscdb"
            ),
            Self::Copilot => matches!(
                extension,
                "jsonl" | "ndjson" | "json" | "sqlite" | "sqlite3" | "db" | "vscdb"
            ),
            Self::LicoAgent => matches!(extension, "jsonl" | "ndjson" | "json"),
            Self::WorkBuddy => matches!(extension, "sqlite" | "sqlite3" | "db"),
            Self::CodeBuddy => matches!(extension, "jsonl" | "json" | "md" | "txt" | "log"),
            Self::TraeWork => matches!(extension, "json"),
            Self::TraeAgent => matches!(extension, "json"),
            Self::KiloCode
            | Self::OpenCode
            | Self::OpenClaw
            | Self::Hermes
            | Self::KimiCode
            | Self::Pi => matches!(
                extension,
                "jsonl" | "ndjson" | "json" | "md" | "txt" | "log" | "sqlite" | "sqlite3" | "db"
            ),
        }
    }

    pub(crate) fn sqlite_table_may_hold_history(self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("sqlite") || lower.contains("fts") || lower.contains("embedding") {
            return false;
        }
        if self == Self::KiloCode
            && (lower.contains("account") || lower.contains("control_account"))
        {
            return false;
        }
        match self {
            Self::Code => {
                lower == "itemtable"
                    || lower.contains("chat")
                    || lower.contains("conversation")
                    || lower.contains("session")
                    || lower.contains("history")
                    || lower.contains("workspace")
                    || lower.contains("state")
            }
            Self::Cursor | Self::Copilot | Self::Antigravity => {
                lower == "itemtable"
                    || lower == "cursordiskkv"
                    || lower.contains("chat")
                    || lower.contains("conversation")
                    || lower.contains("session")
                    || lower.contains("history")
            }
            _ => {
                lower.contains("chat")
                    || lower.contains("conversation")
                    || lower.contains("session")
                    || lower.contains("history")
                    || lower == "itemtable"
            }
        }
    }

    pub(crate) fn sqlite_row_may_hold_history(
        self,
        table: &str,
        key: Option<&str>,
        row_text: &str,
    ) -> bool {
        let key = key.unwrap_or_default().to_ascii_lowercase();
        match self {
            Self::Code => {
                key.contains("chat")
                    || key.contains("conversation")
                    || key.contains("session")
                    || key.contains("history")
                    || key.contains("workspace")
                    || key.contains("recent")
                    || looks_like_history_text(row_text)
            }
            Self::Copilot => {
                table.eq_ignore_ascii_case("ItemTable") && key == COPILOT_CHAT_SESSIONS_KEY
            }
            Self::Cursor => {
                key.contains("aichat")
                    || key.contains("composer")
                    || key.contains("chat")
                    || key.contains("conversation")
                    || key.starts_with("bubbleid:")
                    || key.starts_with("composerdata:")
                    || looks_like_history_text(row_text)
            }
            Self::KiloCode => {
                !table.to_ascii_lowercase().contains("account") && looks_like_history_text(row_text)
            }
            _ => looks_like_history_text(row_text),
        }
    }
}

pub(crate) fn adapter_for_agent(agent_id: &str) -> Option<HistoryAdapter> {
    match agent_id {
        "antigravity" => Some(HistoryAdapter::Antigravity),
        "claude" | "claude-code" => Some(HistoryAdapter::ClaudeCode),
        "code" | "vscode" | "vs-code" => Some(HistoryAdapter::Code),
        "codex" => Some(HistoryAdapter::Codex),
        "copilot" | "github-copilot" => Some(HistoryAdapter::Copilot),
        "cursor" => Some(HistoryAdapter::Cursor),
        "hermes" | "hermes-agent" => Some(HistoryAdapter::Hermes),
        "kilo" | "kilo-code" => Some(HistoryAdapter::KiloCode),
        "kimi-code" | "kimi_code" | "kimicode" => Some(HistoryAdapter::KimiCode),
        "openclaw" => Some(HistoryAdapter::OpenClaw),
        "opencode" => Some(HistoryAdapter::OpenCode),
        "pi" | "pi-agent" | "pi-coding-agent" => Some(HistoryAdapter::Pi),
        "lico-agent" | "lico" => Some(HistoryAdapter::LicoAgent),
        "workbuddy" => Some(HistoryAdapter::WorkBuddy),
        "codebuddy" | "workbuddy-cli" | "workbuddy_cli" => Some(HistoryAdapter::CodeBuddy),
        "trae-work" | "trae_work" | "traework" => Some(HistoryAdapter::TraeWork),
        "trae-agent" | "trae_agent" | "trae-cli" | "trae_cli" => Some(HistoryAdapter::TraeAgent),
        _ => None,
    }
}

/// Usage metadata can be readable even when transcript history is unavailable.
pub(crate) fn usage_adapter_for_agent(agent_id: &str) -> Option<HistoryAdapter> {
    match agent_id {
        "deepseek-harness" => Some(HistoryAdapter::DeepSeekHarness),
        _ => adapter_for_agent(agent_id),
    }
}

/// Canonical immutable Harness generations. Temporary/backup artifacts are not
/// histories, and only the highest generation in each session is authoritative.
pub(crate) fn deepseek_generation(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(".zstd").unwrap_or(name);
    if name == "session.jsonl" {
        return Some(0);
    }
    let version = name.strip_prefix("session.v")?.strip_suffix(".jsonl")?;
    if version.starts_with('0') || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    version
        .parse()
        .ok()
        .filter(|version| *version <= (1 << 53) - 1)
}

pub(crate) fn history_roots(adapter: HistoryAdapter, params: &Value) -> Vec<HistoryRoot> {
    if let Some(root) = text_param(params, &["root", "historyRoot"])
        && !root.is_empty()
    {
        return vec![HistoryRoot {
            path: expand_home(&root),
            source_kind: text_param(params, &["historyRootKind", "rootKind"])
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "override-root".to_string()),
            explicitly_selected: true,
        }];
    }
    if adapter == HistoryAdapter::LicoAgent {
        return lico_agent_history_roots(params);
    }
    let home_override = text_param(params, &["homeDir"])
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let home = home_override.clone().unwrap_or_else(home_dir);
    let host = home_override
        .as_ref()
        .map(|_| crate::domain::targets::scan_paths::HostRoots::from_home(&home))
        .unwrap_or_else(crate::domain::targets::scan_paths::HostRoots::from_environment);
    let mut roots = crate::domain::targets::scan_paths::history_roots(adapter.id(), &host)
        .into_iter()
        .map(|root| HistoryRoot {
            path: root.path,
            source_kind: root.kind,
            explicitly_selected: false,
        })
        .collect::<Vec<_>>();
    let allow_environment = home_override.is_none();
    match adapter {
        HistoryAdapter::DeepSeekHarness => {
            let dsh_home = text_param(params, &["deepseekHarnessHome", "dshHome"])
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    allow_environment
                        .then(|| env::var("DSH_HOME").ok())
                        .flatten()
                        .filter(|value| !value.trim().is_empty())
                })
                .map(|value| expand_home_from(&value, || home.to_path_buf()))
                .unwrap_or_else(|| home.join(".dsh"));
            for root in &mut roots {
                if root.source_kind == "deepseek-harness-session-store" {
                    root.path = dsh_home.join("sessions");
                }
            }
        }
        HistoryAdapter::KimiCode => {
            let kimi_home = kimi_code_history_home(params, &home, allow_environment);
            for root in &mut roots {
                if let Ok(relative) = root.path.strip_prefix(home.join(".kimi-code")) {
                    root.path = kimi_home.join(relative);
                } else if root.path == home.join(".kimi-code") {
                    root.path = kimi_home.clone();
                }
            }
        }
        HistoryAdapter::Copilot => {
            let copilot_home = copilot_history_home(params, &home, allow_environment);
            for root in &mut roots {
                if let Ok(relative) = root.path.strip_prefix(home.join(".copilot")) {
                    root.path = copilot_home.join(relative);
                }
            }
        }
        HistoryAdapter::Pi => {
            let session_dir = pi_history_session_dir(params, &home, allow_environment);
            for root in &mut roots {
                if root.source_kind == "pi-session-store" {
                    root.path = session_dir.clone();
                }
            }
        }
        _ => {}
    }
    roots
}

fn lico_agent_history_roots(params: &Value) -> Vec<HistoryRoot> {
    let explicitly_selected = text_param(params, &["licoAgentSessionDir", "licoAgentSessionsDir"])
        .is_some_and(|value| !value.trim().is_empty());
    lico_agent_session_dir(params)
        .map(|dir| {
            vec![HistoryRoot {
                path: dir,
                source_kind: "lico-agent-session-store".to_string(),
                explicitly_selected,
            }]
        })
        .unwrap_or_default()
}

fn lico_agent_session_dir(params: &Value) -> Option<PathBuf> {
    text_param(params, &["licoAgentSessionDir", "licoAgentSessionsDir"])
        .filter(|value| !value.trim().is_empty())
        .map(|value| expand_home(&value))
        .or_else(|| {
            portable_data_dir()
                .ok()
                .map(|portable| portable.join("client-state/lico-agent/sessions"))
        })
}

fn kimi_code_history_home(params: &Value, home: &Path, allow_environment: bool) -> PathBuf {
    let configured = text_param(params, &["kimiCodeHome"]).or_else(|| {
        allow_environment
            .then(|| env::var("KIMI_CODE_HOME").ok())
            .flatten()
    });
    configured
        .map(|value| expand_home_from(&value, || home.to_path_buf()))
        .unwrap_or_else(|| home.join(".kimi-code"))
}

fn copilot_history_home(params: &Value, home: &Path, allow_environment: bool) -> PathBuf {
    let configured = text_param(params, &["copilotHome"]).or_else(|| {
        allow_environment
            .then(|| env::var("COPILOT_HOME").ok())
            .flatten()
    });
    configured
        .map(|value| expand_home_from(&value, || home.to_path_buf()))
        .unwrap_or_else(|| home.join(".copilot"))
}

fn pi_history_session_dir(params: &Value, home: &Path, allow_environment: bool) -> PathBuf {
    let configured =
        text_param(params, &["piSessionDir", "piCodingAgentSessionDir"]).or_else(|| {
            allow_environment
                .then(|| env::var("PI_CODING_AGENT_SESSION_DIR").ok())
                .flatten()
        });
    configured
        .map(|value| expand_home_from(&value, || home.to_path_buf()))
        .unwrap_or_else(|| home.join(".pi/agent/sessions"))
}

fn looks_like_history_text(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("assistant")
        || lower.contains("user")
        || lower.contains("prompt")
        || lower.contains("message")
        || lower.contains("conversation")
        || lower.contains("chat")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aliases_and_root_overrides_resolve_in_the_catalog() {
        assert_eq!(adapter_for_agent("vscode"), Some(HistoryAdapter::Code));
        assert_eq!(
            adapter_for_agent("kimi_code"),
            Some(HistoryAdapter::KimiCode)
        );

        let history_root = PathBuf::from("test-data").join("history");
        let roots = history_roots(
            HistoryAdapter::Codex,
            &json!({"historyRoot": history_root, "rootKind": "fixture"}),
        );
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, history_root);
        assert_eq!(roots[0].source_kind, "fixture");
    }

    #[test]
    fn sqlite_catalog_rejects_indexes_and_account_tables() {
        assert!(!HistoryAdapter::Cursor.sqlite_table_may_hold_history("chat_fts"));
        assert!(!HistoryAdapter::KiloCode.sqlite_table_may_hold_history("account"));
        assert!(HistoryAdapter::Code.sqlite_table_may_hold_history("ItemTable"));
        assert!(HistoryAdapter::Copilot.accepts_file(Path::new("transcript.jsonl"), "jsonl"));
    }

    #[test]
    fn claude_configuration_is_not_a_history_root() {
        let home = PathBuf::from("synthetic-home");
        let roots = history_roots(
            HistoryAdapter::ClaudeCode,
            &json!({"homeDir": home.clone()}),
        );
        assert!(
            roots
                .iter()
                .all(|root| !root.path.ends_with(".claude.json"))
        );
        assert!(roots.iter().any(|root| {
            root.source_kind == "claude-project-transcripts"
                && root.path == home.join(".claude/projects")
        }));
    }

    #[test]
    fn copilot_sqlite_rows_require_the_native_conversation_key() {
        for key in ["storage", "redhat.optin", "github.copilot.telemetry"] {
            assert!(!HistoryAdapter::Copilot.sqlite_row_may_hold_history(
                "ItemTable",
                Some(key),
                &format!("key: {key}\nvalue: {{\"scope\":\"copilot\"}}"),
            ));
        }
        assert!(HistoryAdapter::Copilot.sqlite_row_may_hold_history(
            "ItemTable",
            Some("github.copilot-chat.chatSessions"),
            "key: github.copilot-chat.chatSessions\nvalue: {\"chatSessions\":[]}",
        ));
    }

    #[test]
    fn catalog_includes_cli_usage_stores_without_overlapping_pi_roots() {
        let home = PathBuf::from("synthetic-home");
        let params = json!({"homeDir": home});

        let antigravity = history_roots(HistoryAdapter::Antigravity, &params);
        assert!(antigravity.iter().any(|root| {
            root.source_kind == "antigravity-cli"
                && root.path == home.join(".gemini/antigravity-cli")
        }));

        let copilot = history_roots(HistoryAdapter::Copilot, &params);
        assert!(copilot.iter().any(|root| {
            root.source_kind == "copilot-cli-session-store"
                && root.path == home.join(".copilot/session-state")
        }));

        let pi = history_roots(HistoryAdapter::Pi, &params);
        assert_eq!(pi.len(), 1);
        assert_eq!(pi[0].path, home.join(".pi/agent/sessions"));

        let lico_override = json!({"licoAgentSessionDir": "/fixture-root/lico-agent-sessions"});
        let lico = history_roots(HistoryAdapter::LicoAgent, &lico_override);
        assert_eq!(lico.len(), 1);
        assert_eq!(
            lico[0].path,
            PathBuf::from("/fixture-root/lico-agent-sessions")
        );
        assert_eq!(lico[0].source_kind, "lico-agent-session-store");
        assert_eq!(
            adapter_for_agent("lico-agent"),
            Some(HistoryAdapter::LicoAgent)
        );
        assert_eq!(adapter_for_agent("lico"), Some(HistoryAdapter::LicoAgent));
    }

    #[test]
    fn workbuddy_and_codebuddy_resolve_independent_roots() {
        let home = PathBuf::from("synthetic-home");
        let params = json!({"homeDir": home});

        let workbuddy = history_roots(HistoryAdapter::WorkBuddy, &params);
        assert_eq!(workbuddy.len(), 1);
        assert_eq!(workbuddy[0].path, home.join(".workbuddy"));
        assert_eq!(workbuddy[0].source_kind, "workbuddy-app-state");
        assert!(HistoryAdapter::WorkBuddy.accepts_file(Path::new("workbuddy.db"), "db"));
        assert!(!HistoryAdapter::WorkBuddy.accepts_file(Path::new("history.jsonl"), "jsonl"));

        let codebuddy = history_roots(HistoryAdapter::CodeBuddy, &params);
        assert_eq!(
            codebuddy
                .iter()
                .find(|root| root.source_kind == "codebuddy-global-history")
                .unwrap()
                .path,
            home.join(".codebuddy/history.jsonl")
        );
        assert!(
            codebuddy
                .iter()
                .any(|root| root.path == home.join(".codebuddy/sessions"))
        );
        assert!(HistoryAdapter::CodeBuddy.accepts_file(Path::new("session.jsonl"), "jsonl"));
        assert_eq!(
            adapter_for_agent("workbuddy-cli"),
            Some(HistoryAdapter::CodeBuddy)
        );
    }

    #[test]
    fn trae_work_has_session_root_and_trae_agent_has_none() {
        let home = PathBuf::from("synthetic-home");
        let params = json!({"homeDir": home});

        let trae_work = history_roots(HistoryAdapter::TraeWork, &params);
        assert_eq!(trae_work.len(), 1);
        assert_eq!(trae_work[0].path, home.join(".trae/sessions"));
        assert_eq!(trae_work[0].source_kind, "trae-work-session-store");
        assert!(HistoryAdapter::TraeWork.accepts_file(Path::new("session.json"), "json"));
        assert!(!HistoryAdapter::TraeWork.accepts_file(Path::new("session.jsonl"), "jsonl"));

        // Trae Agent trajectories are working-directory relative; a static
        // home root would be invented. Override-root browsing still applies.
        assert!(history_roots(HistoryAdapter::TraeAgent, &params).is_empty());
        let override_root = PathBuf::from("trajectories");
        let override_roots = history_roots(
            HistoryAdapter::TraeAgent,
            &json!({"historyRoot": override_root, "rootKind": "fixture"}),
        );
        assert_eq!(override_roots.len(), 1);
        assert_eq!(override_roots[0].source_kind, "fixture");
        assert!(
            HistoryAdapter::TraeAgent
                .accepts_file(Path::new("trajectory_20260101_000000.json"), "json")
        );
        assert_eq!(
            adapter_for_agent("trae-cli"),
            Some(HistoryAdapter::TraeAgent)
        );
    }
}
