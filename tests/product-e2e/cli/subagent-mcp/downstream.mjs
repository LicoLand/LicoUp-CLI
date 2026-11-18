#!/usr/bin/env node
import { execFile } from "node:child_process";
import { mkdirSync, readFileSync, rmdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { verificationModelForAgent } from "../../../../tools/scripts/lib/agent-conversation-verification-models.mjs";
import {
  admitDiscoveryDocument, DirectMcpClient, FROZEN_TOOL_NAMES,
  MCP_SERVER_NAME, MCP_SERVER_VERSION, verifyServiceHealth,
} from "./streamable-http.mjs";
import {
  INTEROP_FAILURE_NOTES, TARGET_AGENTS, createInteropRecord, interopManifestPath,
  isInteropVersion, persistTargetRecord, readInteropManifest, readRepoAppVersion, shouldSkipTarget,
} from "./interop-manifest.mjs";

const execFileAsync = promisify(execFile);
export const APPROVED_TARGET_MODELS = Object.freeze({
  codex: Object.freeze([...new Set([verificationModelForAgent("codex"), "gpt-5.4-mini"])]),
  cursor: Object.freeze([verificationModelForAgent("cursor")]),
  antigravity: Object.freeze([verificationModelForAgent("antigravity")]),
});
export const NON_SELF_CALLER = Object.freeze({ codex: "cursor", cursor: "codex", antigravity: "codex" });
const SUCCESSFUL_CLAIM_STATES = new Set(["running", "completed"]);
const SUCCESSFUL_DISPATCH_STATES = new Set(["accepted", "running", "completed"]);
const SAFE_FAILURE_CODES = new Set([...INTEROP_FAILURE_NOTES, "argument_unsupported"]);
const SAFE_MCP_STAGES = new Set([
  "adapter/select", "caller/authenticate", "capability/admit", "conversation/authorize",
  "conversation/open", "conversation/read", "conversation/store", "dispatch/reconcile",
  "dispatch/admit", "dispatch/transition", "identity/resolve", "lineage/admit",
  "persistent-turn/connect", "persistent-turn/dispatch", "persistent-turn/exchange",
  "persistent-turn/readback", "request/cancel", "schema/validate", "session/authorize",
  "session/resolve", "target/admit",
]);
const SAFE_MCP_RECOVERIES = new Set([
  "correct_request_and_retry", "reconcile_before_retry", "retry_after_recovery",
]);

export class DownstreamVerificationError extends Error {
  constructor(code, failure = null) {
    super(code);
    this.code = code;
    if (failure) this.failure = Object.freeze({ ...failure });
  }
}

export function projectStructuredMcpFailure(result) {
  const value = result?.structuredContent;
  if (result?.isError !== true || !value || typeof value !== "object" || Array.isArray(value)) return null;
  if (JSON.stringify(Object.keys(value).sort())
    !== JSON.stringify(["reasonCode", "recovery", "retryable", "schemaVersion", "stage"])) return null;
  if (value.schemaVersion !== "licoup.mcp.error.v1"
    || !SAFE_FAILURE_CODES.has(value.reasonCode)
    || !SAFE_MCP_STAGES.has(value.stage)
    || typeof value.retryable !== "boolean"
    || !SAFE_MCP_RECOVERIES.has(value.recovery)) return null;
  if ((value.recovery === "correct_request_and_retry") === value.retryable) return null;
  return Object.freeze({
    code: value.reasonCode,
    stage: value.stage,
    retryable: value.retryable,
    recovery: value.recovery,
  });
}

export function selectApprovedModel(targetAgent, availableModels = []) {
  const available = new Set(availableModels);
  return (APPROVED_TARGET_MODELS[targetAgent] ?? []).find((model) => available.has(model)) ?? "";
}

export function evaluateCanonicalEffect({ edge, targetMembershipId, selectedMembershipId }) {
  if (!targetMembershipId || targetMembershipId !== selectedMembershipId) return "target_membership_mismatch";
  if (edge?.inbound?.delegate !== true || edge?.outcomes?.delegate !== "accepted") return "inbound_delegate_missing";
  if (!SUCCESSFUL_CLAIM_STATES.has(edge?.claimState)) return "dispatch_claim_missing";
  if (!SUCCESSFUL_DISPATCH_STATES.has(edge?.dispatchState)) return "target_dispatch_missing";
  return "";
}

export function acquireLiveLease(path) {
  try {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    mkdirSync(path, { mode: 0o700 });
  }
  catch (error) {
    if (error?.code === "EEXIST") throw new DownstreamVerificationError("verification_in_progress");
    throw new DownstreamVerificationError("verification_lease_unavailable");
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    rmdirSync(path);
  };
}

export async function runDownstream(options = {}) {
  const appVersion = options.appVersion ?? readRepoAppVersion(options.repositoryRoot);
  const portableRoot = options.portableRoot ?? process.env.LICOUP_HOME;
  const executable = options.executable ?? process.env.LICOUP_CLI_EXECUTABLE;
  const targetFacts = options.targetFacts ?? await (options.resolveTargetFacts ?? resolveTargetFacts)({
    executable,
    portableRoot,
    executeJson: options.executeJson,
  });
  const manifestPath = options.manifestPath ?? interopManifestPath(options.repositoryRoot);
  const readManifest = options.readManifest ?? (() => readInteropManifest(manifestPath));
  const initial = readManifest();
  const preflight = TARGET_AGENTS.map((targetAgent) => decisionFor(
    targetAgent,
    targetFacts[targetAgent],
    targetFacts[NON_SELF_CALLER[targetAgent]],
    appVersion,
    initial,
  ));
  if (options.live !== true) {
    const admitted = await admitServiceHealth(preflight, options, { portableRoot });
    return { route: "downstream", mode: "preflight", targets: admitted.map(publicDecision) };
  }

  const leasePath = options.leasePath ?? join(
    options.repositoryRoot ?? process.cwd(),
    ".licoup-subagent-mcp-live.lock",
  );
  const release = (options.acquireLease ?? acquireLiveLease)(leasePath);
  const prepareConversation = options.prepareConversation
    ?? ((context) => prepareVerificationConversation({ ...context, portableRoot, executable }));
  const directDelegate = options.directDelegate
    ?? ((context) => directDelegateThroughDiscovery({ ...context, portableRoot }));
  const readCanonicalEdge = options.readCanonicalEdge
    ?? ((context) => readVerificationEdge({ ...context, portableRoot, executable }));
  const receipts = [];
  try {
    const current = readManifest(); // Exclusive final reread before any paid effect.
    const finalDecisions = TARGET_AGENTS.map((targetAgent) => decisionFor(
      targetAgent,
      targetFacts[targetAgent],
      targetFacts[NON_SELF_CALLER[targetAgent]],
      appVersion,
      current,
    ));
    const admitted = await admitServiceHealth(finalDecisions, options, { portableRoot });
    for (const decision of admitted) {
      const { targetAgent } = decision;
      const facts = targetFacts[targetAgent];
      const callerAgent = NON_SELF_CALLER[targetAgent];
      const callerAgentVersion = targetFacts[callerAgent]?.version;
      if (decision.skip) { receipts.push(publicDecision(decision)); continue; }
      if (!isInteropVersion(facts?.version) || !isInteropVersion(callerAgentVersion)) {
        receipts.push(publicDecision(decision));
        continue;
      }
      let result = "failed";
      let notes = decision.reason;
      let failure = null;
      if (!notes) {
        try {
          const prepared = await prepareConversation({ targetAgent, callerAgent, model: decision.model });
          if (prepared?.callerAgent !== callerAgent || prepared?.targetAgent !== targetAgent
            || prepared?.callerMembershipId === prepared?.targetMembershipId) {
            throw new DownstreamVerificationError("target_membership_mismatch");
          }
          const call = await directDelegate({ ...prepared, model: decision.model, callerAgent, targetAgent });
          if (call?.accepted !== true) throw new DownstreamVerificationError("direct_mcp_rejected");
          const edge = await readCanonicalEdge(prepared);
          notes = evaluateCanonicalEffect({
            edge,
            targetMembershipId: call?.membershipId ?? prepared.targetMembershipId,
            selectedMembershipId: prepared.targetMembershipId,
          });
          if (!notes) result = "passed";
        } catch (error) {
          notes = safeFailure(error?.code) || "direct_mcp_failed";
          failure = error?.failure ?? null;
        }
      }
      const record = createInteropRecord({
        appVersion, callerAgent, callerAgentVersion,
        targetAgent, targetAgentVersion: facts?.version, results: result, notes,
      });
      (options.persistRecord ?? ((value) => persistTargetRecord({ path: manifestPath, record: value })))(record);
      receipts.push(failure
        ? { targetAgent, result, reason: notes, failure }
        : { targetAgent, result, reason: notes });
    }
  } finally { release(); }
  return { route: "downstream", mode: "live", targets: receipts };
}

function decisionFor(targetAgent, facts, callerFacts, appVersion, records) {
  if (!isInteropVersion(facts?.version)) {
    return { targetAgent, result: "failed", reason: "target_version_unavailable", skip: false };
  }
  if (shouldSkipTarget(records, { appVersion, targetAgent, targetAgentVersion: facts.version })) {
    return { targetAgent, result: "passed", reason: "already_verified", skip: true };
  }
  if (facts.runtimeAvailable !== true) {
    return { targetAgent, result: "failed", reason: "target_runtime_unavailable", skip: false };
  }
  if (!isInteropVersion(callerFacts?.version)) {
    return { targetAgent, result: "failed", reason: "caller_version_unavailable", skip: false };
  }
  const model = selectApprovedModel(targetAgent, facts.availableModels);
  if (!model) return { targetAgent, result: "failed", reason: "approved_model_unavailable", skip: false };
  return { targetAgent, result: "ready", reason: "", model, skip: false };
}

function safeFailure(code) {
  return SAFE_FAILURE_CODES.has(code) ? code : "";
}

function publicDecision(decision) {
  return {
    targetAgent: decision.targetAgent,
    result: decision.result,
    reason: decision.reason,
    skip: decision.skip,
  };
}

async function admitServiceHealth(decisions, options, { portableRoot }) {
  const verifier = options.verifyHealth ?? verifyDownstreamServiceHealth;
  const callers = [...new Set(decisions
    .filter((decision) => !decision.skip && !decision.reason)
    .map((decision) => NON_SELF_CALLER[decision.targetAgent]))];
  const health = new Map(await Promise.all(callers.map(async (callerAgent) => {
    try {
      const receipt = await verifier({
        callerAgent,
        portableRoot,
        fetchImpl: options.fetchImpl,
      });
      return [callerAgent, receipt?.result === "passed"];
    } catch {
      return [callerAgent, false];
    }
  })));
  return decisions.map((decision) => {
    if (decision.skip || decision.reason) return decision;
    return health.get(NON_SELF_CALLER[decision.targetAgent]) === true
      ? decision
      : { ...decision, result: "failed", reason: "service_unavailable" };
  });
}

export async function resolveTargetFacts({ executable, portableRoot, executeJson } = {}) {
  if (typeof executable !== "string" || !executable || typeof portableRoot !== "string" || !portableRoot) {
    return {};
  }
  const execute = executeJson
    ?? ((args) => executeLicoupJson(executable, portableRoot, args));
  const selected = JSON.stringify({
    targetIds: TARGET_AGENTS,
    modelCatalogTargetIds: TARGET_AGENTS,
  });
  const invocations = [
    [
      "targets", "scan",
      "--stdin-json", selected,
      "--include-accessible-environments", "false",
      "--include-history-model-catalog", "false",
      "--enable-agent-cli-model-lookup", "true",
    ],
    ...TARGET_AGENTS.map((agent) => ["agent-hub", "catalog", "--agent-id", agent]),
  ];
  const settled = [];
  for (const args of invocations) {
    try { settled.push(await execute(args)); } catch { settled.push(null); }
  }
  const scan = settled[0];
  const scanResults = Array.isArray(scan?.results) ? scan.results : [];
  return Object.freeze(Object.fromEntries(TARGET_AGENTS.map((agent, index) => {
    const candidate = scanResults
      .find((row) => row?.targetId === agent && row?.ok === true)?.candidate;
    const models = Array.isArray(candidate?.modelCatalog?.models)
      ? candidate.modelCatalog.models
        .map((model) => model?.name)
        .filter((name) => typeof name === "string" && name.length > 0 && name.length <= 128)
        .slice(0, 512)
      : [];
    const cards = Array.isArray(settled[index + 1]?.cards) ? settled[index + 1].cards : [];
    const card = cards.find((item) => item?.id === agent);
    const version = typeof card?.version === "string" ? card.version : "";
    return [agent, Object.freeze({
      version,
      availableModels: Object.freeze([...new Set(models)]),
      runtimeAvailable: ["detected", "configured", "available"].includes(candidate?.status)
        && Array.isArray(candidate?.supportedActions)
        && candidate.supportedActions.includes("runtime.message.send"),
    })];
  })));
}

async function executeLicoupJson(executable, portableRoot, args) {
  const { stdout } = await execFileAsync(executable, args, {
    env: { ...process.env, LICOUP_HOME: portableRoot },
    maxBuffer: 1024 * 1024,
    encoding: "utf8",
    windowsHide: true,
  });
  const value = JSON.parse(stdout);
  if (value?.ok !== true) throw new DownstreamVerificationError("target_version_unavailable");
  return value;
}

export async function verifyDownstreamServiceHealth({ portableRoot, callerAgent, fetchImpl }) {
  if (!portableRoot) throw new DownstreamVerificationError("service_unavailable");
  const discovery = admitDiscoveryDocument(JSON.parse(readFileSync(
    join(portableRoot, "client-state", "subagent-mcp", "discovery.json"),
    "utf8",
  )));
  return verifyServiceHealth(new DirectMcpClient({
    endpoint: discovery.endpoint,
    token: discovery.tokens?.[callerAgent],
    fetchImpl,
  }));
}

export async function directDelegateThroughDiscovery({ portableRoot, callerAgent, targetAgent, conversationId, callerMembershipId, targetMembershipId, model, fetchImpl }) {
  const discovery = admitDiscoveryDocument(JSON.parse(readFileSync(
    join(portableRoot, "client-state", "subagent-mcp", "discovery.json"),
    "utf8",
  )));
  const client = new DirectMcpClient({
    endpoint: discovery.endpoint, token: discovery.tokens?.[callerAgent],
    conversationId, membershipId: callerMembershipId, fetchImpl,
  });
  try {
    const initialized = await client.initialize();
    const names = (await client.listTools()).map((tool) => tool?.name);
    if (
      initialized?.protocolVersion !== "2025-06-18"
      || initialized?.serverInfo?.name !== MCP_SERVER_NAME
      || initialized?.serverInfo?.version !== MCP_SERVER_VERSION
      || JSON.stringify(names) !== JSON.stringify(FROZEN_TOOL_NAMES)
    ) {
      throw new DownstreamVerificationError("service_unavailable");
    }
    const result = await client.callTool("lico_subagent_delegate", {
      conversationId, membershipId: targetMembershipId,
      prompt: "Verify direct Subagent MCP dispatch.", model,
    });
    if (result?.isError === true) {
      const failure = projectStructuredMcpFailure(result);
      throw new DownstreamVerificationError(failure?.code ?? "direct_mcp_failed", failure);
    }
    const receipt = result?.structuredContent;
    const accepted = result?.isError === false
      && receipt?.schemaVersion === "licoup.subagent.receipt.v3"
      && receipt?.operation === "subagent.delegate"
      && receipt?.conversationId === conversationId
      && receipt?.membershipId === targetMembershipId
      && receipt?.agentId === targetAgent
      && typeof receipt?.dispatchId === "string"
      && receipt.dispatchId.length > 0
      && (receipt?.state === "completed"
        || (receipt?.state === "accepted" && receipt?.accepted === true));
    return { membershipId: receipt?.membershipId ?? "", accepted };
  } finally { await client.close(); }
}

export async function prepareVerificationConversation({ portableRoot, executable, callerAgent, targetAgent, executeFile }) {
  if (!portableRoot || !executable) throw new DownstreamVerificationError("target_membership_unavailable");
  const created = await executeConversationCli(executable, portableRoot, {
    action: "conversation.create",
    title: "Subagent MCP verification",
    owner: { id: "human:verification", kind: "human", displayName: "Verification" },
    members: [callerAgent, targetAgent].map((agent) => ({
      principal: { id: `agent:${agent}`, kind: "agent", displayName: agent, agentId: agent },
      access: "member",
    })),
  }, executeFile);
  const membership = (agent) => created?.memberships?.find((item) => item?.principal?.agentId === agent)?.id ?? "";
  const callerMembershipId = membership(callerAgent);
  const targetMembershipId = membership(targetAgent);
  if (!created?.id || !callerMembershipId || !targetMembershipId) {
    throw new DownstreamVerificationError("target_membership_unavailable");
  }
  return { conversationId: created.id, callerAgent, targetAgent, callerMembershipId, targetMembershipId };
}

export async function readVerificationEdge({ portableRoot, executable, conversationId, callerMembershipId, targetMembershipId, executeFile }) {
  return executeConversationCli(executable, portableRoot, {
    action: "conversation.subagent.edge", conversationId, callerMembershipId, targetMembershipId,
  }, executeFile);
}

export async function executeConversationCli(executable, portableRoot, request, executeFile = execFileAsync) {
  const { stdout } = await executeFile(executable, [
    "conversation", "execute", "--require-running-host", "--stdin-json", JSON.stringify(request),
  ], {
    env: { ...process.env, LICOUP_HOME: portableRoot }, maxBuffer: 256 * 1024, encoding: "utf8",
  });
  const value = JSON.parse(stdout);
  if (value?.ok !== true) throw new DownstreamVerificationError("target_membership_unavailable");
  return value.result;
}

export function parseDownstreamArgs(argv) {
  if (argv.length === 0) return Object.freeze({ live: false });
  if (argv.length === 1 && argv[0] === "--live") return Object.freeze({ live: true });
  throw new DownstreamVerificationError("argument_unsupported");
}

export function failedDownstreamReceipt(error, live = false) {
  return {
    route: "downstream",
    mode: live ? "live" : "preflight",
    result: "failed",
    reason: safeFailure(error?.code ?? error?.message) || "direct_mcp_failed",
  };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  let live = false;
  let receipt;
  try {
    ({ live } = parseDownstreamArgs(process.argv.slice(2)));
    receipt = await runDownstream({ live });
  } catch (error) {
    receipt = failedDownstreamReceipt(error, live);
  }
  process.stdout.write(`${JSON.stringify(receipt)}\n`);
  process.exitCode = Array.isArray(receipt.targets)
    && receipt.targets.every((target) => target.result === "passed" || target.result === "ready")
    ? 0
    : 1;
}
