import { randomUUID } from "node:crypto";
import { performance } from "node:perf_hooks";
import { parityEffortForAgent, parityModelForAgent } from "../agent-ids.mjs";
import { AcpClient } from "../clients/acp-client.mjs";
import { acceptanceMode, sidecarArgs } from "../constants.mjs";
import { AcceptanceError, requireFact, safeErrorCode } from "../errors.mjs";
import { nativeAppServerReadback, nativeAppServerTurn } from "./app-server.mjs";
import { nativeCursorCliReadback, nativeCursorCliTurn } from "./cursor-cli.mjs";
import { nativePiReadback, nativePiTurn, notificationTexts, sessionSettings } from "./pi.mjs";
import { applyAcpVerificationSettings } from "./acp-settings.mjs";
import { runBoundedProcess } from "../process.mjs";

function validPublicStreamIdentifier(value) {
  return typeof value === "string"
    && value.length > 0
    && value === value.trim();
}

async function nativeSidecarCliTurn(context, requestedSessionId, prompt) {
  const request = {
    agent: context.config.id,
    text: prompt,
    workingDirectory: context.cwd,
    binaryPath: context.binary,
    timeoutMs: context.timeoutMs,
    maxStdoutBytes: context.maxOutputBytes,
    maxStderrBytes: context.maxOutputBytes,
    streamEvents: true,
  };
  if (requestedSessionId) request.sessionId = requestedSessionId;
  const forcedModel = parityModelForAgent(context.config.id);
  if (forcedModel) request.model = forcedModel;
  const sidecar = await runSidecar(context, request);
  const sessionId = sidecar.result?.sessionId || sidecar.result?.nativeSessionId || "";
  const output = String(sidecar.result?.output || "").trim();
  requireFact(sessionId.length > 0, "native_session_id_missing");
  requireFact(output.length > 0, "native_final_message_missing");
  context.observedSessions?.add(sessionId);
  return {
    sessionId,
    output,
    historyNotifications: [],
    settings: { cwd: context.cwd, model: forcedModel || null },
    protocolVersion: 1,
    permissionRequests: 0,
    unsupportedRequests: 0,
    boundedOutput: sidecar.boundedOutput === true,
    streamingSeen: sidecar.streamingSeen === true,
    structuredSeen: sidecar.structuredSeen === true,
  };
}

export async function nativeTurn(context, requestedSessionId, prompt) {
  if (context.config.laneFamily === "app-server") {
    return nativeAppServerTurn(context, requestedSessionId, prompt);
  }
  if (context.config.laneFamily === "rpc") {
    return nativePiTurn(context, requestedSessionId, prompt);
  }
  if (context.config.laneFamily === "cli") {
    if (context.config.cliTurnKind === "sidecar") {
      return nativeSidecarCliTurn(context, requestedSessionId, prompt);
    }
    return nativeCursorCliTurn(context, requestedSessionId, prompt);
  }
  const client = new AcpClient(
    context.wrapper.wrapperPath,
    context.config.acpArgs,
    { ...context, environment: context.wrapper.environment },
  );
  try {
    const initializeResult = await client.initialize();
    const historyStart = client.notifications.length;
    const method = requestedSessionId ? "session/load" : "session/new";
    const params = { cwd: context.cwd, mcpServers: [] };
    if (requestedSessionId) params.sessionId = requestedSessionId;
    const sessionResult = await client.request(method, params);
    const sessionId = requestedSessionId || sessionResult?.sessionId || "";
    requireFact(typeof sessionId === "string" && sessionId.length > 0, "native_session_id_missing");
    requireFact(
      !requestedSessionId || sessionResult?.sessionId === undefined || sessionResult.sessionId === requestedSessionId,
      "native_session_identity_mismatch",
    );
    context.observedSessions?.add(sessionId);
    const forcedModel = context.parityModel || parityModelForAgent(context.config.id);
    const selectedSettings = await applyAcpVerificationSettings(client, sessionId, sessionResult, {
      model: forcedModel,
      effort: parityEffortForAgent(context.config.id, forcedModel),
      cwd: context.cwd,
    });
    const historyNotifications = client.notifications.slice(historyStart);
    const promptStart = client.notifications.length;
    const promptHardDeadline = performance.now() + context.timeoutMs;
    client.beginPromptNotificationValidation(promptStart, sessionId);
    const promptResult = await client.request("session/prompt", {
      sessionId,
      messageId: randomUUID(),
      prompt: [{ type: "text", text: prompt }],
    });
    requireFact(promptResult?.stopReason === "end_turn", "native_turn_not_completed");
    const turnNotifications = await client.waitForPromptNotificationQuiescence(
      promptStart,
      sessionId,
      promptHardDeadline,
    );
    const final = notificationTexts(turnNotifications, sessionId);
    requireFact(!final.sessionMismatch, "native_event_session_mismatch");
    requireFact(final.text.length > 0, "native_final_message_missing");
    return {
      sessionId,
      output: final.text.trim(),
      historyNotifications,
      settings: sessionSettings(selectedSettings, context.cwd),
      protocolVersion: initializeResult.protocolVersion,
      permissionRequests: client.permissionRequests,
      unsupportedRequests: client.unsupportedRequests,
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
      streamingSeen: turnNotifications.some((notification) =>
        notification?.params?.update?.sessionUpdate === "agent_message_chunk"),
      structuredSeen: true,
    };
  } finally {
    await client.close();
  }
}

export async function nativeReadback(context, sessionId) {
  if (context.config.laneFamily === "app-server") {
    return nativeAppServerReadback(context, sessionId);
  }
  if (context.config.laneFamily === "rpc") {
    return nativePiReadback(context, sessionId);
  }
  if (context.config.laneFamily === "cli") {
    if (context.config.cliReadbackKind === "none") {
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
    return nativeCursorCliReadback(context, sessionId);
  }
  const client = new AcpClient(
    context.wrapper.wrapperPath,
    context.config.acpArgs,
    { ...context, environment: context.wrapper.environment },
  );
  try {
    await client.initialize();
    const start = client.notifications.length;
    const result = await client.request("session/load", {
      sessionId,
      cwd: context.cwd,
      mcpServers: [],
    });
    requireFact(result?.sessionId === undefined || result.sessionId === sessionId, "readback_session_identity_mismatch");
    const notifications = client.notifications.slice(start);
    const text = notificationTexts(notifications, sessionId);
    requireFact(!text.sessionMismatch, "readback_event_session_mismatch");
    return {
      text: text.text,
      settings: sessionSettings(result, context.cwd),
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
    };
  } finally {
    await client.close();
  }
}

export function parseSidecarStreamStdout(stdout) {
  const events = [];
  const streamRecords = [];
  let result = null;
  for (const line of String(stdout || "").split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let parsed;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      throw new AcceptanceError("sidecar_invalid_json");
    }
    const eventKind = typeof parsed?.event === "string" ? parsed.event : "";
    if (eventKind && eventKind !== "done") {
      events.push(eventKind);
    }
    streamRecords.push(parsed);
    if (parsed?.event === "done" || Object.hasOwn(parsed || {}, "ok")) {
      result = parsed;
    }
  }
  if (!result) {
    throw new AcceptanceError("sidecar_invalid_json");
  }
  const finalSessionId = [result?.nativeSessionId, result?.sessionId, result?.threadId]
    .find(validPublicStreamIdentifier) || "";
  let validStreamingChunks = 0;
  for (const record of streamRecords) {
    if (record?.event !== "agent.message.chunk") continue;
    const validPayload = record.payload !== null
      && typeof record.payload === "object"
      && !Array.isArray(record.payload);
    if (!validPublicStreamIdentifier(record.sessionId)
      || !validPublicStreamIdentifier(record.turnId)
      || !validPayload
      || typeof record.payload.text !== "string"
      || record.payload.text.length === 0) {
      throw new AcceptanceError("sidecar_stream_chunk_invalid");
    }
    if (!validPublicStreamIdentifier(finalSessionId) || record.sessionId !== finalSessionId) {
      throw new AcceptanceError("sidecar_stream_session_mismatch");
    }
    validStreamingChunks += 1;
  }
  const streamingSeen = validStreamingChunks > 0;
  const structuredSeen = events.includes("agent.message.completed")
    && (events.includes("dispatch.turn.completed") || events.includes("dispatch.turn.failed"));
  return { result, events, streamingSeen, structuredSeen };
}

export async function runSidecar(context, request) {
  const streamEvents = request?.streamEvents === true;
  const run = await runBoundedProcess(
    context.sidecar,
    sidecarArgs,
    {
      cwd: context.cwd,
      environment: context.wrapper.environment,
      timeoutMs: context.timeoutMs,
      maxOutputBytes: context.maxOutputBytes,
      stdinText: JSON.stringify({ ...request, acceptanceMode }),
    },
  );
  let result;
  let streamingSeen = false;
  let structuredSeen = false;
  try {
    if (streamEvents) {
      const parsed = parseSidecarStreamStdout(run.stdout);
      result = parsed.result;
      streamingSeen = parsed.streamingSeen;
      structuredSeen = parsed.structuredSeen;
    } else {
      result = JSON.parse(run.stdout);
    }
  } catch (error) {
    if (error instanceof AcceptanceError) throw error;
    throw new AcceptanceError(
      run.statusCode === 0 ? "sidecar_invalid_json" : "sidecar_process_failed",
    );
  }
  if (result?.ok !== true) {
    const code = safeErrorCode(result?.error?.code || "sidecar_rejected");
    throw new AcceptanceError(code);
  }
  requireFact(run.statusCode === 0, "sidecar_process_failed");
  requireFact(result?.schemaVersion === 3, "sidecar_schema_mismatch");
  requireFact(result?.adapterId === context.config.id, "sidecar_adapter_mismatch");
  requireFact(result?.driverId === context.config.driverId, "sidecar_driver_mismatch");
  requireFact(result?.runtimeProtocol === context.config.runtimeProtocol, "sidecar_protocol_mismatch");
  requireFact(
    result?.turnStatus === "end_turn" || result?.turnStatus === "completed",
    "sidecar_turn_not_completed",
  );
  const nativeSessionId = result?.nativeSessionId || result?.sessionId || "";
  if (typeof nativeSessionId === "string" && nativeSessionId.length > 0) {
    context.observedSessions?.add(nativeSessionId);
  }
  return {
    result,
    streamingSeen,
    structuredSeen,
    boundedOutput: run.stdoutBytes <= context.maxOutputBytes
      && run.stderrBytes <= context.maxOutputBytes,
  };
}
