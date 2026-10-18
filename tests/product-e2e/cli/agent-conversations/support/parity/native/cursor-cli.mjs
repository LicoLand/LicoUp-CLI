import { existsSync, readFileSync } from "node:fs";
import { parityModelForAgent } from "../agent-ids.mjs";
import { requireFact } from "../errors.mjs";
import { runBoundedProcess } from "../process.mjs";

function parseStreamLine(message, state) {
  if (message?.subtype === "init" || message?.type === "init") {
    if (!state.model && typeof message?.model === "string" && message.model.length > 0) {
      // Cursor may report a display label here even when the launch used a
      // stable model slug. Keep the explicit request authoritative so the
      // native and sidecar lanes compare the same selector namespace.
      state.model = message.model;
    }
    if (typeof message?.permissionMode === "string" && message.permissionMode.length > 0) {
      state.permissionMode = message.permissionMode;
    }
  }
  if (message?.type === "assistant") {
    const text = message?.message?.content?.[0]?.text;
    if (typeof text === "string" && text.length > 0) state.output = text;
  }
  if (message?.type === "result" && typeof message?.result === "string" && message.result.length > 0) {
    state.output = message.result;
  }
}

function extractStreamSummary(stdout, fallback) {
  const state = {
    output: "",
    cwd: fallback.cwd,
    model: fallback.model,
    streamingSeen: false,
    structuredSeen: false,
  };
  for (const line of String(stdout || "").split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const message = JSON.parse(trimmed);
      parseStreamLine(message, state);
      if (message?.type === "assistant" || message?.type === "content_block_delta") {
        state.streamingSeen = true;
      }
      if (message?.type === "result") state.structuredSeen = true;
    } catch {
      continue;
    }
  }
  return {
    output: state.output.trim(),
    settings: {
      cwd: state.cwd || fallback.cwd,
      model: state.model || fallback.model,
      reasoningEffort: null,
      mode: null,
      runtimeAgent: null,
      allowAll: null,
    },
    streamingSeen: state.streamingSeen,
    structuredSeen: state.structuredSeen,
  };
}

async function createSession(context) {
  const run = await runBoundedProcess(context.wrapper.wrapperPath, ["create-chat"], {
    cwd: context.cwd,
    environment: context.wrapper.environment,
    timeoutMs: Math.min(context.timeoutMs, 30_000),
    maxOutputBytes: context.maxOutputBytes,
  });
  requireFact(run.statusCode === 0, "native_session_id_missing");
  const sessionId = run.stdout.trim().split(/\r?\n/u).map((line) => line.trim()).find(Boolean) || "";
  requireFact(typeof sessionId === "string" && sessionId.length > 0, "native_session_id_missing");
  context.observedSessions?.add(sessionId);
  return sessionId;
}

function turnArgs(context, sessionId, prompt) {
  const args = [
    ...context.config.acpArgs,
    "--workspace",
    context.cwd,
    "--resume",
    sessionId,
    prompt,
  ];
  const forcedModel = parityModelForAgent(context.config.id);
  if (forcedModel) args.splice(args.length - 1, 0, "--model", forcedModel);
  return args;
}

export async function nativeCursorCliTurn(context, requestedSessionId, prompt) {
  const sessionId = requestedSessionId || await createSession(context);
  const run = await runBoundedProcess(
    context.wrapper.wrapperPath,
    turnArgs(context, sessionId, prompt),
    {
      cwd: context.cwd,
      environment: context.wrapper.environment,
      timeoutMs: context.timeoutMs,
      maxOutputBytes: context.maxOutputBytes,
    },
  );
  requireFact(run.statusCode === 0, "native_turn_not_completed");
  const fallback = {
    cwd: context.cwd,
    model: parityModelForAgent(context.config.id) || null,
  };
  const summary = extractStreamSummary(run.stdout, fallback);
  requireFact(summary.output.length > 0, "native_final_message_missing");
  context.observedSessions?.add(sessionId);
  return {
    sessionId,
    output: summary.output,
    historyNotifications: [],
    settings: summary.settings,
    protocolVersion: 1,
    permissionRequests: 0,
    unsupportedRequests: 0,
    boundedOutput: run.stdoutBytes <= context.maxOutputBytes && run.stderrBytes <= context.maxOutputBytes,
    streamingSeen: summary.streamingSeen,
    structuredSeen: summary.structuredSeen,
  };
}

export async function nativeCursorCliReadback(context, sessionId) {
  const statePath = context.wrapper.environment?.LICO_FAKE_ACP_STATE;
  if (typeof statePath === "string" && statePath.length > 0 && existsSync(statePath)) {
    const state = JSON.parse(readFileSync(statePath, "utf8"));
    const messages = state.sessions?.[sessionId]?.messages || [];
    const text = messages
      .filter((entry) => entry.role === "assistant")
      .map((entry) => entry.text)
      .join("\n");
    return {
      text,
      settings: { cwd: context.cwd, model: parityModelForAgent(context.config.id) || null },
      boundedOutput: true,
      readbackAvailable: true,
    };
  }
  return {
    text: "",
    settings: {
      cwd: null,
      model: null,
      reasoningEffort: null,
      mode: null,
      runtimeAgent: null,
      allowAll: null,
    },
    boundedOutput: true,
    readbackAvailable: false,
  };
}
