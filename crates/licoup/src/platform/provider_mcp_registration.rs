//! Digest-bound mutation of one namespaced, LicoUp-owned user MCP entry.
//!
//! One approval also delivers the embedded usage Skill to the provider's user
//! Skill Hub root, and publishes a copy of it on the shared Skill surface.

use licoup_foundation::platform::{file_security, paths};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const SERVER_ID: &str = "land.lico.licoup.subagents";
const ENTRY_SCHEMA: &str = "licoup.subagent-mcp-registration.v2";
const MANAGED_BY: &str = "LicoUp";
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const SKILL_SOURCE: &str = crate::domain::client_conversation::LICOUP_GUIDE_SKILL_SOURCE;

fn cursor_context_environment() -> Value {
    json!({
        "LICOUP_HOME": "${env:LICOUP_HOME}",
        "LICOUP_MCP_CONVERSATION_ID": "${env:LICOUP_MCP_CONVERSATION_ID}",
        "LICOUP_MCP_MEMBERSHIP_ID": "${env:LICOUP_MCP_MEMBERSHIP_ID}",
        "LICOUP_MCP_PARENT_DISPATCH_ID": "${env:LICOUP_MCP_PARENT_DISPATCH_ID}",
    })
}

/// Gemini/Antigravity expand `$VAR` / `${VAR}` in the MCP `env` block and
/// redact undeclared variables. An absent `env` leaves the thin connector
/// without a portable root, so it exits before `initialize` and the IDE
/// reports EOF. Cursor `${env:VAR}` must not be used here: Antigravity would
/// pass that spelling through as a literal path.
fn antigravity_context_environment() -> Value {
    json!({
        "LICOUP_HOME": "${LICOUP_HOME}",
        "LICOUP_MCP_CONVERSATION_ID": "${LICOUP_MCP_CONVERSATION_ID}",
        "LICOUP_MCP_MEMBERSHIP_ID": "${LICOUP_MCP_MEMBERSHIP_ID}",
        "LICOUP_MCP_PARENT_DISPATCH_ID": "${LICOUP_MCP_PARENT_DISPATCH_ID}",
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderConfigKind {
    Cursor,
    Antigravity,
    ClaudeCode,
    Codex,
}

impl ProviderConfigKind {
    pub const fn provider_id(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Antigravity => "antigravity",
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidConnector,
    ConfigUnavailable,
    ConfigAmbiguous,
    ConfigPathUnsupported,
    OwnedEntryAmbiguous,
    DuplicateConnectorEntry,
    ApprovalRequired,
    ApprovalMismatch,
    ApprovalConsumed,
    ConfigChanged,
    WriteFailed,
}

#[derive(Clone, Debug)]
pub struct RegistrationPlan {
    kind: ProviderConfigKind,
    connector: PathBuf,
    config_path: PathBuf,
    shared_skill_path: PathBuf,
    config_digest: String,
    skill_path: PathBuf,
    skill_digest: String,
    skill_was_absent: bool,
    digest: String,
}

impl RegistrationPlan {
    pub fn prepare(kind: ProviderConfigKind, connector: &Path) -> Result<Self, RegistrationError> {
        let connector = canonical_connector(connector)?;
        let config_path = resolve_config_path(kind)?;
        Self::prepare_at(kind, connector, config_path)
    }

    /// Prepare against one explicitly discovered config path. The path must
    /// canonicalize to a reviewed candidate for this provider; the selected
    /// canonical path and the current config bytes are bound into the approval
    /// digest. Without this route the default both-present resolution stays
    /// fail-closed.
    pub fn prepare_with_config_path(
        kind: ProviderConfigKind,
        connector: &Path,
        config_path: &Path,
    ) -> Result<Self, RegistrationError> {
        let connector = canonical_connector(connector)?;
        let config_path = admit_explicit_config_path(kind, config_path)?;
        Self::prepare_at(kind, connector, config_path)
    }

    fn prepare_at(
        kind: ProviderConfigKind,
        connector: PathBuf,
        config_path: PathBuf,
    ) -> Result<Self, RegistrationError> {
        let skill_path = resolve_skill_path(kind)?;
        let shared_skill_path = shared_skill_path()?;
        let raw = read_optional_config(&config_path)?;
        let config = parse_config_for(kind, raw.as_deref())?;
        ensure_owned_entry_unambiguous_doc(&config, kind, &connector)?;
        let config_digest = digest_bytes(raw.as_deref().unwrap_or(b"<absent>"));
        let skill = read_optional_skill(&skill_path)?;
        if skill
            .as_deref()
            .is_some_and(|current| current != SKILL_SOURCE.as_bytes())
        {
            return Err(RegistrationError::OwnedEntryAmbiguous);
        }
        let skill_digest = digest_bytes(skill.as_deref().unwrap_or(b"<absent>"));
        let digest = plan_digest(
            kind,
            &connector,
            &config_path,
            &shared_skill_path,
            &config_digest,
            &skill_digest,
        )?;
        Ok(Self {
            kind,
            connector,
            config_path,
            shared_skill_path,
            config_digest,
            skill_path,
            skill_digest,
            skill_was_absent: skill.is_none(),
            digest,
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn approve(
        &self,
        confirmed: bool,
        digest: &str,
    ) -> Result<RegistrationPermit, RegistrationError> {
        if !confirmed {
            return Err(RegistrationError::ApprovalRequired);
        }
        if digest != self.digest {
            return Err(RegistrationError::ApprovalMismatch);
        }
        Ok(RegistrationPermit {
            digest: digest.to_owned(),
            consumed: false,
        })
    }
}

pub struct RegistrationPermit {
    digest: String,
    consumed: bool,
}

pub fn status(kind: ProviderConfigKind, connector: &Path) -> Result<bool, RegistrationError> {
    let connector = canonical_connector(connector)?;
    let path = resolve_config_path(kind)?;
    status_at(kind, &connector, &path)
}

/// Read-only readiness probe against one explicitly discovered config path,
/// admitted through the same reviewed-candidate rule as the mutating route.
pub fn status_with_config_path(
    kind: ProviderConfigKind,
    connector: &Path,
    config_path: &Path,
) -> Result<bool, RegistrationError> {
    let connector = canonical_connector(connector)?;
    let path = admit_explicit_config_path(kind, config_path)?;
    status_at(kind, &connector, &path)
}

fn status_at(
    kind: ProviderConfigKind,
    connector: &Path,
    path: &Path,
) -> Result<bool, RegistrationError> {
    let raw = read_optional_config(path)?;
    let config = parse_config_for(kind, raw.as_deref())?;
    ensure_owned_entry_unambiguous_doc(&config, kind, connector)?;
    let skill_path = resolve_skill_path(kind)?;
    let skill_ready =
        read_optional_skill(&skill_path)?.is_some_and(|skill| skill == SKILL_SOURCE.as_bytes());
    Ok(skill_ready && config_entry_is_exact(&config, kind, connector))
}

pub fn install(
    plan: &RegistrationPlan,
    permit: &mut RegistrationPermit,
) -> Result<(), RegistrationError> {
    claim(plan, permit)?;
    let raw = read_optional_config(&plan.config_path)?;
    if digest_bytes(raw.as_deref().unwrap_or(b"<absent>")) != plan.config_digest {
        return Err(RegistrationError::ConfigChanged);
    }
    if digest_bytes(
        read_optional_skill(&plan.skill_path)?
            .as_deref()
            .unwrap_or(b"<absent>"),
    ) != plan.skill_digest
    {
        return Err(RegistrationError::ConfigChanged);
    }
    let mut config = parse_config_for(plan.kind, raw.as_deref())?;
    ensure_owned_entry_unambiguous_doc(&config, plan.kind, &plan.connector)?;
    insert_owned_entry(&mut config, plan.kind, &plan.connector)?;
    write_skill(&plan.skill_path)?;
    let published_shared = publish_shared_skill(&plan.shared_skill_path);
    if let Err(error) = write_config_doc(&plan.config_path, &config) {
        if plan.skill_was_absent {
            let _ = fs::remove_file(&plan.skill_path);
        }
        if let Some(path) = published_shared {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    if !config_entry_is_exact(&config, plan.kind, &plan.connector) {
        return Err(RegistrationError::WriteFailed);
    }
    Ok(())
}

pub fn remove(
    plan: &RegistrationPlan,
    permit: &mut RegistrationPermit,
) -> Result<(), RegistrationError> {
    claim(plan, permit)?;
    let raw = read_optional_config(&plan.config_path)?;
    if digest_bytes(raw.as_deref().unwrap_or(b"<absent>")) != plan.config_digest {
        return Err(RegistrationError::ConfigChanged);
    }
    if digest_bytes(
        read_optional_skill(&plan.skill_path)?
            .as_deref()
            .unwrap_or(b"<absent>"),
    ) != plan.skill_digest
    {
        return Err(RegistrationError::ConfigChanged);
    }
    let mut config = parse_config_for(plan.kind, raw.as_deref())?;
    ensure_owned_entry_unambiguous_doc(&config, plan.kind, &plan.connector)?;
    if !remove_owned_entry(&mut config)? {
        return Ok(());
    }
    write_config_doc(&plan.config_path, &config)?;
    if read_optional_skill(&plan.skill_path)?.is_some_and(|skill| skill == SKILL_SOURCE.as_bytes())
    {
        fs::remove_file(&plan.skill_path).map_err(|_| RegistrationError::WriteFailed)?;
    }
    // The shared publication stays: it has no single provider owner, and another
    // registered provider may still rely on it.
    Ok(())
}

fn claim(
    plan: &RegistrationPlan,
    permit: &mut RegistrationPermit,
) -> Result<(), RegistrationError> {
    if permit.consumed {
        return Err(RegistrationError::ApprovalConsumed);
    }
    permit.consumed = true;
    if permit.digest != plan.digest
        || plan_digest(
            plan.kind,
            &plan.connector,
            &plan.config_path,
            &plan.shared_skill_path,
            &plan.config_digest,
            &plan.skill_digest,
        )? != plan.digest
    {
        return Err(RegistrationError::ApprovalMismatch);
    }
    Ok(())
}

/// Reviewed config candidates for one provider, in preference order. Every
/// approved mutation targets exactly one of these locations and nothing else.
fn reviewed_config_candidates(kind: ProviderConfigKind) -> Result<Vec<PathBuf>, RegistrationError> {
    let home = paths::user_home_from_env().ok_or(RegistrationError::ConfigUnavailable)?;
    Ok(match kind {
        ProviderConfigKind::Cursor => vec![home.join(".cursor").join("mcp.json")],
        ProviderConfigKind::Antigravity => vec![
            home.join(".gemini").join("config").join("mcp_config.json"),
            home.join(".gemini")
                .join("antigravity")
                .join("mcp_config.json"),
        ],
        ProviderConfigKind::ClaudeCode => vec![home.join(".claude.json")],
        ProviderConfigKind::Codex => vec![home.join(".codex").join("config.toml")],
    })
}

/// Admit an explicitly discovered config path only when its canonical form is
/// exactly one reviewed candidate for the provider. The path must already
/// exist as a regular file: this route selects the active vendor location the
/// target scan observed; it never creates a new vendor config location.
fn admit_explicit_config_path(
    kind: ProviderConfigKind,
    explicit: &Path,
) -> Result<PathBuf, RegistrationError> {
    if !explicit.is_absolute() {
        return Err(RegistrationError::ConfigPathUnsupported);
    }
    let canonical =
        fs::canonicalize(explicit).map_err(|_| RegistrationError::ConfigPathUnsupported)?;
    let metadata =
        fs::symlink_metadata(&canonical).map_err(|_| RegistrationError::ConfigPathUnsupported)?;
    if !metadata.file_type().is_file() {
        return Err(RegistrationError::ConfigPathUnsupported);
    }
    let candidates = reviewed_config_candidates(kind)?;
    if path_matches_reviewed_candidate(&canonical, &candidates) {
        Ok(canonical)
    } else {
        Err(RegistrationError::ConfigPathUnsupported)
    }
}

fn path_matches_reviewed_candidate(canonical: &Path, candidates: &[PathBuf]) -> bool {
    candidates.iter().any(|candidate| {
        candidate == canonical
            || fs::canonicalize(candidate)
                .ok()
                .is_some_and(|resolved| resolved == canonical)
    })
}

fn resolve_skill_path(kind: ProviderConfigKind) -> Result<PathBuf, RegistrationError> {
    let home = paths::user_home_from_env().ok_or(RegistrationError::ConfigUnavailable)?;
    let root = match kind {
        ProviderConfigKind::Cursor => home.join(".cursor").join("skills"),
        ProviderConfigKind::Antigravity => home.join(".gemini").join("config").join("skills"),
        ProviderConfigKind::ClaudeCode => home.join(".claude").join("skills"),
        ProviderConfigKind::Codex => home.join(".codex").join("skills"),
    };
    Ok(root
        .join(crate::domain::client_conversation::LICOUP_GUIDE_SKILL_ID)
        .join("SKILL.md"))
}

/// The cross-Agent shared Skill surface. Agents that subscribe to it receive
/// the usage Skill even when their own provider-local write is the only other
/// delivery route.
fn shared_skill_path() -> Result<PathBuf, RegistrationError> {
    let home = paths::user_home_from_env().ok_or(RegistrationError::ConfigUnavailable)?;
    Ok(shared_skill_path_in(&home))
}

fn shared_skill_path_in(home: &Path) -> PathBuf {
    home.join(".agents")
        .join("skills")
        .join(crate::domain::client_conversation::LICOUP_GUIDE_SKILL_ID)
        .join("SKILL.md")
}

fn resolve_config_path(kind: ProviderConfigKind) -> Result<PathBuf, RegistrationError> {
    let candidates = reviewed_config_candidates(kind)?;
    match kind {
        ProviderConfigKind::Cursor
        | ProviderConfigKind::ClaudeCode
        | ProviderConfigKind::Codex => Ok(candidates[0].clone()),
        ProviderConfigKind::Antigravity => {
            let official = &candidates[0];
            let legacy = &candidates[1];
            if official.exists() && legacy.exists() {
                return Err(RegistrationError::ConfigAmbiguous);
            }
            if legacy.exists() {
                Ok(legacy.clone())
            } else {
                Ok(official.clone())
            }
        }
    }
}

fn canonical_connector(path: &Path) -> Result<PathBuf, RegistrationError> {
    if !path.is_absolute() {
        return Err(RegistrationError::InvalidConnector);
    }
    let path = fs::canonicalize(path).map_err(|_| RegistrationError::InvalidConnector)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| RegistrationError::InvalidConnector)?;
    if !metadata.file_type().is_file() {
        return Err(RegistrationError::InvalidConnector);
    }
    Ok(path)
}

fn read_optional_config(path: &Path) -> Result<Option<Vec<u8>>, RegistrationError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| RegistrationError::ConfigUnavailable)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(RegistrationError::ConfigUnavailable);
    }
    // A zero-byte vendor file is an absent config, not a corrupt one: the
    // approval digest binds the same "<absent>" marker and the install writes
    // a fresh object. Non-empty invalid JSON stays fail-closed in parse_config.
    let raw = fs::read(path).map_err(|_| RegistrationError::ConfigUnavailable)?;
    if raw.is_empty() {
        return Ok(None);
    }
    Ok(Some(raw))
}

fn read_optional_skill(path: &Path) -> Result<Option<Vec<u8>>, RegistrationError> {
    read_optional_config(path)
}

enum ProviderConfig {
    Json(Value),
    Toml(toml::Table),
}

fn is_licoup_cli_connector(connector: &Path) -> bool {
    connector
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("licoup"))
}

fn is_known_mcp_connector_name(command: &Path) -> bool {
    command
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| {
            stem.eq_ignore_ascii_case("lico-subagent-mcp") || stem.eq_ignore_ascii_case("licoup")
        })
}

fn mcp_args(kind: ProviderConfigKind, connector: &Path) -> Vec<String> {
    if is_licoup_cli_connector(connector) {
        vec![
            "mcp".to_owned(),
            "stdio".to_owned(),
            "--caller".to_owned(),
            kind.provider_id().to_owned(),
        ]
    } else {
        vec!["--caller".to_owned(), kind.provider_id().to_owned()]
    }
}

fn codex_context_env_vars() -> toml::Value {
    toml::Value::Array(
        [
            "LICOUP_HOME",
            "LICOUP_MCP_CONVERSATION_ID",
            "LICOUP_MCP_MEMBERSHIP_ID",
            "LICOUP_MCP_PARENT_DISPATCH_ID",
        ]
        .into_iter()
        .map(|name| toml::Value::String(name.to_owned()))
        .collect(),
    )
}

fn parse_config_for(
    kind: ProviderConfigKind,
    raw: Option<&[u8]>,
) -> Result<ProviderConfig, RegistrationError> {
    match kind {
        ProviderConfigKind::Codex => parse_codex_config(raw).map(ProviderConfig::Toml),
        ProviderConfigKind::Cursor
        | ProviderConfigKind::Antigravity
        | ProviderConfigKind::ClaudeCode => parse_config(raw).map(ProviderConfig::Json),
    }
}

fn parse_codex_config(raw: Option<&[u8]>) -> Result<toml::Table, RegistrationError> {
    let mut table = match raw {
        Some(raw) => {
            let text = std::str::from_utf8(raw).map_err(|_| RegistrationError::ConfigUnavailable)?;
            toml::from_str::<toml::Table>(text).map_err(|_| RegistrationError::ConfigUnavailable)?
        }
        None => toml::Table::new(),
    };
    match table.get("mcp_servers") {
        None => {
            table.insert(
                "mcp_servers".to_owned(),
                toml::Value::Table(toml::Table::new()),
            );
        }
        Some(toml::Value::Table(_)) => {}
        Some(_) => return Err(RegistrationError::ConfigUnavailable),
    }
    Ok(table)
}

fn parse_config(raw: Option<&[u8]>) -> Result<Value, RegistrationError> {
    let mut config = match raw {
        Some(raw) => {
            serde_json::from_slice(raw).map_err(|_| RegistrationError::ConfigUnavailable)?
        }
        None => json!({"mcpServers": {}}),
    };
    let object = config
        .as_object_mut()
        .ok_or(RegistrationError::ConfigUnavailable)?;
    match object.get("mcpServers") {
        None => {
            object.insert("mcpServers".to_owned(), json!({}));
        }
        Some(Value::Object(_)) => {}
        Some(_) => return Err(RegistrationError::ConfigUnavailable),
    }
    Ok(config)
}

fn servers_mut(config: &mut Value) -> Result<&mut Map<String, Value>, RegistrationError> {
    config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or(RegistrationError::ConfigUnavailable)
}

fn codex_servers_mut(config: &mut toml::Table) -> Result<&mut toml::Table, RegistrationError> {
    config
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
        .ok_or(RegistrationError::ConfigUnavailable)
}

fn entry(config: &Value) -> Option<&Map<String, Value>> {
    config
        .get("mcpServers")?
        .as_object()?
        .get(SERVER_ID)?
        .as_object()
}

fn codex_entry(config: &toml::Table) -> Option<&toml::Table> {
    config.get("mcp_servers")?.as_table()?.get(SERVER_ID)?.as_table()
}

fn ensure_owned_entry_unambiguous_doc(
    config: &ProviderConfig,
    kind: ProviderConfigKind,
    connector: &Path,
) -> Result<(), RegistrationError> {
    match config {
        ProviderConfig::Json(json) => ensure_owned_entry_unambiguous(json, kind, connector),
        ProviderConfig::Toml(table) => ensure_owned_codex_entry_unambiguous(table, kind, connector),
    }
}

fn config_entry_is_exact(
    config: &ProviderConfig,
    kind: ProviderConfigKind,
    connector: &Path,
) -> bool {
    match config {
        ProviderConfig::Json(json) => {
            entry(json).is_some_and(|entry| entry_is_exact(entry, kind, connector))
        }
        ProviderConfig::Toml(table) => {
            codex_entry(table).is_some_and(|entry| codex_entry_is_exact(entry, kind, connector))
        }
    }
}

fn insert_owned_entry(
    config: &mut ProviderConfig,
    kind: ProviderConfigKind,
    connector: &Path,
) -> Result<(), RegistrationError> {
    let args = mcp_args(kind, connector);
    match config {
        ProviderConfig::Json(json) => {
            let mut owned_entry = json!({
                "command": connector.to_string_lossy(),
                "args": args,
                "disabled": false,
                "managedBy": MANAGED_BY,
                "schemaVersion": ENTRY_SCHEMA,
                "provider": kind.provider_id(),
            });
            owned_entry["env"] = match kind {
                ProviderConfigKind::Cursor => cursor_context_environment(),
                ProviderConfigKind::Antigravity
                | ProviderConfigKind::ClaudeCode
                | ProviderConfigKind::Codex => antigravity_context_environment(),
            };
            servers_mut(json)?.insert(SERVER_ID.to_owned(), owned_entry);
            Ok(())
        }
        ProviderConfig::Toml(table) => {
            let mut owned_entry = toml::Table::new();
            owned_entry.insert(
                "command".to_owned(),
                toml::Value::String(connector.to_string_lossy().into_owned()),
            );
            owned_entry.insert(
                "args".to_owned(),
                toml::Value::Array(args.into_iter().map(toml::Value::String).collect()),
            );
            owned_entry.insert("enabled".to_owned(), toml::Value::Boolean(true));
            owned_entry.insert("env_vars".to_owned(), codex_context_env_vars());
            codex_servers_mut(table)?.insert(SERVER_ID.to_owned(), toml::Value::Table(owned_entry));
            Ok(())
        }
    }
}

fn remove_owned_entry(config: &mut ProviderConfig) -> Result<bool, RegistrationError> {
    match config {
        ProviderConfig::Json(json) => {
            if entry(json).is_none() {
                return Ok(false);
            }
            servers_mut(json)?.remove(SERVER_ID);
            Ok(true)
        }
        ProviderConfig::Toml(table) => {
            if codex_entry(table).is_none() {
                return Ok(false);
            }
            codex_servers_mut(table)?.remove(SERVER_ID);
            Ok(true)
        }
    }
}

fn ensure_owned_codex_entry_unambiguous(
    config: &toml::Table,
    kind: ProviderConfigKind,
    connector: &Path,
) -> Result<(), RegistrationError> {
    if has_duplicate_codex_connector_entry(config, connector) {
        return Err(RegistrationError::DuplicateConnectorEntry);
    }
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    let Some(raw_entry) = servers.get(SERVER_ID) else {
        return Ok(());
    };
    let Some(entry) = raw_entry.as_table() else {
        return Err(RegistrationError::OwnedEntryAmbiguous);
    };
    let Some(command) = entry.get("command").and_then(toml::Value::as_str) else {
        return Err(RegistrationError::OwnedEntryAmbiguous);
    };
    let args: Vec<&str> = entry
        .get("args")
        .and_then(toml::Value::as_array)
        .map(|items| items.iter().filter_map(toml::Value::as_str).collect())
        .unwrap_or_default();
    let owned_command = (command_is_connector(command, connector)
        && (!is_licoup_cli_connector(Path::new(command)) || args.starts_with(&["mcp", "stdio"])))
        || (is_known_mcp_connector_name(Path::new(command))
            && args
                .windows(2)
                .any(|pair| pair == ["--caller", kind.provider_id()]));
    if owned_command {
        Ok(())
    } else {
        Err(RegistrationError::OwnedEntryAmbiguous)
    }
}

fn has_duplicate_codex_connector_entry(config: &toml::Table, connector: &Path) -> bool {
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return false;
    };
    servers.iter().any(|(key, value)| {
        if key == SERVER_ID {
            return false;
        }
        if key == "lico-up-subagents" {
            return true;
        }
        let Some(table) = value.as_table() else {
            return false;
        };
        let Some(command) = table.get("command").and_then(toml::Value::as_str) else {
            return false;
        };
        if !command_is_connector(command, connector) {
            return false;
        }
        if is_licoup_cli_connector(Path::new(command)) {
            let args: Vec<&str> = table
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|items| items.iter().filter_map(toml::Value::as_str).collect())
                .unwrap_or_default();
            return args.starts_with(&["mcp", "stdio"]);
        }
        true
    })
}

/// Fail closed unless the namespaced entry is LicoUp's own and no sibling entry
/// registers the same connector under another key.
///
/// The owned key is rewritten by `install`; a sibling key for the same
/// connector would leave the client with two servers, two spawned processes and
/// one tool catalog listed twice. Both conditions are reported instead, so
/// `status` never reads as "not installed" while such an entry sits in the
/// config.
fn ensure_owned_entry_unambiguous(
    config: &Value,
    kind: ProviderConfigKind,
    connector: &Path,
) -> Result<(), RegistrationError> {
    if has_duplicate_connector_entry(config, connector) {
        return Err(RegistrationError::DuplicateConnectorEntry);
    }
    let Some(entry) = entry(config) else {
        return Ok(());
    };
    let owned = entry.get("managedBy").and_then(Value::as_str) == Some(MANAGED_BY)
        && entry.get("schemaVersion").and_then(Value::as_str) == Some(ENTRY_SCHEMA)
        && entry.get("provider").and_then(Value::as_str) == Some(kind.provider_id());
    if owned {
        Ok(())
    } else {
        Err(RegistrationError::OwnedEntryAmbiguous)
    }
}

/// Two shapes count as the same connector under a different key: an entry whose
/// `command` resolves to the connector, and an entry keyed by the connector's
/// own reported server identity. An earlier registration shape used that
/// identity as the key, which is why such an entry looks legitimate at a glance.
fn has_duplicate_connector_entry(config: &Value, connector: &Path) -> bool {
    let Some(servers) = config.get("mcpServers").and_then(Value::as_object) else {
        return false;
    };
    servers.iter().any(|(key, value)| {
        if key == SERVER_ID {
            return false;
        }
        if key == "lico-up-subagents" {
            return true;
        }
        let Some(object) = value.as_object() else {
            return false;
        };
        let Some(command) = object.get("command").and_then(Value::as_str) else {
            return false;
        };
        if !command_is_connector(command, connector) {
            return false;
        }
        if is_licoup_cli_connector(Path::new(command)) {
            let args: Vec<&str> = object
                .get("args")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            return args.starts_with(&["mcp", "stdio"]);
        }
        true
    })
}

/// Whether one `mcpServers[].command` names the connector.
///
/// A bare name is resolved through the client's `PATH`, not against the current
/// directory, so `fs::canonicalize` cannot settle it here: a sibling entry
/// holding the bare connector name would read as unrelated and another server
/// would be registered beside it. Bare names are therefore compared by file
/// name, which is the identity the client would resolve.
fn command_is_connector(command: &str, connector: &Path) -> bool {
    let command = Path::new(command);
    if !command.is_absolute()
        && command
            .parent()
            .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return command.file_name() == connector.file_name();
    }
    fs::canonicalize(command)
        .ok()
        .is_some_and(|resolved| resolved == connector)
}

fn entry_is_exact(entry: &Map<String, Value>, kind: ProviderConfigKind, connector: &Path) -> bool {
    let expected_args: Vec<Value> = mcp_args(kind, connector).into_iter().map(Value::String).collect();
    let args_exact = entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| args == &expected_args);
    let environment_exact = match kind {
        ProviderConfigKind::Cursor => entry.get("env") == Some(&cursor_context_environment()),
        ProviderConfigKind::Antigravity
        | ProviderConfigKind::ClaudeCode
        | ProviderConfigKind::Codex => {
            entry.get("env") == Some(&antigravity_context_environment())
        }
    };
    if entry.get("managedBy").and_then(Value::as_str) != Some(MANAGED_BY)
        || entry.get("schemaVersion").and_then(Value::as_str) != Some(ENTRY_SCHEMA)
        || entry.get("provider").and_then(Value::as_str) != Some(kind.provider_id())
        || entry.get("disabled").and_then(Value::as_bool) != Some(false)
        || !args_exact
        || !environment_exact
    {
        return false;
    }
    entry
        .get("command")
        .and_then(Value::as_str)
        .map(Path::new)
        .and_then(|path| fs::canonicalize(path).ok())
        .is_some_and(|path| path == connector)
}

fn codex_entry_is_exact(
    entry: &toml::Table,
    kind: ProviderConfigKind,
    connector: &Path,
) -> bool {
    let expected_args: Vec<toml::Value> = mcp_args(kind, connector)
        .into_iter()
        .map(toml::Value::String)
        .collect();
    let args_exact = entry
        .get("args")
        .and_then(toml::Value::as_array)
        .is_some_and(|args| args == &expected_args);
    if entry.get("enabled").and_then(toml::Value::as_bool) != Some(true)
        || !args_exact
        || entry.get("env_vars") != Some(&codex_context_env_vars())
    {
        return false;
    }
    entry
        .get("command")
        .and_then(toml::Value::as_str)
        .map(Path::new)
        .and_then(|path| fs::canonicalize(path).ok())
        .is_some_and(|path| path == connector)
}

fn write_config_doc(path: &Path, config: &ProviderConfig) -> Result<(), RegistrationError> {
    let text = match config {
        ProviderConfig::Json(json) => {
            serde_json::to_string_pretty(json).map_err(|_| RegistrationError::WriteFailed)?
        }
        ProviderConfig::Toml(table) => {
            toml::to_string_pretty(table).map_err(|_| RegistrationError::WriteFailed)?
        }
    };
    write_config_text(path, &text)
}

fn write_config_text(path: &Path, text: &str) -> Result<(), RegistrationError> {
    let parent = path.parent().ok_or(RegistrationError::WriteFailed)?;
    file_security::ensure_private_dir(parent).map_err(|_| RegistrationError::WriteFailed)?;
    if path.exists() {
        file_security::harden_private_path(path).map_err(|_| RegistrationError::WriteFailed)?;
    }
    file_security::atomic_write_private_text_bounded(path, text, MAX_CONFIG_BYTES as usize)
        .map_err(|_| RegistrationError::WriteFailed)
}

fn write_skill(path: &Path) -> Result<(), RegistrationError> {
    let parent = path.parent().ok_or(RegistrationError::WriteFailed)?;
    file_security::ensure_private_dir(parent).map_err(|_| RegistrationError::WriteFailed)?;
    if path.exists() {
        file_security::harden_private_path(path).map_err(|_| RegistrationError::WriteFailed)?;
    }
    file_security::atomic_write_private_text_bounded(path, SKILL_SOURCE, 64 * 1024)
        .map_err(|_| RegistrationError::WriteFailed)
}

/// Publish the embedded Skill as one copy on the shared surface.
///
/// The provider-local artifact is the one byte-verified against this connector;
/// the shared copy deliberately is not, because a single shared file serves
/// every installed connector version and a differing copy is version drift, not
/// a foreign artifact. The publication is secondary, so its failure never fails
/// an approved registration, and it returns the path only when this call
/// created it, so a failed config write can roll it back.
fn publish_shared_skill(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        let _ = write_skill(path);
        return None;
    }
    write_skill(path).ok()?;
    Some(path.to_path_buf())
}

fn plan_digest(
    kind: ProviderConfigKind,
    connector: &Path,
    config_path: &Path,
    shared_skill_path: &Path,
    config_digest: &str,
    skill_digest: &str,
) -> Result<String, RegistrationError> {
    let binary = fs::read(connector).map_err(|_| RegistrationError::InvalidConnector)?;
    let mut digest = Sha256::new();
    for value in [
        ENTRY_SCHEMA.as_bytes(),
        kind.provider_id().as_bytes(),
        SERVER_ID.as_bytes(),
        config_path.to_string_lossy().as_bytes(),
        shared_skill_path.to_string_lossy().as_bytes(),
        config_digest.as_bytes(),
        skill_digest.as_bytes(),
        SKILL_SOURCE.as_bytes(),
        &binary,
    ] {
        digest.update(value);
        digest.update([0]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn digest_bytes(value: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_namespaced_entry_fails_closed() {
        let config = json!({"mcpServers": {
            SERVER_ID: {"command": "foreign"}
        }});
        assert_eq!(
            ensure_owned_entry_unambiguous(
                &config,
                ProviderConfigKind::Cursor,
                Path::new("/absent-connector")
            ),
            Err(RegistrationError::OwnedEntryAmbiguous)
        );
    }

    #[test]
    fn same_connector_under_another_key_is_reported_instead_of_registered_twice() {
        let root = std::env::temp_dir().join(format!(
            "licoup-provider-duplicate-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(&connector).unwrap();

        // The shape a superseded registration wrote: the connector's own
        // reported identity as the key, no ownership fields, no `--caller`.
        let mut servers = Map::new();
        servers.insert(
            "lico-up-subagents".to_owned(),
            json!({
                "type": "stdio",
                "command": connector.to_string_lossy(),
                "args": [],
                "env": {}
            }),
        );
        let legacy_key = json!({"mcpServers": servers});
        assert_eq!(
            ensure_owned_entry_unambiguous(&legacy_key, ProviderConfigKind::ClaudeCode, &connector),
            Err(RegistrationError::DuplicateConnectorEntry)
        );

        // The same connector under an arbitrary key, matched by its command.
        let arbitrary_key = json!({"mcpServers": {
            "some-server": {"command": connector.to_string_lossy()}
        }});
        assert_eq!(
            ensure_owned_entry_unambiguous(
                &arbitrary_key,
                ProviderConfigKind::ClaudeCode,
                &connector
            ),
            Err(RegistrationError::DuplicateConnectorEntry)
        );

        // A bare name is resolved through the client's PATH, so it cannot be
        // canonicalized here. It still names the same connector.
        let bare_name = json!({"mcpServers": {
            "some-server": {"command": "lico-subagent-mcp"}
        }});
        assert_eq!(
            ensure_owned_entry_unambiguous(&bare_name, ProviderConfigKind::ClaudeCode, &connector),
            Err(RegistrationError::DuplicateConnectorEntry)
        );

        // A bare name for a different connector stays legal.
        let bare_other = json!({"mcpServers": {
            "some-server": {"command": "some-other-connector"}
        }});
        assert_eq!(
            ensure_owned_entry_unambiguous(&bare_other, ProviderConfigKind::ClaudeCode, &connector),
            Ok(())
        );

        // A sibling entry for a different connector stays legal.
        let other = root.join("other-connector");
        fs::write(&other, b"synthetic connector").unwrap();
        let unrelated = json!({"mcpServers": {
            "other": {"command": fs::canonicalize(&other).unwrap().to_string_lossy()}
        }});
        assert_eq!(
            ensure_owned_entry_unambiguous(&unrelated, ProviderConfigKind::ClaudeCode, &connector),
            Ok(())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exact_owned_entry_is_recognized_without_touching_siblings() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-mcp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(connector).unwrap();
        let config = json!({"mcpServers": {
            "other": {"command": "other"},
            SERVER_ID: {
                "command": connector.to_string_lossy(),
                "args": ["--caller", "cursor"],
                "env": {
                    "LICOUP_HOME": "${env:LICOUP_HOME}",
                    "LICOUP_MCP_CONVERSATION_ID": "${env:LICOUP_MCP_CONVERSATION_ID}",
                    "LICOUP_MCP_MEMBERSHIP_ID": "${env:LICOUP_MCP_MEMBERSHIP_ID}",
                    "LICOUP_MCP_PARENT_DISPATCH_ID": "${env:LICOUP_MCP_PARENT_DISPATCH_ID}"
                },
                "disabled": false,
                "managedBy": MANAGED_BY,
                "schemaVersion": ENTRY_SCHEMA,
                "provider": "cursor"
            }
        }});
        assert!(entry(&config).is_some_and(|entry| {
            entry_is_exact(entry, ProviderConfigKind::Cursor, &connector)
        }));
        assert!(config["mcpServers"].get("other").is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn antigravity_owned_entry_requires_gemini_env_interpolation() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-agy-env-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(connector).unwrap();
        let mut entry = json!({
            "command": connector.to_string_lossy(),
            "args": ["--caller", "antigravity"],
            "disabled": false,
            "managedBy": MANAGED_BY,
            "schemaVersion": ENTRY_SCHEMA,
            "provider": "antigravity"
        })
        .as_object()
        .cloned()
        .unwrap();
        assert!(!entry_is_exact(
            &entry,
            ProviderConfigKind::Antigravity,
            &connector
        ));
        entry.insert("env".to_owned(), cursor_context_environment());
        assert!(!entry_is_exact(
            &entry,
            ProviderConfigKind::Antigravity,
            &connector
        ));
        entry.insert("env".to_owned(), antigravity_context_environment());
        assert!(entry_is_exact(
            &entry,
            ProviderConfigKind::Antigravity,
            &connector
        ));
        assert_eq!(
            antigravity_context_environment()["LICOUP_HOME"],
            "${LICOUP_HOME}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_code_owned_entry_uses_shell_env_interpolation() {
        let root = std::env::temp_dir().join(format!(
            "licoup-provider-claude-env-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(connector).unwrap();
        let mut entry = json!({
            "command": connector.to_string_lossy(),
            "args": ["--caller", "claude-code"],
            "disabled": false,
            "managedBy": MANAGED_BY,
            "schemaVersion": ENTRY_SCHEMA,
            "provider": "claude-code"
        })
        .as_object()
        .cloned()
        .unwrap();
        assert!(!entry_is_exact(
            &entry,
            ProviderConfigKind::ClaudeCode,
            &connector
        ));
        entry.insert("env".to_owned(), cursor_context_environment());
        assert!(!entry_is_exact(
            &entry,
            ProviderConfigKind::ClaudeCode,
            &connector
        ));
        entry.insert("env".to_owned(), antigravity_context_environment());
        assert!(entry_is_exact(
            &entry,
            ProviderConfigKind::ClaudeCode,
            &connector
        ));
        assert_eq!(ProviderConfigKind::ClaudeCode.provider_id(), "claude-code");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approval_is_single_use_and_bound_to_unchanged_config_bytes() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-approval-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let config_path = root.join("mcp.json");
        let config_digest = digest_bytes(b"<absent>");
        let skill_path = root.join("skills/licoup-guide/SKILL.md");
        let skill_digest = digest_bytes(b"<absent>");
        let shared_skill_path = root.join("shared/skills/licoup-guide/SKILL.md");
        let digest = plan_digest(
            ProviderConfigKind::Cursor,
            &connector,
            &config_path,
            &shared_skill_path,
            &config_digest,
            &skill_digest,
        )
        .unwrap();
        let plan = RegistrationPlan {
            kind: ProviderConfigKind::Cursor,
            connector,
            config_path: config_path.clone(),
            shared_skill_path,
            config_digest,
            skill_path,
            skill_digest,
            skill_was_absent: true,
            digest,
        };
        let mut permit = plan.approve(true, plan.digest()).unwrap();
        fs::write(&config_path, b"{\"mcpServers\":{}}").unwrap();
        assert_eq!(
            install(&plan, &mut permit),
            Err(RegistrationError::ConfigChanged)
        );
        assert_eq!(
            install(&plan, &mut permit),
            Err(RegistrationError::ApprovalConsumed)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn one_approval_delivers_owned_entry_and_skill_without_touching_sibling() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-delivery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(connector).unwrap();
        let config_path = root.join("mcp.json");
        let config_source = br#"{"mcpServers":{"other":{"command":"other"}}}"#;
        fs::write(&config_path, config_source).unwrap();
        let config_digest = digest_bytes(config_source);
        let skill_path = root.join("skills/licoup-guide/SKILL.md");
        let skill_digest = digest_bytes(b"<absent>");
        let shared_skill_path = root.join("shared/skills/licoup-guide/SKILL.md");
        let digest = plan_digest(
            ProviderConfigKind::Cursor,
            &connector,
            &config_path,
            &shared_skill_path,
            &config_digest,
            &skill_digest,
        )
        .unwrap();
        let plan = RegistrationPlan {
            kind: ProviderConfigKind::Cursor,
            connector,
            config_path: config_path.clone(),
            shared_skill_path: shared_skill_path.clone(),
            config_digest,
            skill_path: skill_path.clone(),
            skill_digest,
            skill_was_absent: true,
            digest,
        };
        let mut permit = plan.approve(true, plan.digest()).unwrap();
        install(&plan, &mut permit).unwrap();
        let config: Value = serde_json::from_slice(&fs::read(config_path).unwrap()).unwrap();
        assert!(config["mcpServers"].get("other").is_some());
        assert!(config["mcpServers"].get(SERVER_ID).is_some());
        assert_eq!(
            config["mcpServers"][SERVER_ID]["env"],
            cursor_context_environment()
        );
        assert_eq!(fs::read_to_string(skill_path).unwrap(), SKILL_SOURCE);
        assert_eq!(fs::read_to_string(shared_skill_path).unwrap(), SKILL_SOURCE);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shared_surface_publication_serves_the_embedded_skill_as_an_unverified_copy() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-shared-{}", uuid::Uuid::new_v4()));
        let shared = shared_skill_path_in(&root);
        assert_eq!(
            shared,
            root.join(".agents")
                .join("skills")
                .join(crate::domain::client_conversation::LICOUP_GUIDE_SKILL_ID)
                .join("SKILL.md")
        );
        assert_eq!(publish_shared_skill(&shared), Some(shared.clone()));
        assert_eq!(fs::read_to_string(&shared).unwrap(), SKILL_SOURCE);
        // A copy that already exists is refreshed but never claimed as created,
        // so a failed config write cannot remove another connector's copy.
        assert_eq!(publish_shared_skill(&shared), None);
        fs::write(&shared, b"another connector version").unwrap();
        assert_eq!(publish_shared_skill(&shared), None);
        assert_eq!(fs::read_to_string(&shared).unwrap(), SKILL_SOURCE);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_config_path_must_canonicalize_to_a_reviewed_candidate() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-explicit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let home = paths::user_home_from_env().unwrap();
        let foreign = root.join("foreign.json");
        fs::write(&foreign, b"{\"mcpServers\":{}}").unwrap();
        assert_eq!(
            RegistrationPlan::prepare_with_config_path(
                ProviderConfigKind::Antigravity,
                &connector,
                &foreign,
            )
            .unwrap_err(),
            RegistrationError::ConfigPathUnsupported
        );
        let relative = Path::new("mcp_config.json");
        assert_eq!(
            RegistrationPlan::prepare_with_config_path(
                ProviderConfigKind::Antigravity,
                &connector,
                relative,
            )
            .unwrap_err(),
            RegistrationError::ConfigPathUnsupported
        );
        let missing = home
            .join(".gemini")
            .join("config")
            .join("definitely-absent-mcp_config.json");
        assert_eq!(
            RegistrationPlan::prepare_with_config_path(
                ProviderConfigKind::Antigravity,
                &connector,
                &missing,
            )
            .unwrap_err(),
            RegistrationError::ConfigPathUnsupported
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_antigravity_candidates_both_admitted_with_path_bound_digest() {
        let home = paths::user_home_from_env().unwrap();
        let official = home.join(".gemini").join("config").join("mcp_config.json");
        let legacy = home
            .join(".gemini")
            .join("antigravity")
            .join("mcp_config.json");
        if !official.exists() || !legacy.exists() {
            // The both-present environment is exercised by the installed
            // contract run; without it this unit stays a pure admission check.
            return;
        }
        let root =
            std::env::temp_dir().join(format!("licoup-provider-agy-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("lico-subagent-mcp");
        fs::write(&connector, b"synthetic connector").unwrap();
        let connector = fs::canonicalize(connector).unwrap();
        // Default resolution stays fail-closed while both candidates exist.
        assert_eq!(
            RegistrationPlan::prepare(ProviderConfigKind::Antigravity, &connector).unwrap_err(),
            RegistrationError::ConfigAmbiguous
        );
        let official_plan = RegistrationPlan::prepare_with_config_path(
            ProviderConfigKind::Antigravity,
            &connector,
            &official,
        )
        .unwrap();
        let legacy_plan = RegistrationPlan::prepare_with_config_path(
            ProviderConfigKind::Antigravity,
            &connector,
            &legacy,
        )
        .unwrap();
        assert_ne!(official_plan.digest(), legacy_plan.digest());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reviewed_candidate_matching_is_exact_and_canonical() {
        let root =
            std::env::temp_dir().join(format!("licoup-provider-match-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let candidate = root.join("mcp_config.json");
        fs::write(&candidate, b"{}").unwrap();
        let canonical = fs::canonicalize(&candidate).unwrap();
        let candidates = vec![candidate.clone()];
        assert!(path_matches_reviewed_candidate(&canonical, &candidates));
        assert!(path_matches_reviewed_candidate(&candidate, &candidates));
        let sibling = root.join("other.json");
        fs::write(&sibling, b"{}").unwrap();
        let sibling = fs::canonicalize(sibling).unwrap();
        assert!(!path_matches_reviewed_candidate(&sibling, &candidates));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_toml_registration_preserves_existing_settings_and_supports_licoup_stdio() {
        let root =
            std::env::temp_dir().join(format!("licoup-codex-toml-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let connector = root.join("licoup");
        fs::write(&connector, b"synthetic licoup").unwrap();
        let connector = fs::canonicalize(&connector).unwrap();
        let config_path = root.join("config.toml");
        let existing_toml = b"model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"other-mcp\"\n";
        let mut config = parse_config_for(ProviderConfigKind::Codex, Some(existing_toml)).unwrap();
        ensure_owned_entry_unambiguous_doc(&config, ProviderConfigKind::Codex, &connector).unwrap();
        insert_owned_entry(&mut config, ProviderConfigKind::Codex, &connector).unwrap();
        write_config_doc(&config_path, &config).unwrap();

        let written = fs::read(&config_path).unwrap();
        let reparsed = parse_config_for(ProviderConfigKind::Codex, Some(&written)).unwrap();
        assert!(config_entry_is_exact(
            &reparsed,
            ProviderConfigKind::Codex,
            &connector
        ));
        let text = String::from_utf8(written).unwrap();
        assert!(text.contains("model = \"gpt-5\""));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains("args = [\"mcp\", \"stdio\", \"--caller\", \"codex\"]")
            || (text.contains("\"mcp\"") && text.contains("\"stdio\"") && text.contains("\"codex\"")));
        let parsed_toml: toml::Table = toml::from_str(&text).unwrap();
        let entry = codex_entry(&parsed_toml).expect("codex server entry");
        assert_eq!(entry.get("enabled").and_then(toml::Value::as_bool), Some(true));
        let args: Vec<&str> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str)
            .collect();
        assert_eq!(args, ["mcp", "stdio", "--caller", "codex"]);

        let mut mutable = reparsed;
        assert!(remove_owned_entry(&mut mutable).unwrap());
        write_config_doc(&config_path, &mutable).unwrap();
        let after_remove = fs::read_to_string(&config_path).unwrap();
        assert!(after_remove.contains("model = \"gpt-5\""));
        assert!(!after_remove.contains(SERVER_ID));

        let foreign = parse_config_for(
            ProviderConfigKind::Codex,
            Some(b"[mcp_servers.\"land.lico.licoup.subagents\"]\ncommand = \"foreign\"\n"),
        )
        .unwrap();
        assert_eq!(
            ensure_owned_entry_unambiguous_doc(&foreign, ProviderConfigKind::Codex, &connector),
            Err(RegistrationError::OwnedEntryAmbiguous)
        );
        let _ = fs::remove_dir_all(root);
    }
}
