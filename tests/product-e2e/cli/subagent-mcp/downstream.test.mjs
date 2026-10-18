import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";
import { verificationModelForAgent } from "../../../../tools/scripts/lib/agent-conversation-verification-models.mjs";
import { acquireLiveLease, APPROVED_TARGET_MODELS, DownstreamVerificationError, directDelegateThroughDiscovery, evaluateCanonicalEffect, prepareVerificationConversation, projectStructuredMcpFailure, readVerificationEdge, resolveTargetFacts, runDownstream, selectApprovedModel } from "./downstream.mjs";
import { createInteropRecord } from "./interop-manifest.mjs";
import { admitDiscoveryDocument, DirectMcpClient, FROZEN_TOOL_NAMES, verifyServiceHealth } from "./streamable-http.mjs";

const execFileAsync = promisify(execFile);
const healthy = async () => ({ result: "passed", reason: "service_healthy" });

const facts = {
  codex: { version: "5.3.0", availableModels: ["auto", ...APPROVED_TARGET_MODELS.codex], runtimeAvailable: true },
  cursor: { version: "2.5.0", availableModels: [...APPROVED_TARGET_MODELS.cursor], runtimeAvailable: true },
  antigravity: { version: "3.7.0", availableModels: [...APPROVED_TARGET_MODELS.antigravity], runtimeAvailable: true },
};
const passedEdge = { inbound: { delegate: true }, outcomes: { delegate: "accepted" }, claimState: "running", dispatchState: "accepted" };

test("default mode is a zero-effect preflight with exact low-cost model order", async () => {
  const effects = { lease: 0, prepare: 0, call: 0, write: 0 };
  const receipt = await runDownstream({ appVersion: "0.1.1", targetFacts: facts, readManifest: () => [],
    verifyHealth: healthy,
    acquireLease: () => { effects.lease += 1; }, prepareConversation: () => { effects.prepare += 1; },
    directDelegate: () => { effects.call += 1; }, persistRecord: () => { effects.write += 1; } });
  assert.equal(receipt.mode, "preflight"); assert.deepEqual(effects, { lease: 0, prepare: 0, call: 0, write: 0 });
  assert.equal(selectApprovedModel("codex", facts.codex.availableModels), verificationModelForAgent("codex"));
  assert.doesNotMatch(JSON.stringify(receipt), /gpt-|composer-|gemini-/u);
  assert.deepEqual(APPROVED_TARGET_MODELS.cursor, [verificationModelForAgent("cursor")]);
  assert.equal(selectApprovedModel("codex", ["gpt-5.4-mini"]), "gpt-5.4-mini");
  assert.equal(selectApprovedModel("codex", ["auto", "gpt-5.9-expensive"]), "");
});

test("live route sends one direct request per unverified target and consumes no Agent output", async () => {
  const calls = []; const writes = []; const healthyCallers = new Set(); let leases = 0;
  const receipt = await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: facts,
    readManifest: () => [], acquireLease: () => { leases += 1; return () => { leases -= 1; }; },
    verifyHealth: async ({ callerAgent }) => { healthyCallers.add(callerAgent); return { result: "passed" }; },
    prepareConversation: async ({ targetAgent, callerAgent, model }) => {
      assert.equal(healthyCallers.size, 2);
      return { targetAgent, callerAgent, model,
        conversationId: `conversation-${targetAgent}`, callerMembershipId: `caller-${targetAgent}`, targetMembershipId: `target-${targetAgent}` };
    },
    directDelegate: async (context) => { calls.push(context); return { membershipId: context.targetMembershipId, accepted: true, runtimeOutput: "privacy-canary" }; },
    readCanonicalEdge: async () => passedEdge, persistRecord: (record) => writes.push(record),
  });
  assert.equal(leases, 0); assert.equal(calls.length, 3); assert.equal(writes.length, 3);
  assert.deepEqual([...healthyCallers].sort(), ["codex", "cursor"]);
  assert.ok(calls.every((call) => call.callerAgent !== call.targetAgent));
  assert.ok(receipt.targets.every((target) => target.result === "passed"));
  assert.doesNotMatch(JSON.stringify(writes), /privacy-canary/u);
});

test("typed MCP failure keeps only closed code, stage, retryability, and recovery", async () => {
  const structuredContent = {
    schemaVersion: "licoup.mcp.error.v1",
    reasonCode: "subagent_dispatch_uncertain",
    stage: "persistent-turn/dispatch",
    retryable: true,
    recovery: "reconcile_before_retry",
  };
  assert.deepEqual(projectStructuredMcpFailure({ isError: true, structuredContent }), {
    code: "subagent_dispatch_uncertain",
    stage: "persistent-turn/dispatch",
    retryable: true,
    recovery: "reconcile_before_retry",
  });
  assert.equal(projectStructuredMcpFailure({
    isError: true,
    structuredContent: { ...structuredContent, rawOutput: "privacy-canary" },
  }), null);
  assert.equal(projectStructuredMcpFailure({
    isError: true,
    structuredContent: { ...structuredContent, stage: "private/path" },
  }), null);
});

test("live receipt projects typed failure while Manifest Notes keeps only its reason code", async () => {
  const writes = [];
  const failure = {
    code: "subagent_dispatch_uncertain", stage: "persistent-turn/dispatch",
    retryable: true, recovery: "reconcile_before_retry",
  };
  const receipt = await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: facts,
    readManifest: () => [], acquireLease: () => () => {}, verifyHealth: healthy,
    prepareConversation: async ({ targetAgent, callerAgent }) => ({
      targetAgent, callerAgent, conversationId: `c-${targetAgent}`,
      callerMembershipId: `caller-${targetAgent}`, targetMembershipId: `target-${targetAgent}`,
    }),
    directDelegate: async (context) => {
      if (context.targetAgent === "cursor") {
        throw new DownstreamVerificationError(failure.code, failure);
      }
      return { membershipId: context.targetMembershipId, accepted: true };
    },
    readCanonicalEdge: async () => passedEdge,
    persistRecord: (record) => writes.push(record),
  });
  assert.deepEqual(receipt.targets.find((target) => target.targetAgent === "cursor")?.failure, failure);
  const row = writes.find((record) => record.targetAgent === "cursor");
  assert.equal(row.notes, "subagent_dispatch_uncertain");
  assert.doesNotMatch(JSON.stringify(row), /persistent-turn|retry_after|reconcile_before/u);
});

test("passing target-version records skip before Conversation creation or payment", async () => {
  let prepare = 0; let paid = 0; let reads = 0;
  const records = Object.entries(facts).map(([targetAgent, value]) => createInteropRecord({ appVersion: "0.1.1",
    callerAgent: targetAgent === "codex" ? "cursor" : "codex", callerAgentVersion: "1.0.0",
    targetAgent, targetAgentVersion: value.version, results: "passed", notes: "" }));
  const receipt = await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: facts,
    readManifest: () => { reads += 1; return records; }, acquireLease: () => () => {},
    prepareConversation: () => { prepare += 1; }, directDelegate: () => { paid += 1; } });
  assert.equal(reads, 2); assert.equal(prepare, 0); assert.equal(paid, 0);
  assert.ok(receipt.targets.every((target) => target.skip === true));
});

test("unavailable approved model fails before paid work", async () => {
  let effects = 0; const unavailable = structuredClone(facts); unavailable.codex.availableModels = ["auto"];
  const writes = [];
  await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: unavailable, readManifest: () => [],
    acquireLease: () => () => {}, verifyHealth: healthy, prepareConversation: async ({ targetAgent, callerAgent }) => {
      if (targetAgent === "codex") effects += 1;
      return { targetAgent, callerAgent, callerMembershipId: `c-${targetAgent}`, targetMembershipId: `t-${targetAgent}` };
    }, directDelegate: async (context) => { if (context.targetAgent === "codex") effects += 1; return { membershipId: context.targetMembershipId, accepted: true }; },
    readCanonicalEdge: async () => passedEdge, persistRecord: (record) => writes.push(record) });
  assert.equal(effects, 0); assert.equal(writes.find((row) => row.targetAgent === "codex").notes, "approved_model_unavailable");
});

test("unhealthy caller service and rejected MCP receipts fail before false pass evidence", async () => {
  let codexPrepared = 0; let edges = 0; const writes = [];
  await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: facts, readManifest: () => [],
    acquireLease: () => () => {},
    verifyHealth: async ({ callerAgent }) => ({ result: callerAgent === "cursor" ? "failed" : "passed" }),
    prepareConversation: async ({ targetAgent, callerAgent }) => {
      if (targetAgent === "codex") codexPrepared += 1;
      return { targetAgent, callerAgent, conversationId: `c-${targetAgent}`,
        callerMembershipId: `caller-${targetAgent}`, targetMembershipId: `target-${targetAgent}` };
    },
    directDelegate: async (context) => ({ membershipId: context.targetMembershipId, accepted: false }),
    readCanonicalEdge: async () => { edges += 1; return passedEdge; },
    persistRecord: (record) => writes.push(record),
  });
  assert.equal(codexPrepared, 0);
  assert.equal(edges, 0);
  assert.equal(writes.find((row) => row.targetAgent === "codex").notes, "service_unavailable");
  assert.ok(writes.filter((row) => row.targetAgent !== "codex")
    .every((row) => row.notes === "direct_mcp_rejected"));
});

test("unsafe or missing Agent versions stop before health, Conversation, payment, or Manifest writes", async () => {
  const unsafe = structuredClone(facts);
  unsafe.cursor.version = "not-a-version";
  unsafe.antigravity.version = "";
  let effects = 0;
  const receipt = await runDownstream({ live: true, appVersion: "0.1.1", targetFacts: unsafe,
    readManifest: () => [], acquireLease: () => () => {},
    verifyHealth: async () => { effects += 1; return { result: "passed" }; },
    prepareConversation: () => { effects += 1; }, directDelegate: () => { effects += 1; },
    persistRecord: () => { effects += 1; },
  });
  assert.equal(effects, 0);
  assert.equal(receipt.targets.find((row) => row.targetAgent === "codex").reason, "caller_version_unavailable");
  assert.equal(receipt.targets.find((row) => row.targetAgent === "cursor").reason, "target_version_unavailable");
});

test("pass requires inbound, durable claim, selected target, and PersistentTurn dispatch", () => {
  const base = { edge: passedEdge, targetMembershipId: "target", selectedMembershipId: "target" };
  assert.equal(evaluateCanonicalEffect(base), "");
  assert.equal(evaluateCanonicalEffect({ ...base, edge: { ...passedEdge, inbound: { delegate: false } } }), "inbound_delegate_missing");
  assert.equal(evaluateCanonicalEffect({ ...base, edge: { ...passedEdge, claimState: null } }), "dispatch_claim_missing");
  assert.equal(evaluateCanonicalEffect({ ...base, edge: { ...passedEdge, claimState: "failed" } }), "dispatch_claim_missing");
  assert.equal(evaluateCanonicalEffect({ ...base, edge: { ...passedEdge, dispatchState: null } }), "target_dispatch_missing");
  assert.equal(evaluateCanonicalEffect({ ...base, edge: { ...passedEdge, dispatchState: "cancelled" } }), "target_dispatch_missing");
  assert.equal(evaluateCanonicalEffect({ ...base, targetMembershipId: "other" }), "target_membership_mismatch");
});

test("exclusive untracked lease rejects contention and is never timeout-broken", () => {
  const root = mkdtempSync(join(tmpdir(), "lico-live-lease-")); const path = join(root, "lease");
  try { const release = acquireLiveLease(path); assert.throws(() => acquireLiveLease(path), /verification_in_progress/u); release(); const again = acquireLiveLease(path); again(); }
  finally { rmSync(root, { recursive: true, force: true }); }
});

test("direct HTTP admission accepts only exact loopback discovery and supervisor tokens", () => {
  const token = "a".repeat(64);
  assert.doesNotThrow(() => new DirectMcpClient({ endpoint: "http://127.0.0.1:34567/mcp", token }));
  for (const endpoint of [
    "http://localhost:34567/mcp",
    "http://127.1:34567/mcp",
    "http://127.0.0.1:034567/mcp",
    "http://127.0.0.1:0/mcp",
    "http://127.0.0.1:65536/mcp",
    "http://127.0.0.1/mcp",
    "https://127.0.0.1:34567/mcp",
    "http://127.0.0.1:34567/mcp?token=unsafe",
    "http://user@127.0.0.1:34567/mcp",
  ]) assert.throws(() => new DirectMcpClient({ endpoint, token }), /discovery_endpoint_invalid/u);
  assert.throws(() => new DirectMcpClient({
    endpoint: "http://127.0.0.1:34567/mcp", token: token.slice(1),
  }), /discovery_token_invalid/u);
  const discovery = {
    schemaVersion: "licoup.subagent-mcp.discovery.v1",
    controlToken: "f".repeat(64),
    endpoint: "http://127.0.0.1:34567/mcp",
    generation: "b".repeat(32),
    tokens: { antigravity: token, codex: token, cursor: token },
  };
  assert.equal(admitDiscoveryDocument(discovery), discovery);
  assert.throws(() => admitDiscoveryDocument({ ...discovery, extra: true }), /discovery_invalid/u);
});

test("direct client initializes, lists, and sends exactly one authenticated delegate call", async () => {
  const root = mkdtempSync(join(tmpdir(), "lico-direct-http-"));
  const discoveryDir = join(root, "client-state", "subagent-mcp");
  mkdirSync(discoveryDir, { recursive: true });
  writeFileSync(join(discoveryDir, "discovery.json"), JSON.stringify({
    schemaVersion: "licoup.subagent-mcp.discovery.v1",
    controlToken: "f".repeat(64),
    endpoint: "http://127.0.0.1:34567/mcp",
    generation: "c".repeat(32),
    tokens: { antigravity: "d".repeat(64), codex: "a".repeat(64), cursor: "e".repeat(64) },
  }));
  const exchanges = [];
  const fetchImpl = async (_url, request) => {
    const body = request.body ? JSON.parse(request.body) : null;
    exchanges.push({ method: request.method, body, headers: request.headers, redirect: request.redirect });
    if (request.method === "DELETE") return new Response(null, { status: 204 });
    const result = body.method === "initialize"
      ? { protocolVersion: "2025-06-18", serverInfo: { name: "lico-up-subagents", version: "0.14.0" } }
      : body.method === "tools/list"
        ? { tools: FROZEN_TOOL_NAMES.map((name) => ({ name })) }
        : {
          content: [{ type: "text", text: "ignored" }],
          structuredContent: {
            schemaVersion: "licoup.subagent.receipt.v3",
            operation: "subagent.delegate", agentId: "cursor",
            conversationId: "conversation-fixture", membershipId: "target-fixture",
            dispatchId: "dispatch-fixture", state: "accepted", accepted: true,
          },
          isError: false,
        };
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result }), {
      status: 200, headers: { "content-type": "application/json", "mcp-session-id": "b".repeat(32),
        "mcp-protocol-version": "2025-06-18" },
    });
  };
  try {
    const receipt = await directDelegateThroughDiscovery({ portableRoot: root, callerAgent: "codex", targetAgent: "cursor",
      conversationId: "conversation-fixture", callerMembershipId: "caller-fixture",
      targetMembershipId: "target-fixture", model: "composer-2.5", fetchImpl });
    assert.deepEqual(receipt, { membershipId: "target-fixture", accepted: true });
    assert.equal(exchanges.filter((exchange) => exchange.body?.method === "tools/call").length, 1);
    const call = exchanges.find((exchange) => exchange.body?.method === "tools/call");
    assert.equal(call.body.params.name, "lico_subagent_delegate");
    assert.equal(call.headers["x-licoup-conversation-id"], "conversation-fixture");
    assert.equal(call.headers["x-licoup-membership-id"], "caller-fixture");
    assert.ok(exchanges.every((exchange) => exchange.redirect === "error"));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("direct client preserves a safe structured target failure without retry", async () => {
  const root = mkdtempSync(join(tmpdir(), "lico-direct-failure-"));
  const discoveryDir = join(root, "client-state", "subagent-mcp");
  mkdirSync(discoveryDir, { recursive: true });
  writeFileSync(join(discoveryDir, "discovery.json"), JSON.stringify({
    schemaVersion: "licoup.subagent-mcp.discovery.v1",
    controlToken: "f".repeat(64),
    endpoint: "http://127.0.0.1:34567/mcp",
    generation: "c".repeat(32),
    tokens: { antigravity: "d".repeat(64), codex: "a".repeat(64), cursor: "e".repeat(64) },
  }));
  let toolCalls = 0;
  const fetchImpl = async (_url, request) => {
    if (request.method === "DELETE") return new Response(null, { status: 204 });
    const body = JSON.parse(request.body);
    if (body.method === "tools/call") toolCalls += 1;
    const result = body.method === "initialize"
      ? { protocolVersion: "2025-06-18", serverInfo: { name: "lico-up-subagents", version: "0.14.0" } }
      : body.method === "tools/list"
        ? { tools: FROZEN_TOOL_NAMES.map((name) => ({ name })) }
        : {
          content: [], isError: true,
          structuredContent: {
            schemaVersion: "licoup.mcp.error.v1",
            reasonCode: "subagent_dispatch_uncertain",
            stage: "persistent-turn/dispatch",
            retryable: true,
            recovery: "reconcile_before_retry",
          },
        };
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result }), {
      status: 200,
      headers: { "content-type": "application/json", "mcp-session-id": "b".repeat(32),
        "mcp-protocol-version": "2025-06-18" },
    });
  };
  try {
    await assert.rejects(
      directDelegateThroughDiscovery({ portableRoot: root, callerAgent: "codex", targetAgent: "cursor",
        conversationId: "conversation-fixture", callerMembershipId: "caller-fixture",
        targetMembershipId: "target-fixture", model: "composer-2.5", fetchImpl }),
      (error) => {
        assert.equal(error.code, "subagent_dispatch_uncertain");
        assert.deepEqual(error.failure, {
          code: "subagent_dispatch_uncertain", stage: "persistent-turn/dispatch",
          retryable: true, recovery: "reconcile_before_retry",
        });
        return true;
      },
    );
    assert.equal(toolCalls, 1);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("malformed initialized responses still close their allocated MCP session", async () => {
  const methods = [];
  const fetchImpl = async (_url, request) => {
    methods.push(request.method);
    if (request.method === "DELETE") return new Response(null, { status: 204 });
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: 999, result: {} }), {
      status: 200,
      headers: { "content-type": "application/json", "mcp-session-id": "f".repeat(32), "mcp-protocol-version": "2025-06-18" },
    });
  };
  const client = new DirectMcpClient({
    endpoint: "http://127.0.0.1:34567/mcp", token: "a".repeat(64), fetchImpl,
  });
  await assert.rejects(verifyServiceHealth(client), /mcp_application_failed/u);
  assert.deepEqual(methods, ["POST", "DELETE"]);
});

test("every MCP response must return the stable session and JSON media type", async () => {
  for (const omission of ["session", "content-type"]) {
    const methods = [];
    const fetchImpl = async (_url, request) => {
      methods.push(request.method);
      if (request.method === "DELETE") return new Response(null, { status: 204 });
      const body = JSON.parse(request.body);
      const headers = {
        "content-type": "application/json",
        "mcp-session-id": "f".repeat(32),
        "mcp-protocol-version": "2025-06-18",
      };
      if (body.method === "tools/list") {
        if (omission === "session") delete headers["mcp-session-id"];
        else delete headers["content-type"];
      }
      const result = body.method === "initialize"
        ? { protocolVersion: "2025-06-18", serverInfo: { name: "lico-up-subagents", version: "0.14.0" } }
        : { tools: FROZEN_TOOL_NAMES.map((name) => ({ name })) };
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result }), {
        status: 200, headers,
      });
    };
    const client = new DirectMcpClient({
      endpoint: "http://127.0.0.1:34567/mcp", token: "a".repeat(64), fetchImpl,
    });
    await assert.rejects(verifyServiceHealth(client), /mcp_(?:session_missing|exchange_failed)/u);
    assert.deepEqual(methods, ["POST", "POST", "DELETE"]);
  }
});

test("preflight resolves target versions and model inventories through existing LicoUp surfaces", async () => {
  const invocations = [];
  const resolved = await resolveTargetFacts({ executable: "fixture-cli", portableRoot: "fixture-root",
    executeJson: async (args) => {
      invocations.push(args);
      if (args[0] === "targets") {
        return { ok: true, results: Object.entries(facts).map(([targetId, value]) => ({
          targetId, ok: true, candidate: {
            status: "detected",
            modelCatalog: { models: value.availableModels.map((name) => ({ name })) },
            adapterCapabilities: { conversationReadiness: "unverified" },
            supportedActions: ["runtime.message.send"],
          },
        })) };
      }
      const agent = args.at(-1);
      return { ok: true, cards: [{ id: agent, version: facts[agent].version }] };
    } });
  assert.deepEqual(resolved, facts);
  assert.equal(invocations.length, 4);
  assert.ok(invocations[0].includes("--enable-agent-cli-model-lookup"));
  assert.doesNotMatch(JSON.stringify(resolved), /binaryPath|configPath|token|prompt/iu);
});

test("conversation preparation and edge readback require the running host without invoking an Agent", async () => {
  const invocations = [];
  const executeFile = async (executable, args, options) => {
    invocations.push({ executable, args, options });
    const request = JSON.parse(args.at(-1));
    const result = request.action === "conversation.create"
      ? {
          id: "conversation-fixture",
          memberships: [
            { id: "caller-fixture", principal: { agentId: "codex" } },
            { id: "target-fixture", principal: { agentId: "cursor" } },
          ],
        }
      : passedEdge;
    return { stdout: JSON.stringify({ ok: true, result }) };
  };

  const prepared = await prepareVerificationConversation({
    portableRoot: "fixture-root", executable: "licoup-fixture", callerAgent: "codex",
    targetAgent: "cursor", executeFile,
  });
  await readVerificationEdge({ ...prepared, portableRoot: "fixture-root", executable: "licoup-fixture", executeFile });

  assert.equal(invocations.length, 2);
  assert.deepEqual(invocations.map(({ args }) => args.slice(0, 4)), [
    ["conversation", "execute", "--require-running-host", "--stdin-json"],
    ["conversation", "execute", "--require-running-host", "--stdin-json"],
  ]);
  assert.ok(invocations.every(({ executable }) => executable === "licoup-fixture"));
  assert.ok(invocations.every(({ args }) => !args.includes("codex") && !args.includes("cursor")));
});

test("target-fact admission fails closed on malformed owned CLI projections", async () => {
  const resolved = await resolveTargetFacts({
    executable: "fixture-cli",
    portableRoot: "fixture-root",
    executeJson: async (args) => args[0] === "targets"
      ? { ok: true, results: { malformed: true } }
      : { ok: true, cards: { malformed: true } },
  });
  for (const agent of ["codex", "cursor", "antigravity"]) {
    assert.deepEqual(resolved[agent], {
      version: "", availableModels: [], runtimeAvailable: false,
    });
  }
});

test("approved primaries come from the shared verification-model authority", () => {
  for (const agent of ["codex", "cursor", "antigravity"]) {
    assert.equal(APPROVED_TARGET_MODELS[agent][0], verificationModelForAgent(agent));
  }
  assert.deepEqual(APPROVED_TARGET_MODELS.codex.slice(1), ["gpt-5.4-mini"]);
});

test("unsupported direct arguments fail with one privacy-safe receipt", async () => {
  const script = fileURLToPath(new URL("./downstream.mjs", import.meta.url));
  let stdout = "";
  try {
    await execFileAsync(process.execPath, [script, "--force"], { encoding: "utf8" });
    assert.fail("unsupported argument unexpectedly passed");
  } catch (error) {
    stdout = String(error?.stdout ?? "");
  }
  assert.deepEqual(JSON.parse(stdout), {
    route: "downstream", mode: "preflight", result: "failed", reason: "argument_unsupported",
  });
  assert.doesNotMatch(stdout, /\/Users\/|\/private\/|Bearer|token|prompt|conversation/iu);
});
