//! Bounded, fail-closed native CLI admission and execution.

use anyhow::Result;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

mod adapter;
mod agent_conversation;
mod agent_hub;
mod agent_usage;
mod autostart;
mod client_conversation;
mod client_update;
mod collaboration;
mod conversation_surface;
mod extension_host;
mod full_backup;
mod gateway;
mod llm_gateway;
mod mcp;
mod mobile;
mod mobile_peer;
mod model_registry;
pub mod native_rpc;
mod opencode_serve;
mod package;
mod project;
mod provider_quota;
mod resource_usage;
mod secure_mesh;
mod skill;
mod snapshots;
mod state;
mod strategy;
mod subagents;
mod targets;

const MAX_CLI_ARGUMENT_COUNT: usize = 4_096;
const MAX_CLI_ARGUMENT_BYTES: usize = 2 * 1024 * 1024;

const ADMISSION_STAGE: &str = "cli/admission";
const ADMISSION_COMPONENT: &str = "native_cli";

#[derive(Debug, PartialEq, Clone)]
pub enum CliExecution {
    Usage,
    Json(Value),
    Streamed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredArgumentKind {
    Json,
    Text,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCardinality {
    Exact,
    Options,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionArity {
    Boolean,
    Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionConstraintKind {
    AtLeastOne,
    ConditionalRequired,
    MutuallyExclusive,
    OneOf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RequiredArgumentSpec {
    name: &'static str,
    kind: RequiredArgumentKind,
}

impl RequiredArgumentSpec {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn kind(&self) -> RequiredArgumentKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionSpec {
    name: &'static str,
    arity: OptionArity,
    repeatable: bool,
    value_kind: RequiredArgumentKind,
    required: bool,
}

impl OptionSpec {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn arity(&self) -> OptionArity {
        self.arity
    }

    pub fn repeatable(&self) -> bool {
        self.repeatable
    }

    pub fn value_kind(&self) -> RequiredArgumentKind {
        self.value_kind
    }

    pub fn required(&self) -> bool {
        self.required
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionConstraintSpec {
    kind: OptionConstraintKind,
    members: &'static [&'static str],
    condition_option: Option<&'static str>,
    condition_value: Option<&'static str>,
    required_option: Option<&'static str>,
}

impl OptionConstraintSpec {
    pub fn kind(&self) -> OptionConstraintKind {
        self.kind
    }

    pub fn members(&self) -> &'static [&'static str] {
        self.members
    }

    pub fn condition_option(&self) -> Option<&'static str> {
        self.condition_option
    }

    pub fn condition_value(&self) -> Option<&'static str> {
        self.condition_value
    }

    pub fn required_option(&self) -> Option<&'static str> {
        self.required_option
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliCommandSchema {
    #[serde(skip)]
    source_module: &'static str,
    #[serde(skip)]
    handler_name: &'static str,
    path: &'static [&'static str],
    required_positionals: &'static [RequiredArgumentSpec],
    options: &'static [OptionSpec],
    constraints: &'static [OptionConstraintSpec],
    cardinality: CommandCardinality,
    help: &'static str,
}

impl CliCommandSchema {
    pub fn source_module(&self) -> &'static str {
        self.source_module
    }

    pub fn handler_name(&self) -> &'static str {
        self.handler_name
    }

    pub fn path(&self) -> &'static [&'static str] {
        self.path
    }

    pub fn required_positionals(&self) -> &'static [RequiredArgumentSpec] {
        self.required_positionals
    }

    pub fn options(&self) -> &'static [OptionSpec] {
        self.options
    }

    pub fn constraints(&self) -> &'static [OptionConstraintSpec] {
        self.constraints
    }

    pub fn cardinality(&self) -> CommandCardinality {
        self.cardinality
    }

    pub fn help(&self) -> &'static str {
        self.help
    }
}

#[derive(Clone, Copy)]
pub struct CliCommandError {
    code: &'static str,
    stage: &'static str,
    component: &'static str,
    retryable: bool,
    recovery: &'static str,
}

const EXECUTION_STAGE: &str = "cli/execution";

impl CliCommandError {
    fn from_admission(code: &'static str, recovery: &'static str) -> Self {
        Self {
            code,
            stage: ADMISSION_STAGE,
            component: ADMISSION_COMPONENT,
            retryable: false,
            recovery,
        }
    }

    fn from_handler(code: &'static str, recovery: &'static str) -> Self {
        Self {
            code,
            stage: EXECUTION_STAGE,
            component: ADMISSION_COMPONENT,
            retryable: false,
            recovery,
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn component(&self) -> &'static str {
        self.component
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    pub fn recovery(&self) -> &'static str {
        self.recovery
    }
}

impl fmt::Debug for CliCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CliCommandError")
            .field("code", &self.code)
            .field("stage", &self.stage)
            .field("component", &self.component)
            .field("retryable", &self.retryable)
            .field("recovery", &self.recovery)
            .finish()
    }
}

impl fmt::Display for CliCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "native CLI request rejected ({})", self.code)
    }
}

impl std::error::Error for CliCommandError {}

type CommandFn = fn(AdmittedCommand) -> Result<CliExecution>;

struct CommandSpec {
    source_module: &'static str,
    handler_name: &'static str,
    path: &'static [&'static str],
    required_positionals: &'static [RequiredArgumentSpec],
    options: &'static [OptionSpec],
    constraints: &'static [OptionConstraintSpec],
    cardinality: CommandCardinality,
    handler: CommandFn,
    help: &'static str,
}

struct CommandDef {
    source_module: &'static str,
    handler_name: &'static str,
    path: &'static [&'static str],
    required_positionals: &'static [RequiredArgumentSpec],
    options: &'static [OptionSpec],
    constraints: &'static [OptionConstraintSpec],
    cardinality: CommandCardinality,
    handler: CommandFn,
    help: &'static str,
}

impl CommandDef {
    fn schema(&self) -> CliCommandSchema {
        CliCommandSchema {
            source_module: self.source_module,
            handler_name: self.handler_name,
            path: self.path,
            required_positionals: self.required_positionals,
            options: self.options,
            constraints: self.constraints,
            cardinality: self.cardinality,
            help: self.help,
        }
    }
}

pub struct CommandTable {
    defs: Vec<CommandDef>,
}

impl CommandTable {
    pub fn new() -> Self {
        Self { defs: Vec::new() }
    }

    fn register_command(&mut self, spec: CommandSpec) {
        self.defs.push(CommandDef {
            source_module: spec.source_module,
            handler_name: spec.handler_name,
            path: spec.path,
            required_positionals: spec.required_positionals,
            options: spec.options,
            constraints: spec.constraints,
            cardinality: spec.cardinality,
            handler: spec.handler,
            help: spec.help,
        });
    }

    fn schemas(&self) -> Vec<CliCommandSchema> {
        self.defs.iter().map(CommandDef::schema).collect()
    }

    fn help_text(&self) -> Vec<String> {
        self.defs
            .iter()
            .map(|definition| {
                let path = definition.path.join(" ");
                let required = definition
                    .required_positionals
                    .iter()
                    .map(|argument| format!(" <{}>", argument.name))
                    .collect::<String>();
                let options = definition
                    .options
                    .iter()
                    .map(|option| {
                        let value = match option.arity {
                            OptionArity::Boolean => String::new(),
                            OptionArity::Value => match option.value_kind {
                                RequiredArgumentKind::Json => " <JSON>".to_owned(),
                                RequiredArgumentKind::Text => " <VALUE>".to_owned(),
                            },
                        };
                        let repeat = if option.repeatable { "..." } else { "" };
                        if option.required {
                            format!(" --{}{value}{repeat}", option.name)
                        } else {
                            format!(" [--{}{value}{repeat}]", option.name)
                        }
                    })
                    .collect::<String>();
                let help = if definition.help.is_empty() {
                    String::new()
                } else {
                    format!("  — {}", definition.help)
                };
                format!("  licoup {path}{required}{options}{help}")
            })
            .collect()
    }

    fn admit(&self, args: Vec<String>) -> Result<AdmittedCommand> {
        validate_cli_admission(&args)?;
        let mut known_root = false;
        let mut missing_path = false;
        let definition = self
            .defs
            .iter()
            .find(|definition| {
                known_root |= args
                    .first()
                    .is_some_and(|argument| definition.path.first() == Some(&argument.as_str()));
                missing_path |= args.len() < definition.path.len()
                    && args
                        .iter()
                        .zip(definition.path)
                        .all(|(argument, expected)| argument == expected);
                args.len() >= definition.path.len()
                    && args
                        .iter()
                        .take(definition.path.len())
                        .zip(definition.path)
                        .all(|(argument, expected)| argument == expected)
            })
            .ok_or_else(|| {
                if args.is_empty() {
                    admission_error("cli_command_missing", "use_cli_help")
                } else if missing_path {
                    admission_error("cli_required_argument_missing", "correct_command_arguments")
                } else if known_root {
                    admission_error("cli_operation_unsupported", "use_cli_help")
                } else {
                    admission_error("cli_command_unknown", "use_cli_help")
                }
            })?;
        let validated = validate_command_arguments(
            definition.path,
            definition.required_positionals,
            definition.cardinality,
            definition.options,
            definition.constraints,
            &args,
        )?;
        let ValidatedCommandArguments {
            required_text,
            mut required_json,
            option_flags,
            option_text,
            mut option_json,
            pending_json,
        } = validated;
        for pending in pending_json {
            let parsed = parse_json_arg(&pending.raw)?;
            if pending.option {
                option_json.insert(pending.name, parsed);
            } else {
                required_json.insert(pending.name, parsed);
            }
        }
        Ok(AdmittedCommand {
            handler: definition.handler,
            schema: definition.schema(),
            required_text,
            required_json,
            option_flags,
            option_text,
            option_json,
        })
    }
}

struct PendingJson {
    option: bool,
    name: &'static str,
    raw: String,
}

struct ValidatedCommandArguments {
    required_text: BTreeMap<&'static str, String>,
    required_json: BTreeMap<&'static str, Value>,
    option_flags: BTreeSet<&'static str>,
    option_text: BTreeMap<&'static str, String>,
    option_json: BTreeMap<&'static str, Value>,
    pending_json: Vec<PendingJson>,
}

#[derive(Debug)]
pub struct AdmittedCommand {
    handler: CommandFn,
    schema: CliCommandSchema,
    required_text: BTreeMap<&'static str, String>,
    required_json: BTreeMap<&'static str, Value>,
    option_flags: BTreeSet<&'static str>,
    option_text: BTreeMap<&'static str, String>,
    option_json: BTreeMap<&'static str, Value>,
}

impl AdmittedCommand {
    pub fn source_module(&self) -> &'static str {
        self.schema.source_module
    }

    pub fn handler_name(&self) -> &'static str {
        self.schema.handler_name
    }

    pub fn path(&self) -> &'static [&'static str] {
        self.schema.path
    }

    pub fn required_positionals(&self) -> Vec<&'static str> {
        self.schema
            .required_positionals
            .iter()
            .map(RequiredArgumentSpec::name)
            .collect()
    }

    pub fn cardinality(&self) -> CommandCardinality {
        self.schema.cardinality
    }

    pub fn option_specs(&self) -> &'static [OptionSpec] {
        self.schema.options
    }

    pub fn required_kind(&self, name: &str) -> RequiredArgumentKind {
        self.schema
            .required_positionals
            .iter()
            .find(|argument| argument.name == name)
            .map(RequiredArgumentSpec::kind)
            .unwrap_or(RequiredArgumentKind::Text)
    }

    pub fn required_text(&self, name: &str) -> &str {
        self.required_text
            .get(name)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn required_json(&self, name: &str) -> &Value {
        self.required_json.get(name).unwrap_or(&Value::Null)
    }

    pub fn option_flag(&self, name: &str) -> bool {
        self.option_flags.contains(name)
    }

    pub fn option_text(&self, name: &str) -> Option<&str> {
        self.option_text.get(name).map(String::as_str)
    }

    pub fn option_json(&self, name: &str) -> Option<&Value> {
        self.option_json.get(name)
    }

    pub fn take_option_json(&mut self, name: &str) -> Option<Value> {
        self.option_json.remove(name)
    }

    pub fn execute(self) -> Result<CliExecution> {
        (self.handler)(self)
    }
}

fn admission_error(code: &'static str, recovery: &'static str) -> CliCommandError {
    CliCommandError::from_admission(code, recovery)
}

/// Structured handler-side failure for an admissible command whose dispatch
/// route disagrees with the registered command table. This is an interior
/// inconsistency, never an assertion panic: the host-facing boundary always
/// returns a typed failure with a stable problem code.
pub(super) fn handler_error(code: &'static str, recovery: &'static str) -> CliCommandError {
    CliCommandError::from_handler(code, recovery)
}

fn validate_cli_admission(args: &[String]) -> Result<()> {
    if args.len() > MAX_CLI_ARGUMENT_COUNT {
        return Err(
            admission_error("cli_argument_count_exceeded", "reduce_command_arguments").into(),
        );
    }
    if args
        .iter()
        .any(|argument| argument.len() > MAX_CLI_ARGUMENT_BYTES)
    {
        return Err(
            admission_error("cli_argument_bytes_exceeded", "reduce_command_arguments").into(),
        );
    }
    if args.len() > 1
        && args
            .first()
            .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        return Err(admission_error("cli_argument_unexpected", "correct_command_arguments").into());
    }
    Ok(())
}

fn validate_command_arguments(
    path: &'static [&'static str],
    required_positionals: &'static [RequiredArgumentSpec],
    cardinality: CommandCardinality,
    options: &'static [OptionSpec],
    constraints: &'static [OptionConstraintSpec],
    args: &[String],
) -> Result<ValidatedCommandArguments> {
    let mut required_text = BTreeMap::new();
    let required_json = BTreeMap::new();
    let mut option_flags = BTreeSet::new();
    let mut option_text = BTreeMap::new();
    let option_json = BTreeMap::new();
    let mut pending_json = Vec::new();
    let mut cursor = path.len();

    for spec in required_positionals {
        let Some(raw) = args.get(cursor).filter(|value| !value.starts_with("--")) else {
            return Err(admission_error(
                "cli_required_argument_missing",
                "correct_command_arguments",
            )
            .into());
        };
        match spec.kind {
            RequiredArgumentKind::Text => {
                required_text.insert(spec.name, raw.clone());
            }
            RequiredArgumentKind::Json => pending_json.push(PendingJson {
                option: false,
                name: spec.name,
                raw: raw.clone(),
            }),
        }
        cursor += 1;
    }

    if cardinality == CommandCardinality::Exact {
        if cursor != args.len() {
            let code = if args[cursor].starts_with("--") {
                "cli_option_unknown"
            } else {
                "cli_argument_unexpected"
            };
            return Err(admission_error(code, "correct_command_arguments").into());
        }
    } else {
        while cursor < args.len() {
            let raw_name = args[cursor].strip_prefix("--").ok_or_else(|| {
                admission_error("cli_argument_unexpected", "correct_command_arguments")
            })?;
            let spec = options
                .iter()
                .find(|candidate| candidate.name == raw_name)
                .ok_or_else(|| {
                    admission_error("cli_option_unknown", "correct_command_arguments")
                })?;
            let already_present = option_flags.contains(spec.name)
                || option_text.contains_key(spec.name)
                || pending_json
                    .iter()
                    .any(|pending| pending.option && pending.name == spec.name);
            if already_present && !spec.repeatable {
                return Err(
                    admission_error("cli_option_duplicate", "correct_command_arguments").into(),
                );
            }
            cursor += 1;
            match spec.arity {
                OptionArity::Boolean => {
                    option_flags.insert(spec.name);
                }
                OptionArity::Value => {
                    let Some(raw) = args.get(cursor).filter(|value| !value.starts_with("--"))
                    else {
                        return Err(admission_error(
                            "cli_option_value_missing",
                            "correct_command_arguments",
                        )
                        .into());
                    };
                    match spec.value_kind {
                        RequiredArgumentKind::Text => {
                            option_text.insert(spec.name, raw.clone());
                        }
                        RequiredArgumentKind::Json => pending_json.push(PendingJson {
                            option: true,
                            name: spec.name,
                            raw: raw.clone(),
                        }),
                    }
                    cursor += 1;
                }
            }
        }
    }

    for spec in options.iter().filter(|spec| spec.required) {
        let present = option_flags.contains(spec.name)
            || option_text.contains_key(spec.name)
            || pending_json
                .iter()
                .any(|pending| pending.option && pending.name == spec.name);
        if !present {
            return Err(admission_error(
                "cli_required_option_missing",
                "correct_command_arguments",
            )
            .into());
        }
    }
    validate_option_constraints(constraints, &option_flags, &option_text, &pending_json)?;

    Ok(ValidatedCommandArguments {
        required_text,
        required_json,
        option_flags,
        option_text,
        option_json,
        pending_json,
    })
}

fn validate_option_constraints(
    constraints: &'static [OptionConstraintSpec],
    option_flags: &BTreeSet<&'static str>,
    option_text: &BTreeMap<&'static str, String>,
    pending_json: &[PendingJson],
) -> Result<()> {
    let present = |name: &str| {
        option_flags.contains(name)
            || option_text.contains_key(name)
            || pending_json
                .iter()
                .any(|pending| pending.option && pending.name == name)
    };
    for constraint in constraints {
        let member_count = constraint
            .members
            .iter()
            .filter(|member| present(member))
            .count();
        let valid = match constraint.kind {
            OptionConstraintKind::AtLeastOne => member_count >= 1,
            OptionConstraintKind::MutuallyExclusive => member_count <= 1,
            OptionConstraintKind::OneOf => member_count == 1,
            OptionConstraintKind::ConditionalRequired => {
                let condition_matches = constraint
                    .condition_option
                    .zip(constraint.condition_value)
                    .is_some_and(|(name, value)| {
                        option_text.get(name).map(String::as_str) == Some(value)
                    });
                !condition_matches || constraint.required_option.is_some_and(|name| present(name))
            }
        };
        if !valid {
            return Err(admission_error(
                "cli_option_constraint_violation",
                "correct_command_arguments",
            )
            .into());
        }
    }
    Ok(())
}

pub fn parse_json_arg(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(|_| {
        CliCommandError::from_admission("cli_json_invalid", "provide_valid_json").into()
    })
}

pub fn cli_command_schemas() -> Vec<CliCommandSchema> {
    build_command_table().schemas()
}

/// Human help and machine discovery are projections of the admission registry.
pub fn cli_command_help() -> Vec<String> {
    build_command_table().help_text()
}

fn handle_commands(_command: AdmittedCommand) -> Result<CliExecution> {
    use crate::contracts::conversation_protocol::{
        CONVERSATION_PROTOCOL_METHODS, CONVERSATION_PROTOCOL_VERSION,
    };
    Ok(CliExecution::Json(json!({
        "schema": "licoup.cli-catalog/v1",
        "commands": cli_command_schemas(),
        "rpc": {
            "protocol": CONVERSATION_PROTOCOL_VERSION,
            "methods": CONVERSATION_PROTOCOL_METHODS.as_slice(),
            "stream": ["rpc", "conversation"],
            "bridge": ["rpc", "stdio"],
            "call": ["rpc", "call"],
        },
    })))
}

pub fn admit_cli_command(args: Vec<String>) -> Result<AdmittedCommand> {
    build_command_table().admit(args)
}

pub fn execute_cli(args: Vec<String>) -> Result<CliExecution> {
    if matches!(
        args.as_slice(),
        [value] if matches!(value.as_str(), "help" | "--help" | "-h")
    ) {
        return Ok(CliExecution::Usage);
    }
    admit_cli_command(args)?.execute()
}

pub fn cli_params(args: &[String]) -> Value {
    let mut params = Map::new();
    let mut positionals = Vec::<Value>::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if let Some(raw_key) = argument.strip_prefix("--") {
            let key = cli_param_key(raw_key);
            if let Some(value) = args.get(index + 1).filter(|value| !value.starts_with("--")) {
                params.insert(key, json!(value));
                index += 2;
            } else {
                params.insert(key, json!(true));
                index += 1;
            }
        } else {
            positionals.push(json!(argument));
            index += 1;
        }
    }
    if !positionals.is_empty() {
        if !params.contains_key("target") {
            if let Some(target) = positionals.first().and_then(Value::as_str) {
                params.insert("target".to_string(), json!(target));
            }
        }
        params.insert("positionals".to_string(), Value::Array(positionals));
    }
    Value::Object(params)
}

pub(super) fn admitted_params(
    text_values: &[(&str, Option<&str>)],
    json_values: &[(&str, Option<&Value>)],
    flags: &[(&str, bool)],
) -> Value {
    let mut params = Map::new();
    for (key, value) in text_values {
        if let Some(value) = value {
            params.insert((*key).to_string(), json!(value));
        }
    }
    for (key, value) in json_values {
        if let Some(value) = value {
            params.insert((*key).to_string(), (*value).clone());
        }
    }
    for (key, value) in flags {
        if *value {
            params.insert((*key).to_string(), Value::Bool(true));
        }
    }
    Value::Object(params)
}

fn cli_param_key(raw: &str) -> String {
    let mut output = String::new();
    let mut uppercase_next = false;
    for character in raw.chars() {
        if character == '-' || character == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            output.extend(character.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(character);
        }
    }
    output
}

const fn update_text_option(name: &'static str) -> OptionSpec {
    OptionSpec {
        name,
        arity: OptionArity::Value,
        repeatable: false,
        value_kind: RequiredArgumentKind::Text,
        required: false,
    }
}

const UPDATE_CHECK_ROUTE_OPTIONS: &[OptionSpec] = &[
    update_text_option("target-release-track"),
    update_text_option("manifest-path"),
    update_text_option("public-keys-path"),
    update_text_option("revocation-path"),
    update_text_option("source-path"),
    update_text_option("source"),
    update_text_option("repo"),
    update_text_option("staging-root"),
    update_text_option("state-root"),
    update_text_option("execute"),
    update_text_option("install-root"),
    update_text_option("gui-pid"),
    update_text_option("wait-for-script"),
];

const UPDATE_ROUTE_OPTIONS: &[OptionSpec] = &[
    update_text_option("manifest-path"),
    update_text_option("public-keys-path"),
    update_text_option("revocation-path"),
    update_text_option("source-path"),
    update_text_option("source"),
    update_text_option("repo"),
    update_text_option("staging-root"),
    update_text_option("state-root"),
    update_text_option("execute"),
    update_text_option("install-root"),
    update_text_option("gui-pid"),
    update_text_option("wait-for-script"),
];

const UPDATE_APPLY_ROUTE_OPTIONS: &[OptionSpec] = &[
    update_text_option("manifest-path"),
    update_text_option("public-keys-path"),
    update_text_option("revocation-path"),
    update_text_option("source-path"),
    update_text_option("source"),
    update_text_option("repo"),
    update_text_option("staging-root"),
    update_text_option("state-root"),
    update_text_option("data-root"),
    update_text_option("execute"),
    update_text_option("install-root"),
    update_text_option("gui-pid"),
    update_text_option("wait-for-script"),
];

const UPDATE_ROUTE_CONSTRAINTS: &[OptionConstraintSpec] = &[
    OptionConstraintSpec {
        kind: OptionConstraintKind::MutuallyExclusive,
        members: &["source-path", "source"],
        condition_option: None,
        condition_value: None,
        required_option: None,
    },
    OptionConstraintSpec {
        kind: OptionConstraintKind::MutuallyExclusive,
        members: &["source-path", "repo"],
        condition_option: None,
        condition_value: None,
        required_option: None,
    },
];

fn build_command_table() -> CommandTable {
    let mut table = CommandTable::new();
    table.register_command(CommandSpec {
        source_module: "mod.rs",
        handler_name: "handle_commands",
        path: &["commands"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: handle_commands,
        help:
            "Discover every native CLI command, admitted option, and generated RPC method as JSON.",
    });
    table.register_command(CommandSpec {
        source_module: "native_rpc.rs",
        handler_name: "handle_rpc_call",
        path: &["rpc", "call"],
        required_positionals: &[RequiredArgumentSpec {
            name: "method",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: native_rpc::handle_rpc_call,
        help: "Call a generated native RPC method through the durable host; print every response frame as NDJSON.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_subagents_catalog",
        path: &["subagents", "catalog"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: subagents::handle_subagents_catalog,
        help: "Discover admitted callers and native Subagents operations.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_subagents_execute",
        path: &["subagents", "execute"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: subagents::handle_subagents_execute,
        help: "Execute a native Subagents operation with its admitted caller scope and protected-effect checks.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_mcp_start",
        path: &["mcp", "start"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "binary",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: subagents::handle_mcp_start,
        help: "Start the independent local Subagents MCP process.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_mcp_stop",
        path: &["mcp", "stop"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: subagents::handle_mcp_stop,
        help: "Stop the independent local Subagents MCP process.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_mcp_reload",
        path: &["mcp", "reload"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "binary",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: subagents::handle_mcp_reload,
        help: "Reload the independent local Subagents MCP process.",
    });
    table.register_command(CommandSpec {
        source_module: "subagents.rs",
        handler_name: "handle_mcp_status",
        path: &["mcp", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: subagents::handle_mcp_status,
        help: "Read the independent local Subagents MCP process status.",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_catalog",
        path: &["adapter", "catalog"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: adapter::handle_catalog,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_antigravity_status",
        path: &["adapter", "antigravity", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: adapter::handle_antigravity_status,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_antigravity_install",
        path: &["adapter", "antigravity", "install"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: adapter::handle_antigravity_install,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_antigravity_uninstall",
        path: &["adapter", "antigravity", "uninstall"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: adapter::handle_antigravity_uninstall,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_antigravity_authorize",
        path: &["adapter", "antigravity", "authorize"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "binary-path",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: adapter::handle_antigravity_authorize,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_subagent_mcp_status",
        path: &["adapter", "subagent-mcp", "status"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "mcp-binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "config-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: adapter::handle_subagent_mcp_status,
        help: "Probe Subagent MCP readiness for a main agent without silent install. An optional --config-path must canonicalize to the provider's reviewed config candidate.",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_subagent_mcp_plan",
        path: &["adapter", "subagent-mcp", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "mcp-binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "config-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: adapter::handle_subagent_mcp_plan,
        help: "Plan a digest-confirmed Subagent MCP install for a supported main agent. An optional --config-path must canonicalize to the provider's reviewed config candidate and is bound into the approval digest.",
    });
    table.register_command(CommandSpec {
        source_module: "adapter.rs",
        handler_name: "handle_subagent_mcp_install",
        path: &["adapter", "subagent-mcp", "install"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "mcp-binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "config-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: adapter::handle_subagent_mcp_install,
        help: "Install Subagent MCP for a supported main agent after digest confirmation. An optional --config-path must canonicalize to the provider's reviewed config candidate and is bound into the approval digest.",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "open"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "send"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "steer"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "cancel"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "capabilities"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "stream"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agent_conversation",
        path: &["agent", "conversation", "cleanup"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agent_conversation,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_conversation.rs",
        handler_name: "handle_conversation_execute",
        path: &["conversation", "execute"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "require-running-host",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: client_conversation::handle_conversation_execute,
        help: "Execute a canonical Conversation action.",
    });
    table.register_command(CommandSpec {
        source_module: "conversation_surface.rs",
        handler_name: "handle_conversation_list",
        path: &["conversation", "list"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "include-archived",
            arity: OptionArity::Boolean,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: conversation_surface::handle_conversation_list,
        help: "List canonical conversations through the durable host. --include-archived also returns archived conversations.",
    });
    table.register_command(CommandSpec {
        source_module: "conversation_surface.rs",
        handler_name: "handle_conversation_get",
        path: &["conversation", "get"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "conversation-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: conversation_surface::handle_conversation_get,
        help: "Read one canonical conversation through the durable host.",
    });
    table.register_command(CommandSpec {
        source_module: "strategy.rs",
        handler_name: "handle_strategy_execute",
        path: &["strategy", "execute"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: strategy::handle_strategy_execute,
        help: "Execute an Adaptive Flywheel strategy action.",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agents_pair",
        path: &["agents", "pair", "request"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agents_pair,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agents_pair",
        path: &["agents", "pair", "approve"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agents_pair,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agents_pair",
        path: &["agents", "pair", "revoke"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agents_pair,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_conversation.rs",
        handler_name: "handle_agents_pair",
        path: &["agents", "pair", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_conversation::handle_agents_pair,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_hub.rs",
        handler_name: "handle_catalog",
        path: &["agent-hub", "catalog"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_hub::handle_catalog,
        help: "Project Agent Hub cards from the AgentCatalog; optional agent-id runs one live local lookup",
    });
    table.register_command(CommandSpec {
        source_module: "agent_hub.rs",
        handler_name: "handle_plan",
        path: &["agent-hub", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "operation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_hub::handle_plan,
        help: "Plan one Agent Hub install, update, or uninstall",
    });
    table.register_command(CommandSpec {
        source_module: "agent_hub.rs",
        handler_name: "handle_apply",
        path: &["agent-hub", "apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "operation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "cancel",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_hub::handle_apply,
        help: "Apply one confirmed Agent Hub plan",
    });
    table.register_command(CommandSpec {
        source_module: "extension_host.rs",
        handler_name: "handle_serve",
        path: &["extension-host", "serve"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[
            OptionSpec {
                name: "await-ms",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "cancel-after-ms",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "capability",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "mode",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "request",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: extension_host::handle_serve,
        help:
            "Run one agent-execution call through the extension host composed over a managed root",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_catalog",
        path: &["package", "catalog"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_catalog,
        help: "List what this client holds, reconciling an interrupted install first",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_install_plan",
        path: &["package", "install-plan"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[OptionSpec {
            name: "archive",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_install_plan,
        help: "Plan one install from a local archive without installing it",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_install_confirm",
        path: &["package", "install-confirm"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "archive",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "plan",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_install_confirm,
        help: "Confirm a reviewed install plan and issue the bound confirmation",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_install_apply",
        path: &["package", "install-apply"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "archive",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_install_apply,
        help: "Apply one confirmed install from a local archive",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_import",
        path: &["package", "import"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[OptionSpec {
            name: "archive",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_import,
        help: "Install an archive the user imported locally, with no directory and no network",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_enable",
        path: &["package", "enable"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_enable,
        help: "Switch one installed package version on without starting it",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_disable",
        path: &["package", "disable"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_disable,
        help: "Switch one installed package version off, keeping its bytes and user data",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_uninstall_preview",
        path: &["package", "uninstall-preview"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_uninstall_preview,
        help: "Show what removing one installed version would touch",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_uninstall_drain",
        path: &["package", "uninstall-drain"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[
            OptionSpec {
                name: "remaining",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "dependents",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "instances",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_uninstall_drain,
        help: "Withdraw admission and drain every instance of one installed version",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_uninstall_collect",
        path: &["package", "uninstall-collect"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[OptionSpec {
            name: "registration-inputs",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_uninstall_collect,
        help: "Release the registrations a drained version created, then reclaim its bytes",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_recover",
        path: &["package", "recover"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_recover,
        help: "Reconcile an interrupted install and report what recovery found",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_update_preview",
        path: &["package", "update-preview"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[OptionSpec {
            name: "archive",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_update_preview,
        help: "Show what replacing an installed version would do, without doing it",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_update_apply",
        path: &["package", "update-apply"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[
            OptionSpec {
                name: "archive",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_update_apply,
        help: "Replace an installed version; refused until the native idle guard exists",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_activate",
        path: &["package", "activate"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: package::handle_activate,
        help: "Serve an installed generation's typed resources and publish the mount plan the client reads",
    });
    table.register_command(CommandSpec {
        source_module: "package.rs",
        handler_name: "handle_resource_select",
        path: &["package", "resource-select"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "data-root",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "package-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "version",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[
            OptionSpec {
                name: "kind",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "resource",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: package::handle_resource_select,
        help: "Serve one declared resource of an installed release as the plan's binding for its kind",
    });
    table.register_command(CommandSpec {
        source_module: "agent_usage.rs",
        handler_name: "handle_agent_usage_scan",
        path: &["agent-usage", "scan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "history-days",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "timezone-offset-minutes",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "timezone-transitions-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "force-refresh",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_usage::handle_agent_usage_scan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "agent_usage.rs",
        handler_name: "handle_agent_usage_report",
        path: &["agent-usage", "report"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: agent_usage::handle_agent_usage_report,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "provider_quota.rs",
        handler_name: "handle_provider_quota_snapshot",
        path: &["provider-quota", "snapshot"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "force-refresh",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: provider_quota::handle_provider_quota_snapshot,
        help: "Provider-quota snapshots for agents with a quota source",
    });
    table.register_command(CommandSpec {
        source_module: "resource_usage.rs",
        handler_name: "handle_resource_usage_scan",
        path: &["resource-usage", "scan"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "state-root",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: resource_usage::handle_resource_usage_scan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_update.rs",
        handler_name: "handle_update",
        path: &["update", "status"],
        required_positionals: &[],
        options: UPDATE_CHECK_ROUTE_OPTIONS,
        constraints: UPDATE_ROUTE_CONSTRAINTS,
        cardinality: CommandCardinality::Options,
        handler: client_update::handle_update,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_update.rs",
        handler_name: "handle_update",
        path: &["update", "check"],
        required_positionals: &[],
        options: UPDATE_CHECK_ROUTE_OPTIONS,
        constraints: UPDATE_ROUTE_CONSTRAINTS,
        cardinality: CommandCardinality::Options,
        handler: client_update::handle_update,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_update.rs",
        handler_name: "handle_update",
        path: &["update", "download"],
        required_positionals: &[],
        options: UPDATE_ROUTE_OPTIONS,
        constraints: UPDATE_ROUTE_CONSTRAINTS,
        cardinality: CommandCardinality::Options,
        handler: client_update::handle_update,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_update.rs",
        handler_name: "handle_update",
        path: &["update", "verify"],
        required_positionals: &[],
        options: UPDATE_ROUTE_OPTIONS,
        constraints: UPDATE_ROUTE_CONSTRAINTS,
        cardinality: CommandCardinality::Options,
        handler: client_update::handle_update,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "client_update.rs",
        handler_name: "handle_update",
        path: &["update", "apply"],
        required_positionals: &[],
        options: UPDATE_APPLY_ROUTE_OPTIONS,
        constraints: UPDATE_ROUTE_CONSTRAINTS,
        cardinality: CommandCardinality::Options,
        handler: client_update::handle_update,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_status",
        path: &["collaboration", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_status,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_enable",
        path: &["collaboration", "enable"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_enable,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_install_plan",
        path: &["collaboration", "install", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "github-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["github-url", "plan-id"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_install_plan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_install_apply",
        path: &["collaboration", "install", "apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "github-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["github-url", "plan-id"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_install_apply,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_install_cancel",
        path: &["collaboration", "install", "cancel"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "github-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["github-url", "plan-id"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_install_cancel,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_workflow_catalog",
        path: &["collaboration", "workflow", "catalog"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_workflow_catalog,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_deployment_plan",
        path: &["collaboration", "workflow", "local-deployment", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "selected-feature-ids",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "destination",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "destination-confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-plan-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-package-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_local_deployment_plan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_deployment_apply",
        path: &["collaboration", "workflow", "local-deployment", "apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "selected-feature-ids",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "destination",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "destination-confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-plan-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-package-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_local_deployment_apply,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_mcp_install_plan",
        path: &["collaboration", "workflow", "mcp-install", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "selected-plugin-ids",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "agent-destinations",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-plan-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-package-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_mcp_install_plan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_mcp_install_apply",
        path: &["collaboration", "workflow", "mcp-install", "apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "selected-plugin-ids",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "agent-destinations",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-plan-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "expected-package-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_mcp_install_apply,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_workflow_cancel",
        path: &["collaboration", "workflow", "cancel"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "plan-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "expected-plan-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "expected-package-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_workflow_cancel,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_server_status",
        path: &["collaboration", "local-server", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_local_server_status,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_server_start",
        path: &["collaboration", "local-server", "start"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "deployment-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_local_server_start,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_server_stop",
        path: &["collaboration", "local-server", "stop"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "deployment-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_local_server_stop,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_local_server_uninstall",
        path: &["collaboration", "local-server", "uninstall"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "request-origin",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "deployment-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "expected-assembly-manifest-digest-sha256",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: collaboration::handle_local_server_uninstall,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_disable",
        path: &["collaboration", "disable"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_disable,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "collaboration.rs",
        handler_name: "handle_cleanup",
        path: &["collaboration", "cleanup"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: collaboration::handle_cleanup,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mcp.rs",
        handler_name: "handle_preview",
        path: &["mcp", "http", "preview"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mcp::handle_preview,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mcp.rs",
        handler_name: "handle_execute",
        path: &["mcp", "http", "execute"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mcp::handle_execute,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "config", "get"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "authorize",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "hydrate-secrets",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "config", "set"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "station-base-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "relay-enabled",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "pc-client-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "pc-client-name",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "pairing-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "paired",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "reset-pairing",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "pairing", "create"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "pairing-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "pairing", "claim"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "pairing-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "pairing", "status"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "pairing-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "pairing", "revoke"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "pairing-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "pc", "check-in"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "poll"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "sync"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "allow-interaction",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "create"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "create-secure"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "client-intent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "command-kind",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "target-agent-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "workspace-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "body",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "station-base-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "allow-interaction",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "result"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "result-secure"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "idempotency-key",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "acknowledge-receipt-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "commands", "result-replay-proof"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "command-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "e2ee", "secret-store-cleanup"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "cleanup-confirmation",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "e2ee", "secret-store-cleanup-inventory"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "mobile.rs",
        handler_name: "handle_mobile_relay",
        path: &["mobile", "relay", "e2ee", "secret-store-self-test"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: mobile::handle_mobile_relay,
        help: "",
    });
    // The cross-device entry's own routes. They are the native surface through
    // which a protocol line is admitted, an inbound packet reaches the device
    // edge, and a verified device is bound to a membership this host owns.
    table.register_command(CommandSpec {
        source_module: "mobile_peer.rs",
        handler_name: "handle_mobile_peer",
        path: &["mobile", "relay", "peer", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: mobile_peer::handle_mobile_peer,
        help: "Read the cross-device entry's own state: installation, attached device edge, recorded verifications, and peer bindings.",
    });
    table.register_command(CommandSpec {
        source_module: "mobile_peer.rs",
        handler_name: "handle_mobile_peer",
        path: &["mobile", "relay", "peer", "protocol-line"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "authority-file",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile_peer::handle_mobile_peer,
        help: "Admit the caller's LicoArc authority artifact through the pinned SDK and report the verified Protocol Line it accepted.",
    });
    table.register_command(CommandSpec {
        source_module: "mobile_peer.rs",
        handler_name: "handle_mobile_peer",
        path: &["mobile", "relay", "peer", "record"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile_peer::handle_mobile_peer,
        help: "Verify one inbound protected packet through the attached device edge and admit the unit it committed.",
    });
    table.register_command(CommandSpec {
        source_module: "mobile_peer.rs",
        handler_name: "handle_mobile_peer",
        path: &["mobile", "relay", "peer", "bind"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile_peer::handle_mobile_peer,
        help: "Bind one verified device digest to an existing active membership of an existing conversation.",
    });
    table.register_command(CommandSpec {
        source_module: "mobile_peer.rs",
        handler_name: "handle_mobile_peer",
        path: &["mobile", "relay", "peer", "revoke"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: mobile_peer::handle_mobile_peer,
        help: "Remove one peer binding; revocation is expressed by answering no binding again.",
    });
    table.register_command(CommandSpec {
        source_module: "opencode_serve.rs",
        handler_name: "handle_opencode_serve",
        path: &["opencode-serve", "ensure"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "executable",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "attach-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: opencode_serve::handle_opencode_serve,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "opencode_serve.rs",
        handler_name: "handle_opencode_serve",
        path: &["opencode-serve", "start"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "executable",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "attach-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: opencode_serve::handle_opencode_serve,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "opencode_serve.rs",
        handler_name: "handle_opencode_serve",
        path: &["opencode-serve", "stop"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "executable",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "attach-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: opencode_serve::handle_opencode_serve,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "opencode_serve.rs",
        handler_name: "handle_opencode_serve",
        path: &["opencode-serve", "restart"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "executable",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "attach-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: opencode_serve::handle_opencode_serve,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "opencode_serve.rs",
        handler_name: "handle_opencode_serve",
        path: &["opencode-serve", "status"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "executable",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "attach-url",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: opencode_serve::handle_opencode_serve,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "status"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "payload",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "context",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "ledger-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "envelope", "validate"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "payload",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "context",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "ledger-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "command", "policy"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "payload",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "context",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "ledger-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "command", "evaluate"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "payload",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "context",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "ledger-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "command", "execute"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "payload",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "context",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "ledger-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "device-trust", "evaluate"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "identity",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "previous-identity",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "trust-state",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "Evaluate device trust; caller state is advisory and cannot authorize.",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "file", "route"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "manifest",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "file", "receive-destination"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "manifest",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "approved-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "conflict-policy",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "file", "receive-confirmation"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "manifest",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "approved-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "user-confirmed",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "approval", "request"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "pending-operation-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "decision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "approval", "fanout"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "pending-operation-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "decision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "approval", "respond"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "pending-operation-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "decision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "responding-endpoint-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "response-nonce",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "approval", "inbox"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "pending-operation-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "decision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "secure_mesh.rs",
        handler_name: "handle_secure_mesh",
        path: &["secure-mesh", "approval", "adapter-capability"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "pending-operation-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "decision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: secure_mesh::handle_secure_mesh,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_list",
        path: &["skill", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "skill-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_list,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_get",
        path: &["skill", "get"],
        required_positionals: &[RequiredArgumentSpec {
            name: "skill-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "skill-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_get,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_delete_plan",
        path: &["skill", "delete", "plan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "skill",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_delete_plan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_delete_apply",
        path: &["skill", "delete", "apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "skill",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_delete_apply,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_visibility",
        path: &["skill", "visibility", "set"],
        required_positionals: &[RequiredArgumentSpec {
            name: "skill-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "hidden",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_visibility,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_usage_report",
        path: &["skill", "usage", "report"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "skill",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "days",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "from",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "to",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["days", "from"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_usage_report,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "skill.rs",
        handler_name: "handle_skill_usage_scan",
        path: &["skill", "usage", "scan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "history-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "home-dir",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "force-refresh",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: skill::handle_skill_usage_scan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_list",
        path: &["snapshots", "list"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "target",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_list,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_restore",
        path: &["snapshots", "restore"],
        required_positionals: &[RequiredArgumentSpec {
            name: "snapshot-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: snapshots::handle_snapshots_restore,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_collect",
        path: &["snapshots", "collect"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "topic",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_collect,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_root",
        path: &["snapshots", "root", "get"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "path",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_root,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_root",
        path: &["snapshots", "root", "set"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "path",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_root,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_profiles",
        path: &["snapshots", "profiles", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "profile-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "profile-file",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["profile", "profile-json", "profile-file"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_profiles,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_profiles",
        path: &["snapshots", "profiles", "get"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "profile-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "profile-file",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["profile", "profile-json", "profile-file"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_profiles,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_profiles",
        path: &["snapshots", "profiles", "import"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "profile-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
            OptionSpec {
                name: "profile-file",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::MutuallyExclusive,
            members: &["profile", "profile-json", "profile-file"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_profiles,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "collect"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "keywords",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "trigger",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "run"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "trigger",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "verify"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "trigger",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "collection-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::OneOf,
            members: &["profile", "collection-path"],
            condition_option: None,
            condition_value: None,
            required_option: None,
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "report"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "profile",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "trigger",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "preview"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "selection-mode",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "query",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::ConditionalRequired,
            members: &[],
            condition_option: Some("selection-mode"),
            condition_value: Some("exact-keyword"),
            required_option: Some("query"),
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "create"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "selection-mode",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "query",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "plan-binding",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[OptionConstraintSpec {
            kind: OptionConstraintKind::ConditionalRequired,
            members: &[],
            condition_option: Some("selection-mode"),
            condition_value: Some("exact-keyword"),
            required_option: Some("query"),
        }],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "status"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "job-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "job-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "once",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "events"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "job-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "cancel"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "job-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_archive",
        path: &["snapshots", "archive", "jobs", "drain"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "job-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "once",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_archive,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_snapshots_collections",
        path: &["snapshots", "collections", "list"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "snapshot-root",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_snapshots_collections,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_conversations",
        path: &["conversations", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "offset",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "session-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "text",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_conversations,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_conversations",
        path: &["conversations", "stream"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "offset",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "session-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "text",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_conversations,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_conversations",
        path: &["conversations", "append"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "offset",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "session-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "text",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_conversations,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "snapshots.rs",
        handler_name: "handle_conversations",
        path: &["conversations", "delete"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "agent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "offset",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "session-id",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "text",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: snapshots::handle_conversations,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "state.rs",
        handler_name: "handle_state_get",
        path: &["state", "get"],
        required_positionals: &[RequiredArgumentSpec {
            name: "collection",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: state::handle_state_get,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "state.rs",
        handler_name: "handle_state_set",
        path: &["state", "set"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "collection",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "payload",
                kind: RequiredArgumentKind::Json,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: state::handle_state_set,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "state.rs",
        handler_name: "handle_state_admit",
        path: &["state", "admit"],
        required_positionals: &[RequiredArgumentSpec {
            name: "data-root",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: state::handle_state_admit,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "state.rs",
        handler_name: "handle_activity_list",
        path: &["activity", "list"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "type",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "limit",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: state::handle_activity_list,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "model_registry.rs",
        handler_name: "handle_read",
        path: &["model-registry", "read"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: model_registry::handle_read,
        help: "Read the cached public model registry without network access",
    });
    table.register_command(CommandSpec {
        source_module: "model_registry.rs",
        handler_name: "handle_refresh",
        path: &["model-registry", "refresh"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: model_registry::handle_refresh,
        help: "Refresh public model identities and retain the previous catalog on failure",
    });
    table.register_command(CommandSpec {
        source_module: "targets.rs",
        handler_name: "handle_targets_catalog",
        path: &["targets", "catalog"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: targets::handle_targets_catalog,
        help: "List current target catalog IDs without target discovery",
    });
    table.register_command(CommandSpec {
        source_module: "targets.rs",
        handler_name: "handle_targets_scan",
        path: &["targets", "scan"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "include-accessible-environments",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "include-history-model-catalog",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "enable-agent-cli-model-lookup",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: targets::handle_targets_scan,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "targets.rs",
        handler_name: "handle_targets_add",
        path: &["targets", "add"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "target",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "config-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "binary-path",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "history-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: targets::handle_targets_add,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "targets.rs",
        handler_name: "handle_targets_inspect",
        path: &["targets", "inspect"],
        required_positionals: &[RequiredArgumentSpec {
            name: "target",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "include-accessible-environments",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "enable-agent-cli-model-lookup",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: targets::handle_targets_inspect,
        help: "",
    });
    table.register_command(CommandSpec {
        source_module: "targets.rs",
        handler_name: "handle_targets_model_selection",
        path: &["targets", "model-selection"],
        required_positionals: &[RequiredArgumentSpec {
            name: "target",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "state-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "include-accessible-environments",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "enable-agent-cli-model-lookup",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: targets::handle_targets_model_selection,
        help: "Compose the per-Agent model selection matrix: support, availability, credentials and per-scope execution",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_status",
        path: &["llm-gateway", "credentials", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_status,
        help: "System-keyring availability and lease options",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_list",
        path: &["llm-gateway", "credentials", "list"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_list,
        help: "List non-secret model API key metadata without authorization",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_migrate",
        path: &["llm-gateway", "credentials", "migrate"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_migrate,
        help: "Migrate legacy model API keys with native Keychain approval",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_authorize",
        path: &["llm-gateway", "credentials", "authorize"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "credential-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_authorize,
        help: "Authorize loading model API keys into the native app session",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_clear",
        path: &["llm-gateway", "credentials", "clear"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "credential-id",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_clear,
        help: "Clear model API keys from the native app session without deleting vault entries",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_create",
        path: &["llm-gateway", "credentials", "create"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_create,
        help: "Save a private stdin API key after system authorization",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_delete",
        path: &["llm-gateway", "credentials", "delete"],
        required_positionals: &[RequiredArgumentSpec {
            name: "credential-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_delete,
        help: "Permanently delete one Keychain item",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_lease",
        path: &["llm-gateway", "credentials", "lease"],
        required_positionals: &[RequiredArgumentSpec {
            name: "days",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_lease,
        help: "Set process lease to 7/30/60/90/180/365 days",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_update",
        path: &["llm-gateway", "credentials", "update"],
        required_positionals: &[RequiredArgumentSpec {
            name: "credential-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_update,
        help: "Rename a credential or extend its validity period",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_agent_plan",
        path: &["llm-gateway", "agent-config", "plan"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "agent",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "config-root",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_agent_plan,
        help: "Preview a secret-free Codex, Claude Code, OpenCode, or Pi Gateway configuration",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_agent_apply",
        path: &["llm-gateway", "agent-config", "apply"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "agent",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "config-root",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "confirmation",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "confirmed",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_agent_apply,
        help: "Apply a confirmed secret-free Gateway profile",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_status",
        path: &["llm-gateway", "service", "status"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_service_status,
        help: "Report the local LLM Gateway service state",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_usage",
        path: &["llm-gateway", "service", "usage"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_service_usage,
        help: "Report request counts observed by the local LLM Gateway",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_initialize",
        path: &["llm-gateway", "service", "initialize"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_service_initialize,
        help: "Initialize the local LLM Gateway service without authorization",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_start",
        path: &["llm-gateway", "service", "start"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_service_start,
        help: "Start the managed local LLM Gateway service",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_stop",
        path: &["llm-gateway", "service", "stop"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_service_stop,
        help: "Stop the managed local LLM Gateway service",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_autostart_status",
        path: &["llm-gateway", "service", "autostart-status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_service_autostart_status,
        help: "Report whether the LLM Gateway starts at user login",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_autostart_enable",
        path: &["llm-gateway", "service", "autostart-enable"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: llm_gateway::handle_service_autostart_enable,
        help: "Install login autostart for the local LLM Gateway (starts alone, no credentials)",
    });
    table.register_command(CommandSpec {
        source_module: "llm_gateway.rs",
        handler_name: "handle_service_autostart_disable",
        path: &["llm-gateway", "service", "autostart-disable"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: llm_gateway::handle_service_autostart_disable,
        help: "Remove login autostart for the local LLM Gateway",
    });
    table.register_command(CommandSpec {
        source_module: "autostart.rs",
        handler_name: "handle_status",
        path: &["autostart", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: autostart::handle_status,
        help: "Report desktop, Gateway, and local MCP login autostart state",
    });
    table.register_command(CommandSpec {
        source_module: "autostart.rs",
        handler_name: "handle_set",
        path: &["autostart", "set"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "component",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "enabled",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
            OptionSpec {
                name: "silent",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "port",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: autostart::handle_set,
        help: "Enable or disable login autostart for desktop, gateway, or mcp",
    });
    table.register_command(CommandSpec {
        source_module: "autostart.rs",
        handler_name: "handle_prepare_mcp",
        path: &["autostart", "prepare-mcp"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: autostart::handle_prepare_mcp,
        help: "Login oneshot: verify packaged local MCP binaries without silent install",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_help",
        path: &["gateway", "help"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_help,
        help: "Describe the two-layer Gateway Runtime CLI",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_client_token",
        path: &["gateway", "client-token"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "agent",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_client_token,
        help: "Print the private local Gateway client token for an admitted agent",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_service_status",
        path: &["gateway", "service", "status"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_service_status,
        help: "Report Gateway Runtime state (LLM + channels)",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_service_start",
        path: &["gateway", "service", "start"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_service_start,
        help: "Start the unified Gateway Runtime process",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_service_stop",
        path: &["gateway", "service", "stop"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_service_stop,
        help: "Stop the unified Gateway Runtime process",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_service_initialize",
        path: &["gateway", "service", "initialize"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "port",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: false,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_service_initialize,
        help: "Initialize the Gateway Runtime without credential authorization",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_inventory_reload",
        path: &["gateway", "inventory", "reload"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_inventory_reload,
        help: "Hot-reload verified conversation readiness into the running Gateway",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_channel_status",
        path: &["gateway", "channel", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_channel_status,
        help: "Report Communication Channel layer status",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_credentials_status",
        path: &["gateway", "channel", "telegram", "credentials", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_credentials_status,
        help: "Report whether a Telegram bot token is configured",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_credentials_set",
        path: &["gateway", "channel", "telegram", "credentials", "set"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: gateway::handle_telegram_credentials_set,
        help: "Store a Telegram bot token from private stdin JSON",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_credentials_clear",
        path: &["gateway", "channel", "telegram", "credentials", "clear"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_credentials_clear,
        help: "Remove the stored Telegram bot token",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_pairing_list",
        path: &["gateway", "channel", "telegram", "pairing", "list"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_pairing_list,
        help: "List pending Telegram DM pairing codes",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_pairing_approve",
        path: &["gateway", "channel", "telegram", "pairing", "approve"],
        required_positionals: &[RequiredArgumentSpec {
            name: "code",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_pairing_approve,
        help: "Approve a Telegram DM pairing code",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_pairing_revoke",
        path: &["gateway", "channel", "telegram", "pairing", "revoke"],
        required_positionals: &[RequiredArgumentSpec {
            name: "chat-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_pairing_revoke,
        help: "Revoke Telegram DM access for a chat id",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_service_status",
        path: &["gateway", "channel", "telegram", "service", "status"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_service_status,
        help: "Report the Telegram channel package process state",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_service_start",
        path: &["gateway", "channel", "telegram", "service", "start"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_service_start,
        help: "Start the installed Telegram channel package process",
    });
    table.register_command(CommandSpec {
        source_module: "gateway.rs",
        handler_name: "handle_telegram_service_stop",
        path: &["gateway", "channel", "telegram", "service", "stop"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: gateway::handle_telegram_service_stop,
        help: "Stop the Telegram channel package process",
    });
    table.register_command(CommandSpec {
        source_module: "full_backup.rs",
        handler_name: "handle_backup_export",
        path: &["backup", "export"],
        required_positionals: &[RequiredArgumentSpec {
            name: "archive",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[
            OptionSpec {
                name: "data-root",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
            OptionSpec {
                name: "writers-stopped",
                arity: OptionArity::Boolean,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: false,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: full_backup::handle_backup_export,
        help: "Export the selected data home to one plaintext archive after stopped writers are declared",
    });
    table.register_command(CommandSpec {
        source_module: "full_backup.rs",
        handler_name: "handle_backup_import",
        path: &["backup", "import"],
        required_positionals: &[RequiredArgumentSpec {
            name: "archive",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[OptionSpec {
            name: "target-root",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Text,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: full_backup::handle_backup_import,
        help: "Restore one archive into an empty target home and rebase owner-managed references",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_register",
        path: &["project", "register"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: project::handle_project_register,
        help: "Register one explicitly declared authorized project identity and its plan identity",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_read",
        path: &["project", "read"],
        required_positionals: &[RequiredArgumentSpec {
            name: "project-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: project::handle_project_read,
        help: "Read one registered authorized project identity",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_list",
        path: &["project", "list"],
        required_positionals: &[],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: project::handle_project_list,
        help: "List the registered authorized project identities",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_import_preview",
        path: &["project", "import-preview"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: project::handle_project_import_preview,
        help: "Report what one canonical plan document would change, before it changes anything",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_import_apply",
        path: &["project", "import-apply"],
        required_positionals: &[],
        options: &[
            OptionSpec {
                name: "stdin-json",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Json,
                required: true,
            },
            OptionSpec {
                name: "expected-revision",
                arity: OptionArity::Value,
                repeatable: false,
                value_kind: RequiredArgumentKind::Text,
                required: true,
            },
        ],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: project::handle_project_import_apply,
        help: "Apply one canonical plan document over the source revision the caller previewed",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_dependency_declare",
        path: &["project", "dependency", "declare"],
        required_positionals: &[],
        options: &[OptionSpec {
            name: "stdin-json",
            arity: OptionArity::Value,
            repeatable: false,
            value_kind: RequiredArgumentKind::Json,
            required: true,
        }],
        constraints: &[],
        cardinality: CommandCardinality::Options,
        handler: project::handle_project_dependency_declare,
        help: "Declare the artifact input one authorized project's work item takes from a producer",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_dependency_list",
        path: &["project", "dependency", "list"],
        required_positionals: &[RequiredArgumentSpec {
            name: "project-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: project::handle_project_dependency_list,
        help: "List the artifact inputs one registered project declares, with their explicit state",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_dependency_unresolved",
        path: &["project", "dependency", "unresolved"],
        required_positionals: &[RequiredArgumentSpec {
            name: "project-id",
            kind: RequiredArgumentKind::Text,
        }],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: project::handle_project_dependency_unresolved,
        help: "Report the declared artifact inputs of one project whose result is not materialized",
    });
    table.register_command(CommandSpec {
        source_module: "project.rs",
        handler_name: "handle_project_dependency_blocked",
        path: &["project", "dependency", "blocked"],
        required_positionals: &[
            RequiredArgumentSpec {
                name: "project-id",
                kind: RequiredArgumentKind::Text,
            },
            RequiredArgumentSpec {
                name: "work-item-id",
                kind: RequiredArgumentKind::Text,
            },
        ],
        options: &[],
        constraints: &[],
        cardinality: CommandCardinality::Exact,
        handler: project::handle_project_dependency_blocked,
        help: "List the work items one blocked producer blocks, transitively and across projects",
    });
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host-facing boundary guard: malformed JSON admitted as an FFI command
    /// argument yields a structured typed failure (problem code preserved
    /// downcast as `CliCommandError`) and never unwinds the process; the
    /// boundary remains able to serve the next command.
    #[test]
    fn ffi_boundary_malformed_json_is_structured_failure_with_process_alive() {
        let malformed = execute_cli(vec![
            "agent".to_string(),
            "conversation".to_string(),
            "open".to_string(),
            "--stdin-json".to_string(),
            "{not-json:".to_string(),
        ]);
        let error = malformed.expect_err("malformed JSON must be rejected at the boundary");
        let command_error = error
            .downcast_ref::<CliCommandError>()
            .expect("boundary failure must be typed as CliCommandError");
        assert_eq!(command_error.code(), "cli_json_invalid");
        assert_eq!(command_error.stage(), "cli/admission");
        assert!(!command_error.retryable());

        // The same process boundary still serves a subsequent valid command.
        match execute_cli(vec!["adapter".to_string(), "catalog".to_string()]) {
            Ok(CliExecution::Json(value)) => assert_eq!(value["ok"], true),
            Ok(_) => assert!(
                false,
                "catalog command must return JSON after a rejected request"
            ),
            Err(error) => assert!(
                false,
                "catalog command must succeed after a rejected request: {error}"
            ),
        }
    }
}
