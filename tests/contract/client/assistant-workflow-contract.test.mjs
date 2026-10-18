import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const read = (relative) => readFileSync(path.join(root, relative), "utf8");

const decision0003 = read("docs/adrs/0003-group-conversation-agent-profile.md");
const decision0004 = read("docs/adrs/0004-assistant-authored-flexible-workflows.md");
const decision0005 = read("docs/adrs/0005-assistant-auto-adaptation-and-deepseek-harness.md");
const domain = read("crates/licoup-native/src/domain/client_conversation/mod.rs");
const conversationDomain = read("crates/licoup-conversation/src/client_conversation/mod.rs");
const store = read("crates/licoup-conversation/src/store/mod.rs");
const schema = read("crates/licoup-conversation/src/store/schema.rs");
const profile = read("crates/licoup-native/src/domain/client_conversation/profile_snapshot.rs");
const assistant = read("crates/licoup-workflow-runtime/src/assistant.rs");
const flywheelService = read("crates/licoup-workflow-runtime/src/service.rs");
// The route receipt and the source revisions it names are the port surface's
// own contract, so they are read where the runtime declares them.
const portContracts = read("crates/licoup-workflow-runtime/src/ports.rs");
const usage = read("crates/licoup-statistics/src/ledger.rs");
const policy = read("crates/licoup-client-state/src/policy.rs");
const subagents = read("crates/licoup-native/src/domain/subagents/mod.rs");
const conversationContract = JSON.parse(read("schemas/client_bridge/conversation.json"));
const strategyContract = JSON.parse(read("schemas/client_bridge/strategy.json"));
const bundledSkill = read("crates/licoup-native/resources/licoup-guide/SKILL.md");

const FORBIDDEN_PRIVATE = [
  /prompt body/u,
  /secret_token/u,
  /authorization: Bearer/u,
  /<user-home>\/private-workspace-sentinel/u,
  /<windows-user-home>\\private-workspace-sentinel/u,
  /machine-id/u,
  /endpoint-token/u,
];

test("ADR 0003 is historical and ADR 0004 freezes the Assistant boundary", () => {
  assert.match(decision0003, /# ADR 0003/u);
  // Decision 0003 deliberately defers the concrete fields, format, and
  // default usage to a follow-up decision.
  assert.match(decision0003, /intentionally unspecified|follow-up decision|left open/u);
  assert.match(decision0004, /assistant-temporary/u);
  assert.match(decision0004, /licoup-guide/u);
  assert.match(decision0005, /Automatic adaptation/u);
  assert.match(decision0005, /DeepSeek Harness/u);
});

test("conversation migration v8 cuts over to intent-only Assistant Profiles idempotently", () => {
  assert.equal(conversationDomain.includes('LICOUP_GUIDE_SKILL_ID: &str = "licoup-guide"'), true);
  assert.match(domain, /include_str!\([\s\S]*licoup-guide\/SKILL\.md/u);
  assert.match(store, /mod schema;/u);
  assert.match(schema, /CREATE TABLE IF NOT EXISTS membership_profiles/u);
  assert.match(schema, /CREATE INDEX IF NOT EXISTS membership_profiles_membership_idx/u);
  assert.match(schema, /assistant_membership_id TEXT REFERENCES memberships\(id\)/u);
  assert.match(schema, /INSERT INTO schema_meta\(key, value\) VALUES \('version', '8'\)/u);
  assert.match(store, /pub fn set_conversation_assistant/u);
  assert.match(store, /pub fn set_membership_profile/u);
  assert.match(store, /pub fn membership_profiles/u);
  // Migration is applied before any store read and repeated opens replay it
  // without reinterpretation; the retired ordinal generation has no table.
  assert.match(store, /normalize_reserved_default_group_after_legacy_import/u);
  assert.match(schema, /DROP TABLE IF EXISTS flywheels/u);
});

test("Profile snapshots derive only from named existing authorities", () => {
  // The projection and its request-scoped authority live with the workflow
  // runtime's port surface; the host module keeps the owner-backed
  // implementation of the same contract.
  const projection = profile + portContracts;
  assert.match(projection, /trait ProfileSnapshotAuthority/u);
  assert.match(projection, /fn target_facts/u);
  assert.match(projection, /fn model_price_usd_per_million_tokens/u);
  assert.match(projection, /fn coding_score/u);
  assert.match(projection, /agent_model_max_intelligence/u);
  assert.match(projection, /fn skill_names/u);
  assert.match(projection, /RequestScopedAuthority/u);
  assert.match(projection, /reads each owner at most once/u);
  assert.match(projection, /targets::inspect_target_read_only/u);
  assert.match(projection, /provider_model_pricing::model_price/u);
  assert.match(
    projection,
    /agent_intelligence_catalog::agent_model_max_intelligence/u,
  );
  assert.match(projection, /skill_hub::skill_list/u);
  // The Assistant Profile references one concise, product-owned coordinator Skill.
  assert.match(projection, /LICOUP_GUIDE_SKILL_ID/u);
  const prompt = bundledSkill.split("\n---\n").at(-1).trim();
  assert.ok(prompt.length > 0);
  assert.match(bundledSkill, /^name: licoup-guide$/mu);
  const documentedTools = [...prompt.matchAll(/`(lico_[a-z_]+)`/gu)].map((match) => match[1]);
  assert.ok(documentedTools.length > 0);
  for (const tool of documentedTools) {
    assert.ok(subagents.includes(`"${tool}"`), `guide tool is available: ${tool}`);
  }
  assert.match(usage, /graph-usage-ledger-v2\.sqlite3/u);
  assert.match(usage, /licoup\.graph-usage-report\.v2/u);
  assert.doesNotMatch(policy, /assistant-workflow-usage/u);
});

test("candidate ranking is deterministic and keeps unknown optional facts visible", () => {
  assert.match(portContracts, /pub fn rank_candidates/u);
  assert.match(portContracts, /optional_desc\(left\.intelligence_score, right\.intelligence_score\)/u);
  assert.match(portContracts, /optional_price\(left\)\.cmp\(&optional_price\(right\)\)/u);
  assert.match(portContracts, /optional_asc\(left\.latency_class, right\.latency_class\)/u);
  assert.match(portContracts, /left\.membership_id\.cmp\(&right\.membership_id\)/u);
  assert.match(portContracts, /profile_candidate_rejected/u);
  assert.match(portContracts, /Hard constraints/u);
});

test("preflight diagnostic stages match the public bridge contract", () => {
  // Admission, stale-route rejection and effect idempotency are exercised by
  // the native service tests; Rust declarations are not their behavior oracle.
  assert.deepEqual(strategyContract.diagnosticStages, [
    "workflow/parse",
    "workflow/compile",
    "package/validate",
    "assistant-workflow/preflight",
    "assistant-workflow/revalidate",
  ]);
});

test("receipts freeze exact bindings and allowlisted sources without private data", () => {
  assert.match(assistant, /pub struct PreflightReceipt/u);
  for (const field of [
    "conversation_id",
    "assistant_membership_id",
    "workflow_digest",
    "membership_ids",
    "route_receipt",
  ]) {
    assert.match(assistant, new RegExp(`pub ${field}:`, "u"), field);
  }
  assert.doesNotMatch(assistant, /pub checks:/u);
  assert.match(assistant, /route_receipt/u);
  for (const owner of [
    "targets",
    "providerModelPricing",
    "agentIntelligenceCatalog",
    "skillHub",
    "assistantWorkflowAuthoringBundle",
  ]) {
    assert.match(portContracts + read("crates/licoup-native/src/domain/client_conversation/service.rs"), new RegExp(`"${owner}"`, "u"));
  }
  for (const pattern of FORBIDDEN_PRIVATE) {
    assert.doesNotMatch(assistant, pattern, pattern);
  }
});

test("bridge contracts expose Assistant/Profile actions and typed failures", () => {
  for (const action of [
    "conversation.assistant.set",
    "conversation.profile.update",
    "conversation.profile.get",
    "conversation.profile.candidates",
    "conversation.subagent.edge",
  ]) {
    assert.equal(conversationContract.actions.includes(action), true, action);
  }
  for (const action of [
    "strategy.assistant.workflow.execute",
    "strategy.assistant.workflow.inspect",
    "strategy.assistant.workflow.cancel",
  ]) {
    assert.equal(strategyContract.actions.includes(action), true, action);
  }
  for (const code of [
    "profile_intent_invalid",
    "profile_revision_stale",
    "profile_candidate_rejected",
    "graph_invalid",
    "graph_preflight_rejected",
    "graph_identity_rejected",
    "strategy_idempotency_conflict",
  ]) {
    assert.equal(
      [...conversationContract.failureCodes, ...strategyContract.failureCodes].includes(code),
      true,
      code,
    );
  }
});

test("local native Subagent catalog exposes the Assistant workflow facade", () => {
  const assistantTools = [
    "lico_assistant_profiles",
    "lico_assistant_workflow_execute",
    "lico_assistant_workflow_inspect",
    "lico_assistant_workflow_cancel",
  ];
  for (const name of assistantTools) {
    assert.match(subagents, new RegExp(`"${name}"`, "u"), name);
  }
  const catalog = subagents.slice(
    subagents.indexOf("pub const TOOL_NAMES"),
    subagents.indexOf("pub fn tool_catalog"),
  );
  assert.deepEqual(
    [...catalog.matchAll(/"(lico_assistant_[^"]+)"/gu)].map((match) => match[1]),
    assistantTools,
  );
  assert.match(subagents, /"additionalProperties": false/u);
  assert.doesNotMatch(catalog, /conversationPath/u);
  assert.doesNotMatch(catalog, /sessionMode/u);
  for (const pattern of FORBIDDEN_PRIVATE) {
    assert.doesNotMatch(catalog, pattern, pattern);
  }
});
