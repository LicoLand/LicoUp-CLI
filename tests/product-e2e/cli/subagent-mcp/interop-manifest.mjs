#!/usr/bin/env node
import { closeSync, existsSync, openSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
export const INTEROP_MANIFEST_RELATIVE_PATH = "tests/product-e2e/cli/subagent-mcp/interop-manifest.yaml";
export const INTEROP_MANIFEST_FIELDS = Object.freeze([
  "App Version", "Caller Agent", "Caller Agent Version", "Target Agent",
  "Target Agent Version", "Results", "Notes",
]);
export const TARGET_AGENTS = Object.freeze(["codex", "cursor", "antigravity"]);
const HEADER = [
  "# Latest LicoUp App Version Subagent MCP direct verification evidence.",
  "# One privacy-safe row per Target Agent; live execution atomically upserts rows.",
].join("\n");
const APP_VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/u;
const VERSION = /^(?:\d+(?:\.\d+)+(?:-[0-9A-Za-z.-]+)?|\d{4}\.\d{2}\.\d{2}-[0-9A-Fa-f]{7,40})$/u;
export const INTEROP_FAILURE_NOTES = Object.freeze([
  "approved_model_unavailable", "caller_authentication_required",
  "caller_membership_binding_required", "caller_membership_not_authorized",
  "caller_version_unavailable", "conversation_not_found",
  "conversation_state_unavailable", "conversation_working_directory_mismatch",
  "direct_mcp_failed", "direct_mcp_rejected", "dispatch_claim_missing",
  "dispatch_reconciliation_required", "inbound_delegate_missing", "invalid_request",
  "invalid_working_directory", "manifest_invalid", "provider_execution_route_unavailable",
  "provider_identity_mismatch", "provider_not_installed", "request_cancelled",
  "persistent_conversation_transport_required", "service_unavailable",
  "subagent_adapter_unavailable", "subagent_capacity_exhausted",
  "subagent_caller_membership_inactive", "subagent_capability_unavailable",
  "subagent_cross_conversation_rejected", "subagent_depth_exceeded",
  "subagent_dispatch_receipt_invalid", "subagent_dispatch_transition_invalid",
  "subagent_dispatch_uncertain", "subagent_duplicate_active_edge",
  "subagent_lineage_caller_mismatch", "subagent_lineage_cycle",
  "subagent_parent_dispatch_unavailable", "subagent_self_call_rejected",
  "subagent_target_invalid", "subagent_target_membership_inactive",
  "subagent_transport_failed", "subagent_transport_invalid_response",
  "subagent_transport_timeout", "subagent_turn_not_active", "subagent_turn_not_found",
  "subagent_turn_scope_mismatch",
  "target_dispatch_missing", "target_membership_mismatch", "target_membership_unavailable",
  "target_runtime_unavailable", "target_version_unavailable", "verification_in_progress",
  "verification_lease_unavailable",
]);
const ALLOWED_NOTES = new Set(["", ...INTEROP_FAILURE_NOTES]);

export class InteropManifestError extends Error {
  constructor(code = "manifest_invalid") { super(code); this.code = code; }
}

export function interopManifestPath(root = repositoryRoot) {
  return join(root, INTEROP_MANIFEST_RELATIVE_PATH);
}

export function readRepoAppVersion(root = repositoryRoot) {
  try {
    return admitAppVersion(JSON.parse(readFileSync(join(root, "tools", "client-version.json"), "utf8"))?.productVersion);
  } catch (error) {
    if (error instanceof InteropManifestError) throw error;
    throw new InteropManifestError();
  }
}

export function createInteropRecord(input) {
  const record = {
    appVersion: admitAppVersion(input?.appVersion),
    callerAgent: admitAgent(input?.callerAgent),
    callerAgentVersion: admitVersion(input?.callerAgentVersion),
    targetAgent: admitAgent(input?.targetAgent),
    targetAgentVersion: admitVersion(input?.targetAgentVersion),
    results: admitResults(input?.results),
    notes: admitNotes(input?.notes),
  };
  if (record.callerAgent === record.targetAgent) throw new InteropManifestError();
  if ((record.results === "passed") !== (record.notes === "")) throw new InteropManifestError();
  return Object.freeze(record);
}

export function renderInteropManifestYaml(records = []) {
  const admitted = admitRecordSet(records);
  const rows = admitted.map((row) => [
    `- App Version: ${quoted(row.appVersion)}`,
    `  Caller Agent: ${quoted(row.callerAgent)}`,
    `  Caller Agent Version: ${quoted(row.callerAgentVersion)}`,
    `  Target Agent: ${quoted(row.targetAgent)}`,
    `  Target Agent Version: ${quoted(row.targetAgentVersion)}`,
    `  Results: ${quoted(row.results)}`,
    `  Notes: ${quoted(row.notes)}`,
  ].join("\n")).join("\n\n");
  return `${HEADER}\n${rows ? `\n${rows}\n` : ""}`;
}

export function parseInteropManifestYaml(raw) {
  const lines = String(raw ?? "").split(/\r?\n/u);
  const records = [];
  const expectedHeader = HEADER.split("\n");
  if (lines[0] !== expectedHeader[0] || lines[1] !== expectedHeader[1]) throw new InteropManifestError();
  let index = 2;
  if (lines[index] === "") index += 1;
  while (index < lines.length) {
    const fields = {};
    for (let fieldIndex = 0; fieldIndex < INTEROP_MANIFEST_FIELDS.length; fieldIndex += 1) {
      const field = INTEROP_MANIFEST_FIELDS[fieldIndex];
      const prefix = fieldIndex === 0 ? `- ${field}: ` : `  ${field}: `;
      const line = lines[index++];
      if (typeof line !== "string" || !line.startsWith(prefix)) throw new InteropManifestError();
      fields[field] = parseQuoted(line.slice(prefix.length));
    }
    records.push(createInteropRecord({
      appVersion: fields["App Version"], callerAgent: fields["Caller Agent"],
      callerAgentVersion: fields["Caller Agent Version"], targetAgent: fields["Target Agent"],
      targetAgentVersion: fields["Target Agent Version"], results: fields.Results, notes: fields.Notes,
    }));
    if (lines[index] === "") index += 1;
  }
  return admitRecordSet(records);
}

export function shouldSkipTarget(records, { appVersion, targetAgent, targetAgentVersion }) {
  return records.some((record) => record.appVersion === appVersion
    && record.targetAgent === targetAgent && record.targetAgentVersion === targetAgentVersion
    && record.results === "passed");
}

export function isInteropVersion(value) {
  try { admitVersion(value); return true; }
  catch { return false; }
}

export function upsertTargetRecord(existing, input) {
  const next = createInteropRecord(input);
  const merged = existing.filter((row) => row.appVersion === next.appVersion && row.targetAgent !== next.targetAgent);
  merged.push(next);
  return admitRecordSet(merged).sort((left, right) => TARGET_AGENTS.indexOf(left.targetAgent) - TARGET_AGENTS.indexOf(right.targetAgent));
}

export function readInteropManifest(path = interopManifestPath()) {
  return existsSync(path) ? parseInteropManifestYaml(readFileSync(path, "utf8")) : [];
}

export function persistTargetRecord({ path = interopManifestPath(), record }) {
  const merged = upsertTargetRecord(readInteropManifest(path), record);
  atomicWrite(path, renderInteropManifestYaml(merged));
  return merged;
}

function admitRecordSet(records) {
  if (!Array.isArray(records)) throw new InteropManifestError();
  const admitted = records.map((record) => createInteropRecord(record));
  const versions = new Set(admitted.map((record) => record.appVersion));
  const targets = new Set(admitted.map((record) => record.targetAgent));
  if (versions.size > 1 || targets.size !== admitted.length || admitted.length > TARGET_AGENTS.length) throw new InteropManifestError();
  return admitted;
}
function admitAppVersion(value) { const text = String(value ?? ""); if (!APP_VERSION.test(text)) throw new InteropManifestError(); return text; }
function admitAgent(value) { if (!TARGET_AGENTS.includes(value)) throw new InteropManifestError(); return value; }
function admitVersion(value) { const text = String(value ?? ""); if (text.length > 80 || !VERSION.test(text)) throw new InteropManifestError(); return text; }
function admitResults(value) { if (value !== "passed" && value !== "failed") throw new InteropManifestError(); return value; }
function admitNotes(value) { if (!ALLOWED_NOTES.has(value)) throw new InteropManifestError(); return value; }
function quoted(value) { return JSON.stringify(value); }
function parseQuoted(value) {
  if (!value.startsWith('"') || !value.endsWith('"')) throw new InteropManifestError();
  try { const parsed = JSON.parse(value); if (typeof parsed !== "string") throw new Error(); return parsed; }
  catch { throw new InteropManifestError(); }
}
function atomicWrite(path, contents) {
  const temporary = `${path}.tmp-${process.pid}`;
  let descriptor;
  try {
    descriptor = openSync(temporary, "wx", 0o600);
    writeFileSync(descriptor, contents, "utf8");
    closeSync(descriptor); descriptor = undefined;
    renameSync(temporary, path);
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
    if (existsSync(temporary)) unlinkSync(temporary);
  }
}
