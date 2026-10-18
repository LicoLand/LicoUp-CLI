import { randomUUID } from "node:crypto";
import { existsSync, lstatSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { AcpClient } from "./clients/acp-client.mjs";
import { CopilotSdkRpcClient } from "./clients/copilot-sdk-client.mjs";
import { AcceptanceError, requireFact, stableJson } from "./errors.mjs";
import { withAppServer } from "./native/app-server.mjs";
import { piSessionFiles, piSessionHeader, piSessionPath } from "./native/pi.mjs";
import { runBoundedProcess } from "./process.mjs";

export function collectSessionRecords(value, records = new Map()) {
  if (Array.isArray(value)) {
    for (const entry of value) collectSessionRecords(entry, records);
    return records;
  }
  if (!value || typeof value !== "object") return records;
  const id = [value.id, value.sessionId, value.sessionID].find(
    (candidate) => typeof candidate === "string" && candidate.length > 0,
  );
  if (id) records.set(id, stableJson(value));
  for (const child of Object.values(value)) collectSessionRecords(child, records);
  return records;
}

export function readDisposableKimiHistory(context, sessionId) {
  requireFact(
    typeof sessionId === "string"
      && /^[a-z0-9][a-z0-9._-]{0,255}$/iu.test(sessionId),
    "history_session_id_unsafe",
  );
  const sessionsRoot = join(context.disposableDataRoot, "sessions");
  if (!existsSync(sessionsRoot)) return "";
  const chunks = [];
  for (const workspaceName of readdirSync(sessionsRoot)) {
    const workspacePath = join(sessionsRoot, workspaceName);
    const workspaceMetadata = lstatSync(workspacePath);
    if (!workspaceMetadata.isDirectory() || workspaceMetadata.isSymbolicLink()) {
      continue;
    }
    const wirePath = join(
      workspacePath,
      sessionId,
      "agents",
      "main",
      "wire.jsonl",
    );
    if (!existsSync(wirePath)) continue;
    const wireMetadata = lstatSync(wirePath);
    requireFact(
      wireMetadata.isFile()
        && !wireMetadata.isSymbolicLink()
        && wireMetadata.size <= context.maxOutputBytes,
      "session_export_limit",
    );
    chunks.push(readFileSync(wirePath, "utf8"));
  }
  return chunks.join("\n");
}

export async function withCopilotSdkRpc(context, operation) {
  const launchVariants = context.copilotSdkLaunchArgs
    ? [context.copilotSdkLaunchArgs]
    : [
      ["--server", "--stdio", "--no-auto-update"],
      ["--headless", "--no-auto-update", "--stdio"],
    ];
  let lastError = new AcceptanceError("copilot_sdk_start_failed");
  for (const launchArgs of launchVariants) {
    const client = new CopilotSdkRpcClient(context, launchArgs);
    try {
      await client.connect();
      context.copilotSdkLaunchArgs = launchArgs;
      try {
        return await operation(client);
      } finally {
        await client.close();
      }
    } catch (error) {
      lastError = error instanceof AcceptanceError ? error : lastError;
      await client.close();
    }
  }
  throw lastError;
}

export async function withOpenClawAcp(context, operation) {
  const client = new AcpClient(
    context.wrapper.wrapperPath,
    context.config.acpArgs,
    { ...context, environment: context.wrapper.environment },
  );
  try {
    const initialized = await client.initialize();
    return await operation(client, initialized);
  } finally {
    await client.close();
  }
}

export function codexThreadListItems(result) {
  if (Array.isArray(result?.threads)) return result.threads;
  if (Array.isArray(result?.data)) return result.data;
  return null;
}

export async function listSessions(context) {
  if (context.config.cleanupKind === "cursor-cli-chat-leaf") {
    const records = new Map();
    for (const sessionId of context.observedSessions || []) {
      if (!context.cleanedSessions?.has(sessionId)) records.set(sessionId, "cursor-cli-session");
    }
    return records;
  }
  if (context.config.cleanupKind === "pi-disposable-session-root") {
    const records = new Map();
    for (const path of piSessionFiles(context.disposableDataRoot)) {
      const header = piSessionHeader(path);
      if (header?.id) records.set(header.id, stableJson({ id: header.id }));
    }
    return records;
  }
  if (context.config.cleanupKind === "disposable-data-root") {
    const records = new Map();
    for (const sessionId of context.observedSessions || []) {
      if (!context.cleanedSessions?.has(sessionId)) records.set(sessionId, "isolated-session");
    }
    return records;
  }
  if (context.config.cleanupKind === "codex-app-server") {
    return withAppServer(context, async (client) => {
      const result = await client.request("thread/list", {});
      const threads = codexThreadListItems(result);
      requireFact(Array.isArray(threads), "codex_app_server_list_unavailable");
      return collectSessionRecords(
        threads.map((thread) => ({
          id: thread?.id || thread?.threadId || thread?.sessionId,
          sessionId: thread?.id || thread?.threadId || thread?.sessionId,
        })),
      );
    });
  }
  if (context.config.cleanupKind === "copilot-sdk") {
    return withCopilotSdkRpc(context, async (client) => {
      const result = await client.request("session.list", {});
      return collectSessionRecords(result?.sessions || []);
    });
  }
  if (context.config.cleanupKind === "openclaw-acp") {
    return withOpenClawAcp(context, async (client) => {
      const result = await client.request("session/list", { cwd: context.cwd });
      return collectSessionRecords(result?.sessions || []);
    });
  }
  const run = await runBoundedProcess(context.binary, context.config.listArgs, {
    cwd: context.cwd,
    environment: context.environment,
    timeoutMs: Math.min(context.timeoutMs, 30_000),
    maxOutputBytes: context.maxOutputBytes,
  });
  requireFact(run.statusCode === 0, "session_list_failed");
  if (context.config.cleanupKind === "openagent-cli") {
    let parsed;
    try {
      parsed = JSON.parse(run.stdout);
    } catch {
      throw new AcceptanceError("session_list_invalid_json");
    }
    return collectSessionRecords(parsed);
  }
  const records = new Map();
  for (const line of run.stdout.split(/\r?\n/u)) {
    const id = line.trim().split(/\s+/u).at(-1) || "";
    if (/^\d{8}_\d{6}_[a-f0-9]{6,}$/iu.test(id)) records.set(id, line);
  }
  return records;
}

export async function officialHistory(context, sessionId, temporaryDirectory) {
  if (context.config.cleanupKind === "pi-disposable-session-root") {
    const path = piSessionPath(context, sessionId);
    const metadata = lstatSync(path);
    requireFact(metadata.size <= context.maxOutputBytes, "session_export_limit");
    return readFileSync(path, "utf8");
  }
  if (context.config.cleanupKind === "disposable-data-root") {
    return readDisposableKimiHistory(context, sessionId);
  }
  if (context.config.cleanupKind === "codex-app-server") {
    return withAppServer(context, async (client) => {
      const result = await client.request("thread/read", {
        threadId: sessionId,
        includeTurns: true,
      });
      const turns = Array.isArray(result?.thread?.turns) ? result.thread.turns : [];
      return turns
        .flatMap((turn) => Array.isArray(turn?.items) ? turn.items : [])
        .filter((item) => item?.type === "agentMessage" && typeof item.text === "string")
        .map((item) => item.text)
        .join("\n");
    });
  }
  if (context.config.cleanupKind === "openagent-cli") {
    const run = await runBoundedProcess(context.binary, context.config.exportArgs(sessionId), {
      cwd: context.cwd,
      environment: context.environment,
      timeoutMs: Math.min(context.timeoutMs, 30_000),
      maxOutputBytes: context.maxOutputBytes,
    });
    requireFact(run.statusCode === 0, "session_export_failed");
    return run.stdout;
  }
  if (context.config.cleanupKind === "hermes-cli") {
    const destination = join(temporaryDirectory, `history-${randomUUID()}.jsonl`);
    const run = await runBoundedProcess(
      context.binary,
      ["sessions", "export", destination, "--session-id", sessionId, "--redact"],
      {
        cwd: context.cwd,
        environment: context.environment,
        timeoutMs: Math.min(context.timeoutMs, 30_000),
        maxOutputBytes: context.maxOutputBytes,
      },
    );
    requireFact(run.statusCode === 0 && existsSync(destination), "session_export_failed");
    requireFact(statSync(destination).size <= context.maxOutputBytes, "session_export_limit");
    return readFileSync(destination, "utf8");
  }
  return "";
}
