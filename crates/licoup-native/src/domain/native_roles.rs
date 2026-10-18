//! Discover local native custom roles and map them to Membership Profile
//! intent without putting prompts on the public snapshot.
//!
//! Instructions stay on disk and are re-read at dispatch through the
//! `native-role:{host}/{slug}` skill reference.

use crate::domain::targets::scan_paths::{self, HostRoots};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const SKILL_PREFIX: &str = "native-role:";
const MAX_ROLE_FILE_BYTES: u64 = 256 * 1024;
const MAX_SLUG_BYTES: usize = 64;
const MAX_PROFILE_FIELD_BYTES: usize = 128;
const SKIPPED_FILENAMES: &[&str] = &["agents.better-plan.json"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRole {
    pub id: String,
    pub host_agent_id: String,
    pub slug: String,
    pub name: String,
    pub instructions: String,
    pub preferred_model: Option<String>,
    pub preferred_reasoning_effort: Option<String>,
}

impl NativeRole {
    pub fn principal_id(&self) -> String {
        format!("agent:{}:{}", self.host_agent_id, self.slug)
    }

    pub fn skill_reference(&self) -> &str {
        &self.id
    }

    pub fn public_projection(&self) -> Value {
        json!({
            "id": self.id,
            "hostAgentId": self.host_agent_id,
            "name": self.name,
            "preferredModel": self.preferred_model,
            "preferredReasoningEffort": self.preferred_reasoning_effort,
            "hasInstructions": !self.instructions.trim().is_empty(),
        })
    }

    pub fn profile_intent_update(&self) -> crate::domain::client_conversation::ProfileIntentUpdate {
        crate::domain::client_conversation::ProfileIntentUpdate {
            skill_references: vec![self.id.clone()],
            preferred_model: bounded_profile_field(self.preferred_model.as_deref()),
            preferred_reasoning_effort: bounded_profile_field(
                self.preferred_reasoning_effort.as_deref(),
            ),
            ..crate::domain::client_conversation::ProfileIntentUpdate::default()
        }
    }
}

pub fn skill_reference(host_agent_id: &str, slug: &str) -> String {
    format!("{SKILL_PREFIX}{host_agent_id}/{slug}")
}

pub fn parse_skill_reference(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix(SKILL_PREFIX)?;
    rest.split_once('/')
        .filter(|(host, slug)| !host.is_empty() && !slug.is_empty())
}

pub fn role_from_skill_refs(skill_references: &[String]) -> Option<NativeRole> {
    skill_references
        .iter()
        .find_map(|item| item.starts_with(SKILL_PREFIX).then(|| find(item)).flatten())
}

pub fn list() -> Vec<NativeRole> {
    #[cfg(test)]
    {
        return test_roles().unwrap_or_default();
    }
    #[cfg(not(test))]
    discover_with(&HostRoots::from_environment())
}

pub fn find(id: &str) -> Option<NativeRole> {
    let needle = id.trim();
    if needle.is_empty() {
        return None;
    }
    list().into_iter().find(|role| role.id == needle)
}

pub fn discover_with(roots: &HostRoots) -> Vec<NativeRole> {
    let mut seen = BTreeSet::new();
    let mut roles = Vec::new();
    for (host, path) in discovery_paths(roots) {
        collect_from_path(host, &path, roots, &mut seen, &mut roles);
    }
    roles.sort_by(|left, right| left.id.cmp(&right.id));
    roles
}

#[cfg(test)]
static TEST_ROLES: std::sync::Mutex<Option<Vec<NativeRole>>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub struct TestRolesGuard;

#[cfg(test)]
impl Drop for TestRolesGuard {
    fn drop(&mut self) {
        *TEST_ROLES
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }
}

#[cfg(test)]
pub fn install_test_roles(roles: Vec<NativeRole>) -> TestRolesGuard {
    *TEST_ROLES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(roles);
    TestRolesGuard
}

#[cfg(test)]
fn test_roles() -> Option<Vec<NativeRole>> {
    TEST_ROLES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

fn discovery_paths(roots: &HostRoots) -> Vec<(&'static str, PathBuf)> {
    let mut paths = Vec::new();
    if let Some(home) = roots.home.as_ref() {
        paths.push((
            "opencode",
            home.join(".config").join("opencode").join("agents"),
        ));
        paths.push((
            "opencode",
            home.join(".config").join("opencode").join("opencode.jsonc"),
        ));
        paths.push((
            "opencode",
            home.join(".config").join("opencode").join("opencode.json"),
        ));
        paths.push(("codex", home.join(".codex").join("agents")));
        paths.push(("claude-code", home.join(".claude").join("agents")));
        paths.push(("cursor", home.join(".cursor").join("agents")));
    }
    if let Some(xdg) = roots.xdg_config.as_ref() {
        paths.push(("opencode", xdg.join("opencode").join("agents")));
        paths.push(("opencode", xdg.join("opencode").join("opencode.jsonc")));
        paths.push(("opencode", xdg.join("opencode").join("opencode.json")));
    }
    if let Some(appdata) = roots.appdata.as_ref() {
        paths.push(("opencode", appdata.join("opencode").join("agents")));
        paths.push(("opencode", appdata.join("opencode").join("opencode.jsonc")));
    }
    paths
}

fn collect_from_path(
    host: &str,
    path: &Path,
    roots: &HostRoots,
    seen: &mut BTreeSet<String>,
    roles: &mut Vec<NativeRole>,
) {
    if !scan_paths::probe_exists_with(path, roots) {
        return;
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() => metadata,
        _ => return,
    };
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_role_file(host, &entry.path(), roots, seen, roles);
        }
        return;
    }
    if metadata.is_file() {
        collect_config_table(host, path, roots, seen, roles);
    }
}

fn collect_role_file(
    host: &str,
    path: &Path,
    roots: &HostRoots,
    seen: &mut BTreeSet<String>,
    roles: &mut Vec<NativeRole>,
) {
    if !scan_paths::probe_exists_with(path, roots) {
        return;
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_ROLE_FILE_BYTES =>
        {
            metadata
        }
        _ => return,
    };
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    if SKIPPED_FILENAMES.contains(&file_name) {
        return;
    }
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return;
    };
    let Some(slug) = sanitize_slug(stem) else {
        return;
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let parsed = match extension.as_str() {
        "toml" => parse_toml_role(host, &slug, &raw),
        "md" | "markdown" => parse_markdown_role(host, &slug, &raw),
        "json" | "jsonc" => {
            let _ = metadata;
            None
        }
        _ => None,
    };
    if let Some(role) = parsed {
        push_role(role, seen, roles);
    }
}

fn collect_config_table(
    host: &str,
    path: &Path,
    roots: &HostRoots,
    seen: &mut BTreeSet<String>,
    roles: &mut Vec<NativeRole>,
) {
    if !scan_paths::probe_exists_with(path, roots) {
        return;
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_ROLE_FILE_BYTES =>
        {
            metadata
        }
        _ => return,
    };
    let _ = metadata;
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    if SKIPPED_FILENAMES.contains(&file_name) {
        return;
    }
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let Some(value) = parse_json_document(&raw) else {
        return;
    };
    for key in ["agent", "agents"] {
        let Some(table) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (name, spec) in table {
            let Some(slug) = sanitize_slug(name) else {
                continue;
            };
            if let Some(role) = parse_json_role(host, &slug, spec) {
                push_role(role, seen, roles);
            }
        }
    }
}

fn parse_markdown_role(host: &str, slug: &str, raw: &str) -> Option<NativeRole> {
    let (fields, body) = split_frontmatter(raw);
    Some(build_role(
        host,
        slug,
        first_field(&fields, &["name", "title"]),
        first_nonempty([
            Some(body),
            first_field(&fields, &["prompt", "instructions"]),
        ]),
        first_field(&fields, &["model"]),
        first_field(
            &fields,
            &["reasoning_effort", "effort", "thinking", "reasoningEffort"],
        ),
    ))
}

fn parse_toml_role(host: &str, slug: &str, raw: &str) -> Option<NativeRole> {
    let file: TomlRoleFile = toml::from_str(raw).ok()?;
    Some(build_role(
        host,
        slug,
        file.name.as_deref(),
        file.developer_instructions
            .as_deref()
            .or(file.instructions.as_deref())
            .or(file.prompt.as_deref())
            .unwrap_or(""),
        file.model.as_deref(),
        file.reasoning_effort
            .as_deref()
            .or(file.effort.as_deref())
            .or(file.thinking.as_deref()),
    ))
}

fn parse_json_role(host: &str, slug: &str, spec: &Value) -> Option<NativeRole> {
    let object = spec.as_object()?;
    Some(build_role(
        host,
        slug,
        text_field(object, &["name", "title"]),
        text_field(
            object,
            &["prompt", "instructions", "system", "developer_instructions"],
        )
        .unwrap_or(""),
        text_field(object, &["model"]),
        text_field(
            object,
            &["reasoning_effort", "effort", "thinking", "reasoningEffort"],
        ),
    ))
}

fn build_role(
    host: &str,
    slug: &str,
    name: Option<&str>,
    instructions: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> NativeRole {
    let name = name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(slug)
        .to_owned();
    NativeRole {
        id: skill_reference(host, slug),
        host_agent_id: host.to_owned(),
        slug: slug.to_owned(),
        name,
        instructions: instructions.trim().to_owned(),
        preferred_model: bounded_profile_field(model),
        preferred_reasoning_effort: bounded_profile_field(effort),
    }
}

fn push_role(role: NativeRole, seen: &mut BTreeSet<String>, roles: &mut Vec<NativeRole>) {
    if seen.insert(role.id.clone()) {
        roles.push(role);
    }
}

fn sanitize_slug(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_SLUG_BYTES {
        return None;
    }
    let valid = trimmed.chars().enumerate().all(|(index, character)| {
        character.is_ascii_alphanumeric()
            || character == '_'
            || character == '-'
            || (character == '.' && index > 0)
    });
    valid.then(|| trimmed.to_ascii_lowercase())
}

fn bounded_profile_field(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.len() <= MAX_PROFILE_FIELD_BYTES)
        .map(str::to_owned)
}

fn first_field<'a>(fields: &'a BTreeMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| fields.get(*key).map(String::as_str))
}

fn first_nonempty<'a>(values: [Option<&'a str>; 2]) -> &'a str {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or("")
}

fn text_field<'a>(object: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn split_frontmatter(raw: &str) -> (BTreeMap<String, String>, &str) {
    let text = raw.trim_start_matches('\u{feff}');
    let Some(after_open) = text.strip_prefix("---") else {
        return (BTreeMap::new(), text);
    };
    let after_open = after_open.trim_start_matches(['\r', '\n']);
    let Some(end) = after_open.find("\n---") else {
        return (BTreeMap::new(), text);
    };
    let header = &after_open[..end];
    let body = after_open[end + 4..]
        .trim_start_matches('-')
        .trim_start_matches(['\r', '\n']);
    let mut fields = BTreeMap::new();
    for line in header.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            fields.insert(key.to_owned(), value.to_owned());
        }
    }
    (fields, body)
}

fn parse_json_document(raw: &str) -> Option<Value> {
    serde_json::from_str(raw)
        .ok()
        .or_else(|| serde_json::from_str(&strip_json_comments(raw)).ok())
}

fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    output.push(next);
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        output.push(ch);
    }
    output
}

#[derive(Debug, Deserialize)]
struct TomlRoleFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    developer_instructions: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FixtureHome {
        path: PathBuf,
    }

    impl FixtureHome {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let path = std::env::temp_dir().join(format!("licoup-native-roles-{suffix}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for FixtureHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn discovers_opencode_markdown_jsonc_and_codex_toml() {
        let home = FixtureHome::new();
        let opencode_agents = home.path.join(".config").join("opencode").join("agents");
        let codex_agents = home.path.join(".codex").join("agents");
        fs::create_dir_all(&opencode_agents).unwrap();
        fs::create_dir_all(&codex_agents).unwrap();
        fs::write(
            opencode_agents.join("reviewer.md"),
            "---\ndescription: review\nmodel: anthropic/claude-sonnet-4\nreasoning_effort: high\n---\nYou review diffs.\n",
        )
        .unwrap();
        fs::write(
            home.path
                .join(".config")
                .join("opencode")
                .join("opencode.jsonc"),
            r#"{
              // local overlay
              "agent": {
                "planner": {
                  "prompt": "Plan the work.",
                  "model": "openai/gpt-5",
                  "effort": "medium"
                }
              }
            }"#,
        )
        .unwrap();
        fs::write(
            opencode_agents.join("agents.better-plan.json"),
            "{\"prompt\":\"must-not-load\"}",
        )
        .unwrap();
        fs::write(
            codex_agents.join("architect.toml"),
            "name = \"Architect\"\ndeveloper_instructions = \"Design the system.\"\nmodel = \"gpt-5\"\nreasoning_effort = \"xhigh\"\n",
        )
        .unwrap();

        let roots = HostRoots::from_home(&home.path);
        let roles = discover_with(&roots);
        let reviewer = roles
            .iter()
            .find(|role| role.id == "native-role:opencode/reviewer")
            .unwrap();
        assert_eq!(reviewer.host_agent_id, "opencode");
        assert_eq!(reviewer.name, "reviewer");
        assert_eq!(
            reviewer.preferred_model.as_deref(),
            Some("anthropic/claude-sonnet-4")
        );
        assert_eq!(reviewer.preferred_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(reviewer.instructions, "You review diffs.");
        let planner = roles
            .iter()
            .find(|role| role.id == "native-role:opencode/planner")
            .unwrap();
        assert_eq!(planner.instructions, "Plan the work.");
        assert_eq!(planner.preferred_model.as_deref(), Some("openai/gpt-5"));
        let architect = roles
            .iter()
            .find(|role| role.id == "native-role:codex/architect")
            .unwrap();
        assert_eq!(architect.name, "Architect");
        assert_eq!(architect.instructions, "Design the system.");
        assert_eq!(
            architect.preferred_reasoning_effort.as_deref(),
            Some("xhigh")
        );
        assert!(
            roles.iter().all(|role| !role.id.contains("better-plan")
                && !role.instructions.contains("must-not-load"))
        );

        let projected = reviewer.public_projection();
        let encoded = serde_json::to_string(&projected).unwrap();
        assert!(!encoded.contains("You review diffs"));
        assert!(!encoded.contains("prompt"));
        assert_eq!(projected["hasInstructions"], true);
        let intent = reviewer.profile_intent_update();
        assert_eq!(
            intent.skill_references,
            vec!["native-role:opencode/reviewer".to_owned()]
        );
        assert!(
            serde_json::to_string(&intent)
                .unwrap()
                .contains("preferredModel")
        );
        assert!(
            !serde_json::to_string(&intent)
                .unwrap()
                .contains("You review")
        );
    }

    #[test]
    fn skill_reference_round_trips_without_a_prompt() {
        assert_eq!(
            parse_skill_reference("native-role:opencode/reviewer"),
            Some(("opencode", "reviewer"))
        );
        assert_eq!(
            skill_reference("opencode", "reviewer"),
            "native-role:opencode/reviewer"
        );
    }
}
