import { performance } from "node:perf_hooks";
import {
  AcpClient,
  createPromptQuiescenceBudget,
  promptNotificationError,
  promptQuiescenceExpiration,
  promptQuietMs,
  resetPromptQuiescenceBudget,
} from "../clients/acp-client.mjs";
import { AcceptanceError } from "../errors.mjs";
import { parseSidecarStreamStdout } from "../native/acp-turn.mjs";
import { cleanupSession } from "../session-cleanup.mjs";

function sidecarNdjson(records) {
  return `${records.map((record) => JSON.stringify(record)).join("\n")}\n`;
}

function sidecarStreamParseError(records) {
  try {
    parseSidecarStreamStdout(sidecarNdjson(records));
    return "";
  } catch (error) {
    return error instanceof AcceptanceError ? error.code : "unexpected_failure";
  }
}

export function publicStreamChunkOracleContract() {
  const sessionId = "public-stream-session";
  const done = {
    event: "done",
    ok: true,
    sessionId,
    nativeSessionId: sessionId,
  };
  const validChunk = {
    event: "agent.message.chunk",
    sessionId,
    turnId: "arbitrary-turn",
    payload: { text: "arbitrary single chunk" },
  };
  const malformedChunks = [
    { event: "agent.message.chunk" },
    { ...validChunk, sessionId: 7 },
    { ...validChunk, sessionId: "" },
    { ...validChunk, sessionId: " padded-session " },
    { ...validChunk, turnId: {} },
    { ...validChunk, turnId: "" },
    { ...validChunk, turnId: " padded-turn " },
    { ...validChunk, payload: null },
    { ...validChunk, payload: [] },
    { ...validChunk, payload: "text" },
    { ...validChunk, payload: {} },
    { ...validChunk, payload: { text: 7 } },
    { ...validChunk, payload: { text: "" } },
  ];
  const valid = parseSidecarStreamStdout(sidecarNdjson([validChunk, done]));
  const nonChunks = [
    { ...validChunk, event: "agent.message.completed" },
    { ...validChunk, event: 7 },
    { ...validChunk, event: "" },
  ].map((record) => parseSidecarStreamStdout(sidecarNdjson([record, done])));
  return valid.streamingSeen === true
    && valid.events.filter((event) => event === "agent.message.chunk").length === 1
    && nonChunks.every((result) => result.streamingSeen === false)
    && malformedChunks.every((chunk) => (
      sidecarStreamParseError([chunk, validChunk, done]) === "sidecar_stream_chunk_invalid"
      && sidecarStreamParseError([validChunk, chunk, done])
        === "sidecar_stream_chunk_invalid"
    ))
    && sidecarStreamParseError([
      { ...validChunk, sessionId: "other-session" },
      validChunk,
      done,
    ]) === "sidecar_stream_session_mismatch"
    && sidecarStreamParseError([
      validChunk,
      { ...validChunk, sessionId: "other-session" },
      done,
    ]) === "sidecar_stream_session_mismatch";
}

function notification(sessionId, update, additions = {}) {
  return {
    jsonrpc: "2.0",
    method: "session/update",
    params: { sessionId, update, _meta: { fixture: true } },
    ...additions,
  };
}

function validConfigOptions() {
  return [
    {
      id: "pace",
      name: "Pace",
      description: "Synthetic select option",
      type: "select",
      currentValue: "steady",
      options: [{
        value: "steady",
        name: "Steady",
        futureValueField: true,
      }],
      futureConfigField: true,
    },
    {
      id: "guarded",
      name: "Guarded",
      type: "boolean",
      currentValue: true,
      futureConfigField: true,
    },
    {
      id: "profile",
      name: "Profile",
      type: "select",
      currentValue: "safe",
      options: [{
        group: "recommended",
        name: "Recommended",
        options: [{ value: "safe", name: "Safe" }],
        futureGroupField: true,
      }],
    },
  ];
}

export function sessionUpdateOracleContract() {
  const sessionId = "session-oracle";
  const supportedContentBlocks = [
    { type: "text", text: "fixture", futureContentField: true },
    {
      type: "image",
      data: "aW1hZ2U=",
      mimeType: "image/png",
      futureContentField: true,
    },
    {
      type: "audio",
      data: "YXVkaW8=",
      mimeType: "audio/wav",
      futureContentField: true,
    },
    {
      type: "resource_link",
      uri: "file:///fixture.txt",
      name: "fixture.txt",
      futureContentField: true,
    },
    {
      type: "resource",
      resource: {
        uri: "file:///fixture.txt",
        text: "fixture",
        futureResourceField: true,
      },
      futureContentField: true,
    },
    {
      type: "resource",
      resource: {
        uri: "file:///fixture.bin",
        blob: "Zml4dHVyZQ==",
        futureResourceField: true,
      },
      futureContentField: true,
    },
  ];
  const validUpdates = [
    ...["user_message_chunk", "agent_message_chunk", "agent_thought_chunk"]
      .flatMap((sessionUpdate) => supportedContentBlocks.map((content) => ({
        sessionUpdate,
        content,
        futureField: true,
      }))),
    {
      sessionUpdate: "tool_call",
      toolCallId: "tool-1",
      title: "Inspect fixture",
      kind: "read",
      status: "in_progress",
      content: [{
        type: "content",
        content: { type: "text", text: "synthetic output" },
        futureContentField: true,
      }, {
        type: "terminal",
        terminalId: "terminal-1",
        futureTerminalField: true,
      }, {
        type: "diff",
        path: "/fixture/project/lib.rs",
        oldText: "before",
        newText: "after",
        futureDiffField: true,
      }],
      locations: [{ path: "/fixture/project/lib.rs", line: 7 }],
      futureToolField: true,
    },
    {
      sessionUpdate: "tool_call_update",
      toolCallId: "tool-1",
      title: "Inspect fixture complete",
      status: "completed",
      content: [{
        type: "content",
        content: { type: "text", text: "complete" },
      }],
      futureToolUpdateField: true,
    },
    {
      sessionUpdate: "plan",
      entries: [{
        content: "Validate the fixture",
        priority: "high",
        status: "in_progress",
        futurePlanEntryField: true,
      }],
      futurePlanField: true,
    },
    {
      sessionUpdate: "available_commands_update",
      availableCommands: [{
        name: "review",
        description: "Review a synthetic fixture",
        input: { hint: "fixture path", futureInputField: true },
        futureCommandField: true,
      }],
      futureCommandsField: true,
    },
    {
      sessionUpdate: "current_mode_update",
      currentModeId: "review",
      futureModeField: true,
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: validConfigOptions(),
      futureConfigUpdateField: true,
    },
    {
      sessionUpdate: "session_info_update",
      title: "Synthetic session",
      updatedAt: "2026-01-01T00:00:00Z",
      futureSessionInfoField: true,
    },
    {
      sessionUpdate: "usage_update",
      used: 4,
      size: 128,
      cost: { amount: 0.25, currency: "USD", futureCostField: true },
      futureUsageField: true,
    },
  ];
  const malformedEnvelope = notification(
    sessionId,
    validUpdates[0],
    { unexpectedEnvelopeField: true },
  );
  const invalidParams = notification(sessionId, validUpdates[1]);
  invalidParams.params.unexpectedParamsField = true;
  const invalidMeta = notification(sessionId, validUpdates[1]);
  invalidMeta.params._meta = [];
  const invalidSession = notification(sessionId, validUpdates[1]);
  invalidSession.params.sessionId = " session-oracle ";
  const oversizedSession = notification(sessionId, validUpdates[1]);
  oversizedSession.params.sessionId = "s".repeat(1_025);
  const malformedContentBlocks = [
    {},
    { text: "MISSING-DISCRIMINATOR-CANARY" },
    { type: "future_content", text: "UNSUPPORTED-DISCRIMINATOR-CANARY" },
    { type: "text" },
    { type: "text", text: 7 },
    { type: "image", data: "aW1hZ2U=" },
    { type: "image", data: 7, mimeType: "image/png" },
    { type: "image", data: "aW1hZ2U=", mimeType: 7 },
    { type: "audio", mimeType: "audio/wav" },
    { type: "audio", data: 7, mimeType: "audio/wav" },
    { type: "resource_link", uri: "file:///fixture.txt" },
    { type: "resource_link", uri: 7, name: "fixture.txt" },
    { type: "resource", resource: {} },
    { type: "resource", resource: { uri: 7, text: "fixture" } },
    { type: "resource", resource: { uri: "file:///fixture", text: 7 } },
  ];
  const invalidUpdates = [
    ...malformedContentBlocks.map((content) => ({
      sessionUpdate: "agent_message_chunk",
      content,
    })),
    { sessionUpdate: "tool_call", title: "Inspect" },
    { sessionUpdate: "tool_call", toolCallId: 7, title: "Inspect" },
    { sessionUpdate: "tool_call", toolCallId: "tool-1" },
    { sessionUpdate: "tool_call", toolCallId: "tool-1", title: [] },
    {
      sessionUpdate: "tool_call",
      toolCallId: "tool-1",
      title: "Inspect",
      content: [{ type: "content" }],
    },
    {
      sessionUpdate: "tool_call",
      toolCallId: "tool-1",
      title: "Inspect",
      content: [{ type: "terminal" }],
    },
    {
      sessionUpdate: "tool_call",
      toolCallId: "tool-1",
      title: "Inspect",
      content: [{ type: "diff", path: "/fixture/project/lib.rs" }],
    },
    {
      sessionUpdate: "tool_call",
      toolCallId: "tool-1",
      title: "Inspect",
      locations: [{ path: 7 }],
    },
    { sessionUpdate: "tool_call_update" },
    { sessionUpdate: "tool_call_update", toolCallId: {} },
    { sessionUpdate: "tool_call_update", toolCallId: " tool-1 " },
    { sessionUpdate: "tool_call_update", toolCallId: "tool-1", status: 7 },
    { sessionUpdate: "plan" },
    { sessionUpdate: "plan", entries: {} },
    { sessionUpdate: "plan", entries: [{}] },
    {
      sessionUpdate: "plan",
      entries: [{ content: "Validate", priority: "urgent", status: "pending" }],
    },
    {
      sessionUpdate: "plan",
      entries: [{ content: "Validate", priority: "high", status: "running" }],
    },
    { sessionUpdate: "available_commands_update" },
    { sessionUpdate: "available_commands_update", availableCommands: {} },
    { sessionUpdate: "available_commands_update", availableCommands: [{}] },
    {
      sessionUpdate: "available_commands_update",
      availableCommands: [{ name: "review" }],
    },
    {
      sessionUpdate: "available_commands_update",
      availableCommands: [{ name: 7, description: "Review" }],
    },
    {
      sessionUpdate: "available_commands_update",
      availableCommands: [{ name: "review", description: "Review", input: {} }],
    },
    { sessionUpdate: "current_mode_update" },
    { sessionUpdate: "current_mode_update", currentModeId: " review " },
    { sessionUpdate: "config_option_update" },
    { sessionUpdate: "config_option_update", configOptions: {} },
    { sessionUpdate: "config_option_update", configOptions: [{}] },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{ id: "pace", name: "Pace", currentValue: "steady", options: [] }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "pace",
        name: "Pace",
        type: "future",
        currentValue: true,
      }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "x".repeat(1_025),
        name: "Pace",
        type: "boolean",
        currentValue: true,
      }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "guarded",
        name: "Guarded",
        type: "boolean",
        currentValue: "true",
      }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "pace",
        name: "Pace",
        type: "select",
        currentValue: "steady",
        options: [{ value: "steady" }],
      }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "pace",
        name: "Pace",
        type: "select",
        currentValue: "steady",
        options: [{ name: "Steady" }],
      }],
    },
    {
      sessionUpdate: "config_option_update",
      configOptions: [{
        id: "pace",
        name: "Pace",
        type: "select",
        currentValue: "steady",
        options: [{ group: 7, name: "Normal", options: [] }],
      }],
    },
    { sessionUpdate: "session_info_update", title: 7 },
    { sessionUpdate: "session_info_update", updatedAt: [] },
    { sessionUpdate: "session_info_update", _meta: [] },
    { sessionUpdate: "usage_update", size: 128 },
    { sessionUpdate: "usage_update", used: 4 },
    { sessionUpdate: "usage_update", used: -1, size: 128 },
    { sessionUpdate: "usage_update", used: 4, size: 1.5 },
    {
      sessionUpdate: "usage_update",
      used: 4,
      size: 128,
      cost: { amount: "0.25", currency: "USD" },
    },
    { sessionUpdate: "future_unsupported_kind" },
  ];
  const oversized = notification(sessionId, {
    sessionUpdate: "agent_message_chunk",
    content: { type: "text", text: "x".repeat((1024 * 1024) + 1) },
  });
  return validUpdates.every((update) => (
    promptNotificationError(notification(sessionId, update), sessionId) === ""
  ))
    && promptNotificationError(malformedEnvelope, sessionId)
      === "acp_notification_envelope_invalid"
    && promptNotificationError(invalidParams, sessionId) === "acp_session_update_invalid"
    && promptNotificationError(invalidMeta, sessionId) === "acp_session_update_invalid"
    && promptNotificationError(invalidSession, sessionId) === "acp_session_id_invalid"
    && promptNotificationError(oversizedSession, sessionId) === "acp_session_id_invalid"
    && invalidUpdates.every((update) => (
      promptNotificationError(notification(sessionId, update), sessionId)
        === "acp_session_update_invalid"
    ))
    && promptNotificationError(notification(sessionId, validUpdates[1]), "other-session")
      === "acp_session_mismatch"
    && promptNotificationError(oversized, sessionId) === "acp_message_too_large";
}

export function promptQuiescenceBudgetContract() {
  const promptResponseAt = 1_000;
  const hardDeadlineAt = 1_250;
  const initial = createPromptQuiescenceBudget(promptResponseAt, hardDeadlineAt);
  const reset = resetPromptQuiescenceBudget(initial, 1_090);
  const nearHardDeadline = resetPromptQuiescenceBudget(reset, 1_180);
  const capped = resetPromptQuiescenceBudget(nearHardDeadline, 1_240);
  const afterHardDeadline = resetPromptQuiescenceBudget(capped, 1_260);
  return promptQuietMs === 100
    && initial.quietDeadlineAt === 1_100
    && promptQuiescenceExpiration(initial, 1_100) === "quiet"
    && reset.quietDeadlineAt === 1_190
    && nearHardDeadline.quietDeadlineAt === hardDeadlineAt
    && capped.quietDeadlineAt === hardDeadlineAt
    && promptQuiescenceExpiration(capped, hardDeadlineAt) === "hard"
    && afterHardDeadline.hardDeadlineAt === hardDeadlineAt
    && afterHardDeadline.quietDeadlineAt === hardDeadlineAt;
}

function fixtureClient(context) {
  return new AcpClient(
    context.wrapper.wrapperPath,
    context.config.acpArgs,
    { ...context, environment: context.wrapper.environment },
  );
}

async function createFixtureSession(client, context) {
  await client.initialize();
  const result = await client.request("session/new", {
    cwd: context.cwd,
    mcpServers: [],
  });
  return typeof result?.sessionId === "string" ? result.sessionId : "";
}

export async function exerciseQuiescenceOracle(context) {
  const client = fixtureClient(context);
  let sessionId = "";
  let proof = false;
  try {
    sessionId = await createFixtureSession(client, context);
    if (!sessionId) throw new AcceptanceError("native_session_id_missing");
    const startIndex = client.notifications.length;
    const hardDeadlineAt = performance.now() + context.timeoutMs;
    client.beginPromptNotificationValidation(startIndex, sessionId);
    const result = await client.request("session/prompt", {
      sessionId,
      messageId: "fixture-quiescence-message",
      prompt: [{ type: "text", text: "Reply with exactly 2468" }],
    });
    const responseSequence = client.lastResponseSequence;
    const updates = await client.waitForPromptNotificationQuiescence(
      startIndex,
      sessionId,
      hardDeadlineAt,
    );
    const sequences = client.notificationSequences.slice(startIndex);
    const chunks = updates
      .map((update, index) => ({ update, sequence: sequences[index] }))
      .filter(({ update }) => (
        update?.params?.update?.sessionUpdate === "agent_message_chunk"
      ));
    const delayedChunks = chunks.filter(({ update }) => (
      update?.params?._meta?.fixtureDelivery
        === "after_prompt_response_set_immediate"
    ));
    proof = result?.stopReason === "end_turn"
      && chunks.length === 2
      && chunks.every(({ sequence }) => sequence > responseSequence)
      && delayedChunks.length === 1
      && delayedChunks[0].sequence > responseSequence
      && delayedChunks[0].update.params._meta.fixtureDelayedChunkIndex === 0
      && delayedChunks[0].update.params.update.content.text === "68"
      && chunks.map(({ update }) => update.params.update.content.text).join("") === "2468";
  } catch {
    proof = false;
  } finally {
    await client.close();
  }
  const cleanup = sessionId
    ? await cleanupSession(context, sessionId, context.temporaryDirectory)
    : false;
  return proof && cleanup;
}

export async function exerciseMalformedBeforePromptResponse(context) {
  const client = fixtureClient(context);
  let sessionId = "";
  let code = "";
  let proof = false;
  try {
    sessionId = await createFixtureSession(client, context);
    if (!sessionId) throw new AcceptanceError("native_session_id_missing");
    const startIndex = client.notifications.length;
    const hardDeadlineAt = performance.now() + context.timeoutMs;
    const responseSequenceBeforePrompt = client.lastResponseSequence;
    client.beginPromptNotificationValidation(startIndex, sessionId);
    try {
      await client.request("session/prompt", {
        sessionId,
        messageId: "fixture-malformed-before-response",
        prompt: [{ type: "text", text: "SELFTEST_MALFORMED_BEFORE_PROMPT_RESPONSE" }],
      });
      await client.waitForPromptNotificationQuiescence(
        startIndex,
        sessionId,
        hardDeadlineAt,
      );
    } catch (error) {
      code = error instanceof AcceptanceError ? error.code : "unexpected_failure";
    }
    await new Promise((resolveTick) => setImmediate(resolveTick));
    const laterValidNotificationObserved = client.notifications.some((update) => (
      update?.params?.update?.content?.text === "MUST-NOT-RECOVER"
    ));
    const failedBeforeResponse = client.lastResponseSequence === responseSequenceBeforePrompt;
    const failureStayedMalformed = client.failure === "acp_session_update_invalid";
    proof = code === "acp_session_update_invalid"
      && code !== "acp_request_timeout"
      && laterValidNotificationObserved
      && failedBeforeResponse
      && failureStayedMalformed;
  } catch {
    proof = false;
  } finally {
    await client.close();
  }
  const cleanup = sessionId
    ? await cleanupSession(context, sessionId, context.temporaryDirectory)
    : false;
  return proof && cleanup;
}
