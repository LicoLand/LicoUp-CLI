import { randomUUID } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { agentConfigs, workspaceRoot } from "./constants.mjs";
import { AcceptanceError, digest, requireFact } from "./errors.mjs";
import { withAppServer } from "./native/app-server.mjs";
import { readBoundedJson, requireExactFields } from "./packaging.mjs";
import { createPrivateWrapper, runBoundedProcess, seedDisposableProfile } from "./process.mjs";
import { resolveExecutable } from "./sidecar.mjs";
import { collectSessionRecords, withCopilotSdkRpc, withOpenClawAcp, codexThreadListItems, listSessions } from "./session-query.mjs";

export async function cleanupSession(context, sessionId, temporaryDirectory) {
  try {
    if (context.config.cleanupKind === "cursor-cli-chat-leaf") {
      if (context.config.laneFamily === "cli" && process.env.LICO_FAKE_ACP_STATE) {
        const statePath = process.env.LICO_FAKE_ACP_STATE;
        if (existsSync(statePath)) {
          const state = JSON.parse(readFileSync(statePath, "utf8"));
          delete state.sessions?.[sessionId];
          writeFileSync(statePath, JSON.stringify(state));
        }
      }
      for (const observed of context.observedSessions || []) context.cleanedSessions.add(observed);
      context.cleanedSessions.add(sessionId);
      return true;
    }
    if (context.config.cleanupKind === "antigravity-cli-brain-leaf") {
      for (const observed of context.observedSessions || []) context.cleanedSessions.add(observed);
      context.cleanedSessions.add(sessionId);
      return true;
    }
    if (context.config.cleanupKind === "pi-disposable-session-root") {
      requireFact(
        context.disposableDataRoot && dirname(context.disposableDataRoot) === context.temporaryDirectory,
        "disposable_profile_path_unsafe",
      );
      rmSync(context.disposableDataRoot, { recursive: true, force: true });
      mkdirSync(context.disposableDataRoot, { recursive: true, mode: 0o700 });
      for (const observed of context.observedSessions || []) context.cleanedSessions.add(observed);
      context.cleanedSessions.add(sessionId);
      return true;
    }
    if (context.config.cleanupKind === "disposable-data-root") {
      requireFact(
        context.disposableDataRoot && dirname(context.disposableDataRoot) === context.temporaryDirectory,
        "disposable_profile_path_unsafe",
      );
      rmSync(context.disposableDataRoot, { recursive: true, force: true });
      mkdirSync(context.disposableDataRoot, { recursive: true, mode: 0o700 });
      seedDisposableProfile(context);
      context.cleanedSessions.add(sessionId);
      return true;
    }
    if (context.config.cleanupKind === "codex-app-server") {
      await withAppServer(context, async (client) => {
        await client.request("thread/delete", { threadId: sessionId });
        const result = await client.request("thread/list", {});
        const threads = codexThreadListItems(result);
        requireFact(Array.isArray(threads), "codex_app_server_list_unavailable");
        const remaining = collectSessionRecords(
          threads.map((thread) => ({
            id: thread?.id || thread?.threadId || thread?.sessionId,
            sessionId: thread?.id || thread?.threadId || thread?.sessionId,
          })),
        );
        requireFact(!remaining.has(sessionId), "session_cleanup_not_verified");
      });
      return true;
    }
    if (context.config.cleanupKind === "copilot-sdk") {
      await withCopilotSdkRpc(context, async (client) => {
        await client.request("session.delete", { sessionId });
        const result = await client.request("session.list", {});
        requireFact(
          !collectSessionRecords(result?.sessions || []).has(sessionId),
          "session_cleanup_not_verified",
        );
      });
      return true;
    }
    if (context.config.cleanupKind === "openclaw-acp") {
      await withOpenClawAcp(context, async (client) => {
        await client.request("session/close", { sessionId });
        // OpenClaw documents duplicate close as part of the ACP lifecycle
        // smoke contract. Requiring it here proves cleanup is idempotent.
        await client.request("session/close", { sessionId });
        const result = await client.request("session/list", {
          cwd: context.cwd,
        });
        requireFact(
          !collectSessionRecords(result?.sessions || []).has(sessionId),
          "session_cleanup_not_verified",
        );
      });
      return true;
    }
    const deletion = await runBoundedProcess(context.binary, context.config.deleteArgs(sessionId), {
      cwd: context.cwd,
      environment: context.environment,
      timeoutMs: Math.min(context.timeoutMs, 30_000),
      maxOutputBytes: context.maxOutputBytes,
    });
    requireFact(deletion.statusCode === 0, "session_cleanup_failed");
    if (context.config.cleanupKind === "openagent-cli") {
      const verification = await runBoundedProcess(context.binary, context.config.exportArgs(sessionId), {
        cwd: context.cwd,
        environment: context.environment,
        timeoutMs: Math.min(context.timeoutMs, 30_000),
        maxOutputBytes: context.maxOutputBytes,
      });
      requireFact(verification.statusCode !== 0, "session_cleanup_not_verified");
    } else {
      const destination = join(temporaryDirectory, `deleted-${randomUUID()}.jsonl`);
      const verification = await runBoundedProcess(
        context.binary,
        ["sessions", "export", destination, "--session-id", sessionId, "--redact"],
        {
          cwd: context.cwd,
          environment: context.environment,
          timeoutMs: Math.min(context.timeoutMs, 30_000),
          maxOutputBytes: context.maxOutputBytes,
        },
      );
      const absent = verification.statusCode !== 0
        || !existsSync(destination)
        || statSync(destination).size === 0;
      requireFact(absent, "session_cleanup_not_verified");
    }
    return true;
  } catch {
    return false;
  }
}

export async function preflightCleanup(context) {
  if (context.config.cleanupKind === "unavailable") {
    return { ready: false, code: context.config.cleanupBlocker };
  }
  if (context.config.cleanupKind === "cursor-cli-chat-leaf") {
    return { ready: true, code: null };
  }
  if (context.config.cleanupKind === "antigravity-cli-brain-leaf") {
    return { ready: true, code: null };
  }
  if (context.config.cleanupKind === "pi-disposable-session-root") {
    if (!context.disposableDataRoot || dirname(context.disposableDataRoot) !== context.temporaryDirectory) {
      return { ready: false, code: "pi_disposable_session_root_unavailable" };
    }
    try {
      mkdirSync(context.disposableDataRoot, { recursive: true, mode: 0o700 });
      chmodSync(context.disposableDataRoot, 0o700);
      return { ready: true, code: null };
    } catch {
      return { ready: false, code: "pi_disposable_session_root_unavailable" };
    }
  }
  if (context.config.cleanupKind === "disposable-data-root") {
    if (!context.disposableDataRoot || dirname(context.disposableDataRoot) !== context.temporaryDirectory) {
      return { ready: false, code: "disposable_profile_unavailable" };
    }
    try {
      mkdirSync(context.disposableDataRoot, { recursive: true, mode: 0o700 });
      context.disposableProfileSeeded = seedDisposableProfile(context);
      return { ready: true, code: null };
    } catch {
      return { ready: false, code: "disposable_profile_unavailable" };
    }
  }
  if (context.config.cleanupKind === "codex-app-server") {
    try {
      await withAppServer(context, async (client) => {
        const result = await client.request("thread/list", {});
        requireFact(Array.isArray(codexThreadListItems(result)), "codex_app_server_list_unavailable");
      });
      return { ready: true, code: null };
    } catch (error) {
      return {
        ready: false,
        code: error instanceof AcceptanceError
          ? error.code
          : "codex_app_server_cleanup_probe_failed",
      };
    }
  }
  if (context.config.cleanupKind === "copilot-sdk") {
    try {
      await withCopilotSdkRpc(context, async (client) => {
        const result = await client.request("session.list", {});
        requireFact(Array.isArray(result?.sessions), "copilot_sdk_list_unavailable");
      });
      return { ready: true, code: null };
    } catch {
      return { ready: false, code: "copilot_sdk_cleanup_probe_failed" };
    }
  }
  if (context.config.cleanupKind === "openclaw-acp") {
    try {
      await withOpenClawAcp(context, async (client, initialized) => {
        const capabilities = initialized?.agentCapabilities?.sessionCapabilities;
        requireFact(capabilities?.list != null, "openclaw_acp_list_unavailable");
        requireFact(capabilities?.resume != null, "openclaw_acp_resume_unavailable");
        requireFact(capabilities?.close != null, "openclaw_acp_close_unavailable");
        const result = await client.request("session/list", { cwd: context.cwd });
        requireFact(Array.isArray(result?.sessions), "openclaw_acp_list_invalid");
      });
      return { ready: true, code: null };
    } catch {
      return { ready: false, code: "openclaw_acp_cleanup_probe_failed" };
    }
  }
  const helpArgs = context.config.cleanupKind === "hermes-cli"
    ? ["sessions", "delete", "--help"]
    : ["session", "delete", "--help"];
  try {
    const help = await runBoundedProcess(context.binary, helpArgs, {
      cwd: context.cwd,
      environment: context.environment,
      timeoutMs: 10_000,
      maxOutputBytes: 256 * 1024,
    });
    if (help.statusCode !== 0) return { ready: false, code: "session_cleanup_interface_unavailable" };
    await listSessions(context);
    return { ready: true, code: null };
  } catch {
    return { ready: false, code: "session_cleanup_probe_failed" };
  }
}

export async function cleanupProductSession(options) {
  const config = agentConfigs[options.agent];
  requireFact(Boolean(config), "agent_not_acp_packaged");
  const receipt = readBoundedJson(
    options.cleanupProductSession,
    4 * 1024,
    "product_session_receipt_invalid",
  );
  requireExactFields(
    receipt,
    new Set(["schemaVersion", "agentId", "nativeSessionId"]),
    "product_session_receipt_unbounded",
  );
  requireFact(
    receipt.schemaVersion === "lico-agent-conversation-product-session-v1"
      && receipt.agentId === config.id
      && typeof receipt.nativeSessionId === "string"
      && receipt.nativeSessionId.length > 0
      && receipt.nativeSessionId.length <= 512,
    "product_session_receipt_incomplete",
  );
  requireFact(config.cleanupKind !== "unavailable", "product_session_cleanup_unavailable");
  const binary = resolveExecutable(options.binary, config);
  requireFact(binary.length > 0, "agent_executable_unavailable");
  const temporaryDirectory = dirname(options.cleanupProductSession);
  const disposableDataRoot = config.disposableEnvironmentKey
    ? String(process.env[config.disposableEnvironmentKey] || "")
    : "";
  const wrapper = createPrivateWrapper(temporaryDirectory, binary);
  wrapper.environment = { ...wrapper.environment, ...process.env };
  const context = {
    config,
    binary,
    wrapper,
    cwd: workspaceRoot,
    environment: process.env,
    temporaryDirectory,
    disposableDataRoot,
    disposableSeedSource: "",
    disposableProfileSeeded: false,
    cleanedSessions: new Set(),
    observedSessions: new Set([receipt.nativeSessionId]),
    timeoutMs: options.timeoutMs,
    maxOutputBytes: options.maxOutputBytes,
    copilotSdkLaunchArgs: null,
  };
  const cleaned = await cleanupSession(
    context,
    receipt.nativeSessionId,
    temporaryDirectory,
  );
  requireFact(cleaned, "product_session_cleanup_failed");
  const remaining = await listSessions(context);
  requireFact(!remaining.has(receipt.nativeSessionId), "product_session_cleanup_not_verified");
  return {
    status: "passed",
    cleanupPassed: true,
    agent: config.id,
    receiptDigest: `sha256:${digest({
      agentId: config.id,
      nativeSessionId: receipt.nativeSessionId,
    })}`,
  };
}
