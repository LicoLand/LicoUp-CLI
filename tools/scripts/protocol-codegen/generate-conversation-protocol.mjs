import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const manifestPath = "schemas/conversation_protocol/manifest.json";
const maximumManifestBytes = 64 * 1024;
const maximumSchemaBytes = 512 * 1024;
const maximumOutputBytes = 2 * 1024 * 1024;
const maximumPathLength = 240;

class ContractError extends Error {}

function fail(message) {
  throw new ContractError(message);
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function readBoundedJson(relativePath, maximumBytes, label) {
  const absolutePath = path.join(repositoryRoot, relativePath);
  let stats;
  let source;
  try {
    stats = fs.statSync(absolutePath);
    if (!stats.isFile()) fail(`${label} is not a file: ${relativePath}`);
    if (stats.size > maximumBytes) fail(`${label} is too large: ${relativePath}`);
    source = fs.readFileSync(absolutePath, "utf8");
  } catch (error) {
    if (error instanceof ContractError) throw error;
    fail(`missing or unreadable ${label}: ${relativePath}`);
  }
  try {
    return JSON.parse(source);
  } catch {
    fail(`invalid JSON in ${label}: ${relativePath}`);
  }
}

function validatePath(value, label) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximumPathLength ||
    value.includes("\\") ||
    value.includes("\0") ||
    path.posix.isAbsolute(value) ||
    path.posix.normalize(value) !== value ||
    value === ".." ||
    value.startsWith("../")
  ) {
    fail(`unsafe or invalid ${label} path`);
  }
  return value;
}

function validateManifest() {
  const manifest = readBoundedJson(
    manifestPath,
    maximumManifestBytes,
    "protocol-codegen manifest",
  );
  if (!isRecord(manifest) || manifest.version !== 1) {
    fail("unsupported protocol-codegen manifest version");
  }
  if (!Array.isArray(manifest.families) || manifest.families.length === 0) {
    fail("protocol-codegen manifest must contain ordered families");
  }
  const ids = new Set();
  const schemas = new Set();
  const dartOutputs = new Set();
  const rustOutputs = new Set();
  for (let index = 0; index < manifest.families.length; index += 1) {
    const family = manifest.families[index];
    if (!isRecord(family)) fail(`invalid protocol family at index ${index}`);
    if (
      typeof family.id !== "string" ||
      !/^[a-z][a-z0-9_]{0,63}$/u.test(family.id)
    ) {
      fail(`invalid protocol family id at index ${index}`);
    }
    if (ids.has(family.id)) fail(`duplicate protocol family id at index ${index}`);
    ids.add(family.id);
    if (family.status !== "active") {
      fail(`invalid protocol family status at index ${index}`);
    }
    validatePath(family.schema, "schema");
    validatePath(family.dartOutput, "Dart output");
    if (family.rustOutput !== null) validatePath(family.rustOutput, "Rust output");
    for (const [value, seen, label] of [
      [family.schema, schemas, "schema"],
      [family.dartOutput, dartOutputs, "Dart output"],
      [family.rustOutput, rustOutputs, "Rust output"],
    ]) {
      if (value === null) continue;
      if (seen.has(value)) fail(`duplicate protocol ${label} path`);
      seen.add(value);
    }
  }
  return manifest;
}

function pascal(value) {
  return value
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join("");
}

function camel(value) {
  const asPascal = pascal(value);
  return asPascal[0].toLowerCase() + asPascal.slice(1);
}

function validateSchema(schema) {
  const methodNamePattern = /^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)*$/u;
  const kinds = new Set(["execute", "command", "stream", "shutdown"]);
  const lanes = new Set(["command", "conversation"]);
  if (
    !isRecord(schema) ||
    schema.title !== "ConversationProtocol" ||
    schema.version !== 1 ||
    typeof schema.protocolVersion !== "string" ||
    schema.protocolVersion.length === 0 ||
    !isRecord(schema.limits) ||
    !isRecord(schema.envelopeKeys) ||
    !Array.isArray(schema.methods) ||
    schema.methods.length === 0
  ) {
    fail(`unsupported conversation protocol schema: ${manifestFamiliesSchema()}`);
  }
  const limitKeys = [
    "maxFrameBytes",
    "maxResponseBytes",
    "maxIdBytes",
    "maxArgs",
    "maxClientArgs",
    "maxErrorCodeBytes",
    "maxStderrBytes",
  ];
  for (const key of limitKeys) {
    const value = schema.limits[key];
    if (!Number.isSafeInteger(value) || value <= 0 || value > 16 * 1024 * 1024) {
      fail(`invalid conversation protocol limit: ${key}`);
    }
  }
  const names = new Set();
  for (let index = 0; index < schema.methods.length; index += 1) {
    const method = schema.methods[index];
    if (
      !isRecord(method) ||
      typeof method.name !== "string" ||
      !methodNamePattern.test(method.name) ||
      method.name.length > 128 ||
      names.has(method.name) ||
      !kinds.has(method.kind) ||
      !lanes.has(method.lane) ||
      typeof method.structured !== "boolean" ||
      typeof method.stream !== "boolean" ||
      typeof method.inFlightControl !== "boolean"
    ) {
      fail(`invalid conversation protocol method at index ${index}`);
    }
    names.add(method.name);
    if (method.cli !== null) {
      if (
        !isRecord(method.cli) ||
        !Array.isArray(method.cli.argv) ||
        method.cli.argv.length === 0 ||
        method.cli.argv.some(
          (token) => typeof token !== "string" || token.length === 0,
        ) ||
        !Number.isSafeInteger(method.cli.positionals) ||
        method.cli.positionals < 0 ||
        method.cli.positionals > 8 ||
        !Array.isArray(method.cli.flags) ||
        method.cli.flags.length === 0 ||
        method.cli.flags.some(
          (token) => typeof token !== "string" || token.length === 0,
        ) ||
        !Array.isArray(method.cli.paramAliases) ||
        method.cli.paramAliases.some(
          (alias) =>
            !isRecord(alias) ||
            !Number.isSafeInteger(alias.argvIndex) ||
            alias.argvIndex < 0 ||
            typeof alias.param !== "string" ||
            alias.param.length === 0,
        )
      ) {
        fail(`invalid conversation protocol CLI shape at index ${index}`);
      }
    }
    for (const key of ["conversationLaneActions", "unboundedActions"]) {
      const actions = method[key];
      if (
        actions !== undefined &&
        (!Array.isArray(actions) ||
          actions.some(
            (action) => typeof action !== "string" || action.length === 0,
          ) ||
          new Set(actions).size !== actions.length)
      ) {
        fail(`invalid conversation protocol ${key} at index ${index}`);
      }
    }
    if (
      method.kind !== "command" &&
      (method.conversationLaneActions !== undefined ||
        method.unboundedActions !== undefined)
    ) {
      fail(`conversation protocol action sets require command kind at index ${index}`);
    }
  }
  if (
    schema.envelopeKeys.protocol !== "protocol" ||
    schema.envelopeKeys.method !== "method" ||
    schema.envelopeKeys.kind !== "kind"
  ) {
    fail(`unsupported conversation protocol envelope keys: ${manifestFamiliesSchema()}`);
  }
  return schema;
}

function manifestFamiliesSchema() {
  const manifest = readBoundedJson(
    manifestPath,
    maximumManifestBytes,
    "protocol-codegen manifest",
  );
  const family = manifest.families?.find(
    (candidate) => candidate.id === "conversation_protocol",
  );
  return family?.schema ?? "conversation protocol schema";
}

function rustEnumName(method) {
  return pascal(method.name);
}

function rustConversationProtocolOutput(schema, schemaPath) {
  const methods = schema.methods;
  const variants = methods
    .map(
      (method) =>
        `    ${rustEnumName(method)},`,
    )
    .join("\n");
  const asStrArms = methods
    .map(
      (method) =>
        `            Self::${rustEnumName(method)} => ${JSON.stringify(method.name)},`,
    )
    .join("\n");
  const fromWireArms = methods
    .map(
      (method) =>
        `            ${JSON.stringify(method.name)} => Some(Self::${rustEnumName(method)}),`,
    )
    .join("\n");
  const methodConsts = methods
    .map((method) => `    ${JSON.stringify(method.name)},`)
    .join("\n ");
  const fromWireFn = `impl ConversationProtocolMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
${asStrArms}
        }
    }

    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
${fromWireArms}
            _ => None,
        }
    }
}`;
  return `// @generated by ${schemaPath}; do not edit.
use serde_json::Value;
use std::path::PathBuf;

pub const CONVERSATION_PROTOCOL_VERSION: &str = ${JSON.stringify(
    schema.protocolVersion,
  )};
pub const CONVERSATION_PROTOCOL_MAX_FRAME_BYTES: usize = ${schema.limits.maxFrameBytes};
pub const CONVERSATION_PROTOCOL_MAX_RESPONSE_BYTES: usize = ${schema.limits.maxResponseBytes};
pub const CONVERSATION_PROTOCOL_MAX_ID_BYTES: usize = ${schema.limits.maxIdBytes};
pub const CONVERSATION_PROTOCOL_MAX_ARGS: usize = ${schema.limits.maxArgs};
pub const CONVERSATION_PROTOCOL_MAX_CLIENT_ARGS: usize = ${schema.limits.maxClientArgs};
pub const CONVERSATION_PROTOCOL_MAX_ERROR_CODE_BYTES: usize = ${schema.limits.maxErrorCodeBytes};
pub const CONVERSATION_PROTOCOL_MAX_STDERR_BYTES: usize = ${schema.limits.maxStderrBytes};

pub const CONVERSATION_PROTOCOL_METHODS: [&str; ${methods.length}] = [
${methodConsts}
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationProtocolMethod {
${variants}
}

${fromWireFn}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationCommand {
    pub id: String,
    pub workflow_id: String,
    pub method: ConversationProtocolMethod,
    pub params: Value,
    pub args: Option<Vec<String>>,
    pub portable_data_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationCommandError {
    pub id: Option<String>,
    pub workflow_id: Option<String>,
    pub code: &'static str,
}

impl ConversationCommand {
    pub fn decode(bytes: &[u8]) -> Result<Self, ConversationCommandError> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| ConversationCommandError {
            id: None,
            workflow_id: None,
            code: "invalid_json",
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| ConversationCommandError {
                id: None,
                workflow_id: None,
                code: "invalid_request",
            })?;
        if object.get("protocol").and_then(Value::as_str) != Some(CONVERSATION_PROTOCOL_VERSION) {
            return Err(ConversationCommandError {
                id: None,
                workflow_id: None,
                code: "invalid_protocol",
            });
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| ConversationCommand::valid_identifier(value))
            .map(str::to_string);
        let workflow_id = object
            .get("workflowId")
            .and_then(Value::as_str)
            .filter(|value| ConversationCommand::valid_identifier(value))
            .map(str::to_string);
        let invalid = |code| ConversationCommandError {
            id: id.clone(),
            workflow_id: workflow_id.clone(),
            code,
        };
        let id = id.clone().ok_or_else(|| invalid("invalid_request_id"))?;
        let workflow_id = workflow_id
            .clone()
            .ok_or_else(|| invalid("invalid_workflow_id"))?;
        let method = object
            .get("method")
            .and_then(Value::as_str)
            .and_then(ConversationProtocolMethod::from_wire)
            .ok_or_else(|| invalid("invalid_method"))?;
        let params = object
            .get("params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        if !params.is_object() {
            return Err(invalid("invalid_params"));
        }
        let args = match method {
            ConversationProtocolMethod::Execute => Some(
                object
                    .get("args")
                    .and_then(Value::as_array)
                    .filter(|args| args.len() <= CONVERSATION_PROTOCOL_MAX_ARGS)
                    .ok_or_else(|| invalid("invalid_args"))?
                    .iter()
                    .map(|value| value.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| invalid("invalid_args"))?,
            ),
            _ => None,
        };
        let portable_data_dir = match object.get("portableDataDir") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if !value.trim().is_empty() => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(invalid("invalid_portable_data_dir"));
                }
                Some(path)
            }
            _ => return Err(invalid("invalid_portable_data_dir")),
        };
        Ok(Self {
            id,
            workflow_id,
            method,
            params,
            args,
            portable_data_dir,
        })
    }

    pub fn valid_identifier(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= CONVERSATION_PROTOCOL_MAX_ID_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    }

    pub fn frame(
        id: &str,
        workflow_id: &str,
        method: ConversationProtocolMethod,
        params: Value,
    ) -> Value {
        json_frame(
            CONVERSATION_PROTOCOL_VERSION,
            id,
            workflow_id,
            method.as_str(),
            params,
        )
    }
}

fn json_frame(
    protocol: &str,
    id: &str,
    workflow_id: &str,
    method: &str,
    params: Value,
) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("protocol".to_owned(), Value::String(protocol.to_owned()));
    object.insert("id".to_owned(), Value::String(id.to_owned()));
    object.insert("workflowId".to_owned(), Value::String(workflow_id.to_owned()));
    object.insert("method".to_owned(), Value::String(method.to_owned()));
    object.insert("params".to_owned(), params);
    Value::Object(object)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConversationDelta {
    Event {
        id: String,
        workflow_id: String,
        sequence: u64,
        event: Value,
    },
    Terminal {
        id: String,
        workflow_id: String,
        sequence: u64,
        ok: bool,
        result: Option<Value>,
        error: Option<Value>,
    },
}

impl ConversationDelta {
    pub fn event_frame(id: &str, workflow_id: &str, sequence: u64, event: Value) -> Value {
        let mut object = serde_json::Map::new();
        object.insert(
            "protocol".to_owned(),
            Value::String(CONVERSATION_PROTOCOL_VERSION.to_owned()),
        );
        object.insert("id".to_owned(), Value::String(id.to_owned()));
        object.insert(
            "workflowId".to_owned(),
            Value::String(workflow_id.to_owned()),
        );
        object.insert("kind".to_owned(), Value::String("event".to_owned()));
        object.insert("sequence".to_owned(), Value::from(sequence));
        object.insert("event".to_owned(), event);
        Value::Object(object)
    }

    pub fn terminal_success_frame(
        id: &str,
        workflow_id: &str,
        sequence: u64,
        result: Value,
    ) -> Value {
        terminal_frame(id, workflow_id, sequence, true, Some(result), None)
    }

    pub fn terminal_error_frame(
        id: &str,
        workflow_id: &str,
        sequence: u64,
        error: Value,
    ) -> Value {
        terminal_frame(id, workflow_id, sequence, false, None, Some(error))
    }

    pub fn decode(frame: &Value) -> Result<Self, &'static str> {
        let object = frame.as_object().ok_or("invalid_delta")?;
        if object.get("protocol").and_then(Value::as_str) != Some(CONVERSATION_PROTOCOL_VERSION) {
            return Err("invalid_delta");
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| ConversationCommand::valid_identifier(value))
            .ok_or("invalid_delta")?;
        let workflow_id = object
            .get("workflowId")
            .and_then(Value::as_str)
            .filter(|value| ConversationCommand::valid_identifier(value))
            .ok_or("invalid_delta")?;
        let sequence = object
            .get("sequence")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or("invalid_delta")?;
        match object.get("kind").and_then(Value::as_str) {
            Some("event") => {
                let event = object.get("event").cloned().ok_or("invalid_delta")?;
                if !event.is_object() {
                    return Err("invalid_delta");
                }
                Ok(Self::Event {
                    id: id.to_owned(),
                    workflow_id: workflow_id.to_owned(),
                    sequence,
                    event,
                })
            }
            Some("terminal") => {
                let ok = object.get("ok").and_then(Value::as_bool).ok_or("invalid_delta")?;
                if ok {
                    let result = object.get("result").cloned().ok_or("invalid_delta")?;
                    if !result.is_object() {
                        return Err("invalid_delta");
                    }
                    Ok(Self::Terminal {
                        id: id.to_owned(),
                        workflow_id: workflow_id.to_owned(),
                        sequence,
                        ok: true,
                        result: Some(result),
                        error: None,
                    })
                } else {
                    let error = object.get("error").cloned().ok_or("invalid_delta")?;
                    if !error.is_object() {
                        return Err("invalid_delta");
                    }
                    Ok(Self::Terminal {
                        id: id.to_owned(),
                        workflow_id: workflow_id.to_owned(),
                        sequence,
                        ok: false,
                        result: None,
                        error: Some(error),
                    })
                }
            }
            _ => Err("invalid_delta"),
        }
    }
}

fn terminal_frame(
    id: &str,
    workflow_id: &str,
    sequence: u64,
    ok: bool,
    result: Option<Value>,
    error: Option<Value>,
) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "protocol".to_owned(),
        Value::String(CONVERSATION_PROTOCOL_VERSION.to_owned()),
    );
    object.insert("id".to_owned(), Value::String(id.to_owned()));
    object.insert(
        "workflowId".to_owned(),
        Value::String(workflow_id.to_owned()),
    );
    object.insert("kind".to_owned(), Value::String("terminal".to_owned()));
    object.insert("sequence".to_owned(), Value::from(sequence));
    object.insert("ok".to_owned(), Value::Bool(ok));
    if let Some(result) = result {
        object.insert("result".to_owned(), result);
    }
    if let Some(error) = error {
        object.insert("error".to_owned(), error);
    }
    Value::Object(object)
}
`;
}

function formatRust(source) {
  const result = spawnSync("rustfmt", ["--edition", "2024", "--emit", "stdout"], {
    cwd: repositoryRoot,
    encoding: "utf8",
    input: source,
  });
  if (result.status !== 0 || typeof result.stdout !== "string") {
    fail("rustfmt rejected generated conversation protocol output");
  }
  return result.stdout;
}

function atomicWrite(relativePath, content) {
  const absolutePath = path.join(repositoryRoot, relativePath);
  const directory = path.dirname(absolutePath);
  fs.mkdirSync(directory, { recursive: true });
  let temporaryPath;
  let descriptor;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    temporaryPath = path.join(
      directory,
      `.${path.basename(relativePath)}.${process.pid}.${attempt}.tmp`,
    );
    try {
      descriptor = fs.openSync(temporaryPath, "wx", 0o644);
      break;
    } catch (error) {
      if (error?.code !== "EEXIST") throw error;
    }
  }
  if (descriptor === undefined) fail(`unable to stage generated output: ${relativePath}`);
  try {
    fs.writeFileSync(descriptor, content, "utf8");
    fs.fsyncSync(descriptor);
    fs.closeSync(descriptor);
    descriptor = undefined;
    fs.renameSync(temporaryPath, absolutePath);
  } finally {
    if (descriptor !== undefined) fs.closeSync(descriptor);
    if (temporaryPath && fs.existsSync(temporaryPath)) fs.unlinkSync(temporaryPath);
  }
}

function reconcileOutput(relativePath, content, checkOnly) {
  let current = null;
  try {
    current = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
  } catch (error) {
    if (error?.code !== "ENOENT") fail(`unreadable generated output: ${relativePath}`);
  }
  if (current === content) return null;
  if (checkOnly) return `stale generated output: ${relativePath}`;
  atomicWrite(relativePath, content);
  return null;
}

// The manifest is shared with the Desktop repository, which renders the Dart
// halves. This repository renders only the Rust outputs it owns.
function renderFamily(family) {
  if (family.rustOutput === null) return [];
  if (family.id === "conversation_protocol") {
    const schema = readBoundedJson(
      family.schema,
      maximumSchemaBytes,
      "active conversation protocol schema",
    );
    const validated = validateSchema(schema);
    const rust = formatRust(
      rustConversationProtocolOutput(validated, family.schema),
    );
    return [[family.rustOutput, rust]];
  }
  fail(`unsupported protocol-codegen family: ${family.id}`);
}

function main() {
  const args = process.argv.slice(2);
  if (args.some((argument) => argument !== "--check") || args.length > 1) {
    fail("usage: generate-conversation-protocol.mjs [--check]");
  }
  const checkOnly = args[0] === "--check";
  const manifest = validateManifest();
  const diagnostics = [];
  for (const family of manifest.families) {
    for (const [relativePath, content] of renderFamily(family)) {
      if (content.length > maximumOutputBytes) {
        fail(`generated output exceeds limit: ${relativePath}`);
      }
      const diagnostic = reconcileOutput(relativePath, content, checkOnly);
      if (diagnostic) diagnostics.push(diagnostic);
    }
  }
  if (diagnostics.length > 0) fail(diagnostics.join("\n"));
}

try {
  main();
} catch (error) {
  const message =
    error instanceof ContractError
      ? error.message
      : "conversation protocol generation failed";
  process.stderr.write(`${message.slice(0, 4096)}\n`);
  process.exitCode = 1;
}
