import { join } from "node:path";
import { parityEffortForAgent, parityModelForAgent } from "../agent-ids.mjs";
import { AppServerClient } from "../clients/app-server-client.mjs";
import { AcceptanceError, requireFact } from "../errors.mjs";

export function appServerFinalMessage(turn) {
  const items = Array.isArray(turn?.items) ? turn.items : [];
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (items[index]?.type === "agentMessage" && typeof items[index].text === "string") {
      return items[index].text.trim();
    }
  }
  return "";
}

export function appServerSettings(threadResult, cwd, expectedModel = "", expectedEffort = "") {
  return {
    cwd,
    // `turn/start` can override the thread defaults. Parity is defined over
    // the settings of the completed turn, so an explicit turn request wins.
    // The current app-server schema does not expose the resolved default
    // model/effort on Turn. Compare only an explicit acceptance override;
    // otherwise report the selection as runtime-owned instead of inferring it
    // from optional thread response extensions that differ on start/resume.
    model: expectedModel || null,
    reasoningEffort: expectedEffort || null,
    mode: null,
    runtimeAgent: null,
    allowAll: null,
  };
}

export async function withAppServer(context, operation) {
  const client = new AppServerClient(
    context.wrapper.wrapperPath,
    context.config.acpArgs,
    { ...context, environment: context.wrapper.environment },
  );
  try {
    await client.initialize();
    return await operation(client);
  } finally {
    await client.close();
  }
}

export async function runAppServerTurn(client, threadId, prompt, model = "", effort = "") {
  const result = await client.request("turn/start", {
    threadId,
    input: [{ type: "text", text: prompt }],
    ...(model ? { model } : {}),
    ...(effort ? { effort } : {}),
  });
  const turnId = result?.turn?.id;
  requireFact(typeof turnId === "string" && turnId.length > 0, "native_turn_id_missing");
  let completed;
  try {
    completed = await client.waitForNotification(
      (message) => message.method === "turn/completed"
        && message.params?.threadId === threadId
        && message.params?.turn?.id === turnId,
    );
  } catch (error) {
    if (!(error instanceof AcceptanceError)
      || error.code !== "app_server_notification_timeout") throw error;
    const saw = (method) => client.notifications.some((message) => message.method === method);
    const diagnosticMask = [
      saw("turn/started"),
      saw("item/agentMessage/delta"),
      saw("item/completed"),
      saw("turn/completed"),
    ].map((value) => value ? "1" : "0").join("");
    throw new AcceptanceError(`app_server_terminal_timeout_m${diagnosticMask}`);
  }
  const turnStatus = String(completed.params?.turn?.status || "").toLowerCase();
  requireFact(turnStatus === "completed", "native_turn_not_completed");
  const completedItems = client.notifications
    .filter((message) => message.method === "item/completed"
      && message.params?.threadId === threadId
      && message.params?.turnId === turnId)
    .map((message) => message.params?.item)
    .filter(Boolean);
  return {
    ...completed.params.turn,
    items: [
      ...(Array.isArray(completed.params.turn.items) ? completed.params.turn.items : []),
      ...completedItems,
    ],
  };
}

export async function nativeAppServerTurn(context, requestedSessionId, prompt) {
  const forcedModel = context.config.id === "codex"
    ? (context.parityModel || parityModelForAgent("codex"))
    : "";
  const forcedEffort = context.config.id === "codex"
    ? parityEffortForAgent("codex", forcedModel)
    : "";
  return withAppServer(context, async (client) => {
    let threadResult;
    let sessionId = requestedSessionId;
    if (requestedSessionId) {
      threadResult = await client.request("thread/resume", { threadId: requestedSessionId });
      sessionId = threadResult?.thread?.id || requestedSessionId;
    } else {
      threadResult = await client.request("thread/start", {
        cwd: context.cwd,
        ...(forcedModel ? { model: forcedModel } : {}),
      });
      sessionId = threadResult?.thread?.id || "";
    }
    requireFact(typeof sessionId === "string" && sessionId.length > 0, "native_session_id_missing");
    requireFact(
      !requestedSessionId || sessionId === requestedSessionId,
      "native_session_identity_mismatch",
    );
    context.observedSessions?.add(sessionId);
    const turn = await runAppServerTurn(client, sessionId, prompt, forcedModel, forcedEffort);
    const output = appServerFinalMessage(turn);
    requireFact(output.length > 0, "native_final_message_missing");
    return {
      sessionId,
      output,
      historyNotifications: [],
      settings: appServerSettings(
        threadResult,
        context.cwd,
        forcedModel,
        forcedEffort,
      ),
      protocolVersion: 1,
      permissionRequests: client.permissionRequests,
      unsupportedRequests: client.unsupportedRequests,
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
    };
  });
}

export async function nativeAppServerReadback(context, sessionId) {
  const forcedModel = context.config.id === "codex"
    ? (context.parityModel || parityModelForAgent("codex"))
    : "";
  const forcedEffort = context.config.id === "codex"
    ? parityEffortForAgent("codex", forcedModel)
    : "";
  return withAppServer(context, async (client) => {
    const result = await client.request("thread/read", {
      threadId: sessionId,
      includeTurns: true,
    });
    requireFact(result?.thread?.id === undefined || result.thread.id === sessionId, "readback_session_identity_mismatch");
    const turns = Array.isArray(result?.thread?.turns) ? result.thread.turns : [];
    const text = turns
      .flatMap((turn) => Array.isArray(turn?.items) ? turn.items : [])
      .filter((item) => item?.type === "agentMessage" && typeof item.text === "string")
      .map((item) => item.text)
      .join("");
    return {
      text,
      settings: appServerSettings(result, context.cwd, forcedModel, forcedEffort),
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
    };
  });
}
