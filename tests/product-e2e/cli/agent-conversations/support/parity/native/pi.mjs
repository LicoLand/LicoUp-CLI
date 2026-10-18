import { existsSync, lstatSync, openSync, closeSync, readSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { PiRpcClient } from "../clients/pi-rpc-client.mjs";
import { requireFact } from "../errors.mjs";

export function sessionSettings(result, cwd) {
  const options = Array.isArray(result?.configOptions) ? result.configOptions : [];
  const optionValue = (ids) => {
    const option = options.find((candidate) => ids.includes(candidate?.id));
    if (!option) return null;
    if (Object.hasOwn(option, "currentValue")) return option.currentValue;
    if (Object.hasOwn(option, "value")) return option.value;
    return null;
  };
  return {
    cwd,
    model: optionValue(["model"]) ?? result?.models?.currentModelId ?? null,
    reasoningEffort: optionValue(["reasoning_effort", "variant", "thinking"]) ?? null,
    mode: optionValue(["mode"]) ?? result?.modes?.currentModeId ?? null,
    runtimeAgent: optionValue(["agent"]) ?? null,
    allowAll: optionValue(["allow_all"]),
  };
}

export function arcSettings(result) {
  const effective = result?.effective || {};
  return {
    cwd: effective.cwd ?? result?.workingDirectory ?? result?.cwd ?? null,
    model: effective.model ?? result?.model ?? null,
    reasoningEffort: effective.reasoningEffort ?? result?.reasoningEffort ?? null,
    mode: effective.mode ?? null,
    runtimeAgent: effective.runtimeAgent ?? null,
    allowAll: effective.allowAll ?? null,
  };
}

export function notificationTexts(notifications, expectedSessionId = "") {
  const matched = [];
  let sessionMismatch = false;
  for (const notification of notifications) {
    if (notification?.method !== "session/update") continue;
    const notificationSessionId = notification?.params?.sessionId;
    if (expectedSessionId && notificationSessionId !== expectedSessionId) {
      sessionMismatch = true;
      continue;
    }
    const update = notification?.params?.update;
    const kind = update?.sessionUpdate;
    if (["agent_message_chunk", "agent_message"].includes(kind)) {
      const text = update?.content?.text;
      if (typeof text === "string") matched.push(text);
    }
  }
  return { text: matched.join(""), sessionMismatch };
}

export function piSessionFiles(root, limit = 4096) {
  const files = [];
  const stack = [root];
  while (stack.length > 0 && files.length < limit) {
    const directory = stack.pop();
    if (!existsSync(directory)) continue;
    for (const name of readdirSync(directory).sort()) {
      const path = join(directory, name);
      const metadata = lstatSync(path);
      if (metadata.isSymbolicLink()) continue;
      if (metadata.isDirectory()) stack.push(path);
      else if (metadata.isFile() && name.endsWith(".jsonl")) files.push(path);
      if (files.length >= limit) break;
    }
  }
  return files;
}

export function piSessionHeader(path) {
  const metadata = lstatSync(path);
  requireFact(metadata.isFile() && !metadata.isSymbolicLink(), "pi_session_file_unsafe");
  const bytes = Buffer.alloc(64 * 1024);
  const descriptor = openSync(path, "r");
  let count;
  try {
    count = readSync(descriptor, bytes, 0, bytes.length, 0);
  } finally {
    closeSync(descriptor);
  }
  const newline = bytes.subarray(0, count).indexOf(0x0a);
  if (newline < 0) return null;
  const first = bytes.subarray(0, newline).toString("utf8").replace(/\r$/u, "");
  try {
    const header = JSON.parse(first);
    return header?.type === "session" && typeof header?.id === "string" ? header : null;
  } catch {
    return null;
  }
}

export function piSessionPath(context, sessionId) {
  const matches = piSessionFiles(context.disposableDataRoot)
    .filter((path) => piSessionHeader(path)?.id === sessionId);
  requireFact(matches.length === 1, matches.length === 0
    ? "pi_session_not_found"
    : "pi_session_identity_ambiguous");
  return matches[0];
}

export function piModelIdentity(model) {
  if (!model || typeof model !== "object") return null;
  const provider = typeof model.provider === "string" ? model.provider : "";
  const id = typeof model.id === "string" ? model.id
    : (typeof model.modelId === "string" ? model.modelId : "");
  return provider && id ? `${provider}/${id}` : null;
}

export function piSettings(state, cwd) {
  return {
    cwd,
    model: piModelIdentity(state?.model),
    reasoningEffort: typeof state?.thinkingLevel === "string" ? state.thinkingLevel : null,
    mode: null,
    runtimeAgent: null,
    allowAll: null,
  };
}

export function piMessageText(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(piMessageText).join("");
  if (!value || typeof value !== "object") return "";
  if (value.type === "text" && typeof value.text === "string") return value.text;
  return piMessageText(value.content || value.message || []);
}

export async function withPiRpc(context, operation) {
  const client = new PiRpcClient(context.wrapper.wrapperPath, {
    ...context,
    environment: context.wrapper.environment,
  });
  try {
    return await operation(client);
  } finally {
    await client.close();
  }
}

export async function nativePiTurn(context, requestedSessionId, prompt) {
  return withPiRpc(context, async (client) => {
    if (requestedSessionId) {
      const switched = await client.request("switch_session", {
        sessionPath: piSessionPath(context, requestedSessionId),
      });
      requireFact(switched?.cancelled !== true, "pi_session_switch_cancelled");
    }
    const state = await client.request("get_state");
    const sessionId = state?.sessionId || "";
    requireFact(typeof sessionId === "string" && sessionId.length > 0, "native_session_id_missing");
    requireFact(!requestedSessionId || sessionId === requestedSessionId, "native_session_identity_mismatch");
    context.observedSessions?.add(sessionId);
    const eventStart = client.events.length;
    await client.request("prompt", { message: prompt });
    await client.waitForEvent((event) => event?.type === "agent_settled");
    const final = await client.request("get_last_assistant_text");
    const output = typeof final?.text === "string" ? final.text.trim() : "";
    requireFact(output.length > 0, "native_final_message_missing");
    const turnEvents = client.events.slice(eventStart);
    requireFact(
      turnEvents.some((event) => event?.type === "message_update"
        && event?.assistantMessageEvent?.type === "text_delta"),
      "native_streaming_missing",
    );
    return {
      sessionId,
      output,
      historyNotifications: [],
      settings: piSettings(state, context.cwd),
      protocolVersion: 1,
      permissionRequests: client.permissionRequests,
      unsupportedRequests: client.unsupportedRequests,
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
    };
  });
}

export async function nativePiReadback(context, sessionId) {
  return withPiRpc(context, async (client) => {
    const switched = await client.request("switch_session", {
      sessionPath: piSessionPath(context, sessionId),
    });
    requireFact(switched?.cancelled !== true, "pi_session_switch_cancelled");
    const state = await client.request("get_state");
    requireFact(state?.sessionId === sessionId, "readback_session_identity_mismatch");
    const result = await client.request("get_messages");
    return {
      text: piMessageText(result?.messages || []),
      settings: piSettings(state, context.cwd),
      boundedOutput: client.outputBytes <= context.maxOutputBytes
        && client.stderrBytes <= context.maxOutputBytes,
    };
  });
}
