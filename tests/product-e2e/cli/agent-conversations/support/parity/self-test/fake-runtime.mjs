import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

export const fakeRuntimeSource = String.raw`#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname } from "node:path";
import { createInterface } from "node:readline";

const statePath = process.env.LICO_FAKE_ACP_STATE;
const load = () => {
  const defaults = { counter: 0, sessions: {} };
  if (!existsSync(statePath)) return { ...defaults };
  const parsed = JSON.parse(readFileSync(statePath, "utf8"));
  return {
    ...defaults,
    ...parsed,
    counter: Number(parsed?.counter || 0),
    sessions: parsed?.sessions && typeof parsed.sessions === "object" && !Array.isArray(parsed.sessions)
      ? parsed.sessions
      : {},
  };
};
const save = (state) => writeFileSync(statePath, JSON.stringify(state));
const options = [
  { id: "model", name: "Model", type: "select", currentValue: "fake-model", options: [{ value: "fake-model", name: "Fake model" }] },
  { id: "variant", name: "Variant", type: "select", currentValue: "fake-effort", options: [{ value: "fake-effort", name: "Fake effort" }] },
  { id: "mode", name: "Mode", type: "select", currentValue: "fake-mode", options: [{ value: "fake-mode", name: "Fake mode" }] },
  { id: "agent", name: "Agent", type: "select", currentValue: "fake-agent", options: [{ value: "fake-agent", name: "Fake agent" }] },
  { id: "allow_all", name: "Allow all", type: "boolean", currentValue: false },
];
const modes = {
  currentModeId: "fake-mode",
  availableModes: [{ id: "fake-mode", name: "Fake mode" }],
};
const send = (message) => process.stdout.write(JSON.stringify(message) + "\n");
const responseFirst = process.env.LICO_FAKE_ACP_RESPONSE_FIRST !== "0";

async function acp() {
  const lines = createInterface({ input: process.stdin });
  let active = "";
  for await (const line of lines) {
    const message = JSON.parse(line);
    if (message.method === "initialize") {
      send({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: true, sessionCapabilities: { delete: {}, list: {}, resume: {}, close: {} } } } });
    } else if (message.method === "session/new") {
      const state = load();
      active = "fake-session-" + (++state.counter);
      state.sessions[active] = { cwd: message.params.cwd, messages: [] };
      save(state);
      send({ jsonrpc: "2.0", id: message.id, result: { sessionId: active, configOptions: options, modes } });
    } else if (message.method === "session/load") {
      const state = load();
      active = message.params.sessionId;
      const session = state.sessions[active];
      if (!session) {
        send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: "missing" } });
        continue;
      }
      for (const entry of session.messages.filter((item) => item.role === "assistant")) {
        send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: active, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: entry.text } } } });
      }
      send({ jsonrpc: "2.0", id: message.id, result: { sessionId: active, configOptions: options, modes } });
    } else if (message.method === "session/list") {
      const state = load();
      send({ jsonrpc: "2.0", id: message.id, result: { sessions: Object.entries(state.sessions).map(([sessionId, value]) => ({ sessionId, cwd: value.cwd })) } });
    } else if (message.method === "session/close") {
      const state = load();
      delete state.sessions[message.params.sessionId];
      save(state);
      send({ jsonrpc: "2.0", id: message.id, result: {} });
    } else if (message.method === "session/set_config_option") {
      const state = load();
      const session = state.sessions[message.params.sessionId];
      if (!session) {
        send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: "missing" } });
        continue;
      }
      session.config ||= {};
      session.config[message.params.configId] = message.params.value;
      save(state);
      send({ jsonrpc: "2.0", id: message.id, result: { configOptions: options.map((option) => option.id === message.params.configId ? { ...option, currentValue: message.params.value } : option) } });
    } else if (message.method === "session/prompt") {
      const prompt = message.params.prompt[0].text;
      if (prompt.includes("SELFTEST_PERMISSION")) {
        const state = load();
        state.sessions[active].messages.push({ role: "user", text: prompt });
        save(state);
        send({ jsonrpc: "2.0", id: 991, method: "session/request_permission", params: { sessionId: active, options: [] } });
        continue;
      }
      if (prompt.includes("SELFTEST_OVERFLOW")) {
        const state = load();
        state.sessions[active].messages.push({ role: "user", text: prompt });
        save(state);
        send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: active, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "x".repeat(65536) } } } });
        continue;
      }
      if (prompt.includes("SELFTEST_MALFORMED_UPDATE_ENVELOPE")) {
        const state = load();
        state.sessions[active].messages.push({ role: "user", text: prompt });
        save(state);
        send({ jsonrpc: "2.0", id: message.id, result: { stopReason: "end_turn" } });
        send({
          jsonrpc: "2.0",
          method: "session/update",
          params: {
            sessionId: active,
            update: {
              sessionUpdate: "agent_message_chunk",
              content: { type: "text", text: "MALFORMED-ENVELOPE-CANARY" },
            },
          },
          unexpectedEnvelopeField: true,
        });
        continue;
      }
      if (prompt.includes("SELFTEST_MALFORMED_BEFORE_PROMPT_RESPONSE")) {
        const state = load();
        state.sessions[active].messages.push({ role: "user", text: prompt });
        save(state);
        process.stdout.write([
          {
            jsonrpc: "2.0",
            method: "session/update",
            params: {
              sessionId: active,
              update: {
                sessionUpdate: "agent_message_chunk",
                content: {
                  type: "text",
                  text: { canary: "MALFORMED-BEFORE-RESPONSE-CANARY" },
                },
              },
            },
          },
          { jsonrpc: "2.0", id: message.id, result: { stopReason: "end_turn" } },
          {
            jsonrpc: "2.0",
            method: "session/update",
            params: {
              sessionId: active,
              update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "MUST-NOT-RECOVER" },
              },
            },
          },
        ].map((entry) => JSON.stringify(entry)).join("\n") + "\n");
        continue;
      }
      if (prompt.includes("SELFTEST_MALFORMED_CONTENT")) {
        const state = load();
        state.sessions[active].messages.push({ role: "user", text: prompt });
        save(state);
        send({ jsonrpc: "2.0", id: message.id, result: { stopReason: "end_turn" } });
        send({
          jsonrpc: "2.0",
          method: "session/update",
          params: {
            sessionId: active,
            update: {
              sessionUpdate: "agent_message_chunk",
              content: {
                type: "text",
                text: { canary: "MALFORMED-CONTENT-CANARY" },
              },
            },
          },
        });
        send({
          jsonrpc: "2.0",
          method: "session/update",
          params: {
            sessionId: active,
            update: {
              sessionUpdate: "agent_message_chunk",
              content: { type: "text", text: "MUST-NOT-RESET-QUIESCENCE" },
            },
          },
        });
        continue;
      }
      const reply = prompt.match(/Reply with exactly ([0-9]+)/u)?.[1] || "0";
      const state = load();
      state.sessions[active].messages.push({ role: "user", text: prompt }, { role: "assistant", text: reply });
      save(state);
      const split = Math.max(1, Math.ceil(reply.length / 2));
      const chunks = [reply.slice(0, split), reply.slice(split)].filter(Boolean);
      if (!responseFirst) {
        send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: active, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: reply } } } });
      }
      send({ jsonrpc: "2.0", id: message.id, result: { stopReason: "end_turn" } });
      if (responseFirst) {
        for (const chunk of chunks.slice(0, -1)) {
          send({ jsonrpc: "2.0", method: "session/update", params: { sessionId: active, update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: chunk } } } });
        }
        await new Promise((resolveImmediate) => setImmediate(resolveImmediate));
        for (const [index, chunk] of chunks.slice(-1).entries()) {
          send({
            jsonrpc: "2.0",
            method: "session/update",
            params: {
              sessionId: active,
              update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: chunk },
              },
              _meta: {
                fixtureDelivery: "after_prompt_response_set_immediate",
                fixtureDelayedChunkIndex: index,
              },
            },
          });
        }
      }
    } else if (message.method === "session/cancel" || (message.id === 991 && message.result?.outcome?.outcome === "cancelled")) {
      process.exit(0);
    }
  }
}

async function piRpc() {
  const root = process.env.PI_CODING_AGENT_SESSION_DIR;
  mkdirSync(root, { recursive: true });
  const state = load();
  let active = "fake-pi-" + (++state.counter);
  state.sessions[active] = { cwd: process.cwd(), messages: [] };
  save(state);
  const sessionPath = (id) => root + "/" + id + ".jsonl";
  const persist = () => {
    const current = load().sessions[active];
    const rows = [
      { type: "session", version: 3, id: active, cwd: current.cwd },
      ...current.messages.map((message, index) => ({ type: "message", id: active + "-" + index, message })),
    ];
    writeFileSync(sessionPath(active), rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
  };
  persist();
  const lines = createInterface({ input: process.stdin });
  for await (const line of lines) {
    const command = JSON.parse(line);
    const respond = (success, data = {}) => send({ id: command.id, type: "response", command: command.type, success, data });
    if (command.type === "switch_session") {
      const header = JSON.parse(readFileSync(command.sessionPath, "utf8").split(/\r?\n/u)[0]);
      active = header.id;
      respond(true, { cancelled: false });
    } else if (command.type === "get_state") {
      respond(true, { sessionId: active, model: null, thinkingLevel: "off", isStreaming: false });
    } else if (command.type === "prompt") {
      const reply = command.message.match(/Reply with exactly ([0-9]+)/u)?.[1] || "0";
      const current = load();
      current.sessions[active].messages.push(
        { role: "user", content: [{ type: "text", text: command.message }] },
        { role: "assistant", content: [{ type: "text", text: reply }] },
      );
      save(current);
      persist();
      respond(true);
      send({ type: "message_update", assistantMessageEvent: { type: "text_delta", delta: reply } });
      send({ type: "agent_settled" });
    } else if (command.type === "get_last_assistant_text") {
      const messages = load().sessions[active].messages;
      const text = messages.filter((message) => message.role === "assistant").at(-1)?.content?.[0]?.text || null;
      respond(true, { text });
    } else if (command.type === "get_messages") {
      respond(true, { messages: load().sessions[active].messages });
    } else if (command.type === "abort") {
      respond(true);
    } else {
      respond(false);
    }
  }
}

async function sidecar() {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;
  const request = JSON.parse(input);
  const operation = process.argv.slice(2)[2] || "send";
  const laneFamilies = {
    openclaw: "acp", "claude-code": "stream-json", codex: "app-server",
    antigravity: "cli", opencode: "serve-http", copilot: "acp",
    "kilo-code": "serve-http", cursor: "cli", hermes: "acp",
    "kimi-code": "acp", pi: "rpc", "deepseek-harness": "rpc",
    "lico-agent": "rpc",
  };
  const exactResume = true;
  const laneFamily = laneFamilies[request.agent] || "unavailable";
  if (operation === "capabilities") {
    process.stdout.write(JSON.stringify({
      ok: true,
      agentId: request.agent,
      laneFamily,
      runtimeProtocol: "fixture-protocol",
      capabilities: { officialLane: laneFamily !== "unavailable", exactResume },
    }));
    return;
  }
  if (operation === "open") {
    if (laneFamily === "unavailable" || (request.sessionId && !exactResume)) {
      process.stdout.write(JSON.stringify({ ok: false, error: { code: "fixture_open_blocked" } }));
    } else {
      process.stdout.write(JSON.stringify({ ok: true, sessionId: request.sessionId || "" }));
    }
    return;
  }
  if (operation === "stream") {
    process.stdout.write(JSON.stringify({ ok: true, status: "bound_on_send" }));
    return;
  }
  if (operation === "cancel") {
    process.stdout.write(JSON.stringify({ ok: false, error: { code: "dispatch_cancel_unsupported" } }));
    return;
  }
  const emitStream = request.streamEvents === true;
  const writeFinal = (payload) => {
    if (emitStream) {
      process.stdout.write(JSON.stringify({ event: "dispatch.turn.started", sessionId: payload.sessionId || "", turnId: "fake-turn", payload: {} }) + "\n");
      process.stdout.write(JSON.stringify({ event: "agent.message.chunk", sessionId: payload.sessionId || "", turnId: "fake-turn", payload: { text: payload.output || "" } }) + "\n");
      process.stdout.write(JSON.stringify({ event: "agent.message.completed", sessionId: payload.sessionId || "", turnId: "fake-turn", payload: { text: payload.output || "" } }) + "\n");
      process.stdout.write(JSON.stringify({ ...payload, event: "done" }) + "\n");
      return;
    }
    process.stdout.write(JSON.stringify(payload));
  };
  const reportedSessionId = (sessionId) =>
    process.env.LICO_FAKE_RESUME_ID_DRIFT === "1" && request.sessionId
      ? sessionId + "-drift"
      : sessionId;
  if (request.agent === "cursor") {
    let sessionId = request.sessionId || "";
    if (!sessionId) {
      const created = spawnSync(request.binaryPath, ["create-chat"], {
        cwd: request.workingDirectory,
        env: process.env,
        encoding: "utf8",
      });
      sessionId = String(created.stdout || "").trim().split(/\r?\n/u).find(Boolean) || "";
    }
    const args = [
      "--print", "--output-format", "stream-json", "--trust", "--force",
      "--workspace", request.workingDirectory, "--resume", sessionId, request.text,
    ];
    const run = spawnSync(request.binaryPath, args, {
      cwd: request.workingDirectory,
      env: process.env,
      encoding: "utf8",
    });
    let output = "";
    for (const line of String(run.stdout || "").split(/\r?\n/u)) {
      if (!line.trim()) continue;
      try {
        const message = JSON.parse(line);
        if (message?.type === "result" && typeof message.result === "string") output = message.result;
        if (message?.type === "assistant") {
          const text = message?.message?.content?.[0]?.text;
          if (typeof text === "string" && text.length > 0) output = text;
        }
      } catch {}
    }
    sessionId = reportedSessionId(sessionId);
    writeFinal({
      ok: true,
      schemaVersion: 3,
      adapterId: "cursor",
      driverId: "cursor-cli",
      runtimeProtocol: "cursor-agent-cli-v1",
      sessionId,
      threadId: sessionId,
      nativeSessionId: sessionId,
      turnStatus: "completed",
      output,
      effective: { cwd: request.workingDirectory, model: request.model || null, reasoningEffort: request.reasoningEffort || null, mode: null, runtimeAgent: null, allowAll: null },
    });
    return;
  }
  const child = spawn(request.binaryPath, request.agent === "codex" ? ["app-server", "--stdio"] : ["acp"], { cwd: request.workingDirectory, env: { ...process.env, LICO_FAKE_ACP_RESPONSE_FIRST: "0" }, stdio: ["pipe", "pipe", "ignore"] });
  const lines = createInterface({ input: child.stdout });
  const pending = new Map();
  let nextId = 1;
  let output = "";
  let sessionId = request.sessionId || "";
  if (request.agent === "codex") {
    const call = (method, params) => new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(String(id), { resolve, reject });
      child.stdin.write(JSON.stringify({ id, method, params }) + "\n");
    });
    lines.on("line", (line) => {
      const message = JSON.parse(line);
      if (message.method === "item/completed" && message.params?.item?.type === "agentMessage") {
        output += message.params.item.text || "";
      }
      if (Object.hasOwn(message, "id") && !message.method) {
        const waiter = pending.get(String(message.id));
        if (waiter) { pending.delete(String(message.id)); message.error ? waiter.reject(new Error("rejected")) : waiter.resolve(message.result); }
      }
    });
    await call("initialize", { clientInfo: { name: "fake" }, capabilities: { experimentalApi: true } });
    child.stdin.write(JSON.stringify({ method: "initialized" }) + "\n");
    if (sessionId) {
      await call("thread/resume", { threadId: sessionId });
    } else {
      const started = await call("thread/start", { cwd: request.workingDirectory });
      sessionId = started.thread.id;
    }
    output = "";
    const turn = await call("turn/start", { threadId: sessionId, input: [{ type: "text", text: request.text }] });
    await new Promise((resolve) => {
      const timer = setInterval(() => {
        if (output.length > 0) { clearInterval(timer); resolve(); }
      }, 5);
      setTimeout(() => { clearInterval(timer); resolve(); }, 200);
    });
    child.kill();
    sessionId = reportedSessionId(sessionId);
    writeFinal({
      ok: true,
      schemaVersion: 3,
      adapterId: "codex",
      driverId: "codex-app-server",
      runtimeProtocol: "codex-app-server-stdio-jsonrpc",
      sessionId,
      threadId: sessionId,
      nativeSessionId: sessionId,
      turnStatus: "end_turn",
      output: output || String(turn?.turn?.items?.find((item) => item.type === "agentMessage")?.text || ""),
      effective: { cwd: request.workingDirectory, model: request.model || null, reasoningEffort: request.reasoningEffort || null, mode: null, runtimeAgent: null, allowAll: null },
    });
    return;
  }
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    if (message.method === "session/update") output += message.params?.update?.content?.text || "";
    if (Object.hasOwn(message, "id") && !message.method) {
      const waiter = pending.get(String(message.id));
      if (waiter) { pending.delete(String(message.id)); message.error ? waiter.reject(new Error("rejected")) : waiter.resolve(message.result); }
    }
  });
  const call = (method, params) => new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(String(id), { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
  await call("initialize", { protocolVersion: 1, clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false } });
  const session = await call(request.sessionId ? "session/load" : "session/new", { cwd: request.workingDirectory, mcpServers: [], ...(request.sessionId ? { sessionId: request.sessionId } : {}) });
  sessionId = request.sessionId || session.sessionId;
  output = "";
  const turn = await call("session/prompt", { sessionId, prompt: [{ type: "text", text: request.text }] });
  child.kill();
  const identity = {
    openclaw: ["openclaw-acp", "openclaw-acp-stdio-jsonrpc"],
    hermes: ["hermes-acp", "hermes-acp-stdio-jsonrpc"],
    "kilo-code": ["kilo-code-serve", "kilo-code-serve-http-v1"],
    cursor: ["cursor-cli", "cursor-agent-cli-v1"],
    copilot: ["copilot-acp", "copilot-acp-v1-stdio-ndjson"],
    "kimi-code": ["kimi-code-acp", "kimi-code-acp-v1-stdio-ndjson"],
    opencode: ["opencode-serve", "opencode-serve-http-v1"],
  }[request.agent] || ["opencode-serve", "opencode-serve-http-v1"];
  sessionId = reportedSessionId(sessionId);
  writeFinal({
    ok: true,
    schemaVersion: 3,
    adapterId: request.agent,
    driverId: identity[0],
    runtimeProtocol: identity[1],
    sessionId,
    threadId: sessionId,
    nativeSessionId: sessionId,
    turnStatus: turn.stopReason,
    output,
    effective: { cwd: request.workingDirectory, model: "fake-model", reasoningEffort: "fake-effort", mode: "fake-mode", runtimeAgent: "fake-agent", allowAll: false },
  });
}

async function appServer() {
  const lines = createInterface({ input: process.stdin });
  let nextTurn = 1;
  for await (const line of lines) {
    const message = JSON.parse(line);
    if (message.method === "initialize") {
      process.stdout.write(JSON.stringify({ id: message.id, result: { protocolVersion: 1 } }) + "\n");
    } else if (message.method === "initialized") {
      continue;
    } else if (message.method === "thread/start") {
      const state = load();
      const threadId = "fake-thread-" + (++state.counter);
      state.sessions[threadId] = { cwd: message.params?.cwd || "", model: message.params?.model || null, messages: [], turns: [] };
      save(state);
      process.stdout.write(JSON.stringify({ id: message.id, result: { thread: { id: threadId }, model: state.sessions[threadId].model } }) + "\n");
    } else if (message.method === "thread/resume") {
      const state = load();
      const threadId = message.params.threadId;
      if (!state.sessions[threadId]) {
        process.stdout.write(JSON.stringify({ id: message.id, error: { code: -32000, message: "missing" } }) + "\n");
        continue;
      }
      process.stdout.write(JSON.stringify({ id: message.id, result: { thread: { id: threadId }, model: state.sessions[threadId].model } }) + "\n");
    } else if (message.method === "thread/read") {
      const state = load();
      const thread = state.sessions[message.params.threadId];
      if (!thread) {
        process.stdout.write(JSON.stringify({ id: message.id, error: { code: -32000, message: "missing" } }) + "\n");
        continue;
      }
      process.stdout.write(JSON.stringify({ id: message.id, result: { thread: { id: message.params.threadId, turns: thread.turns }, model: thread.model } }) + "\n");
    } else if (message.method === "thread/list") {
      const state = load();
      process.stdout.write(JSON.stringify({ id: message.id, result: { threads: Object.keys(state.sessions).map((id) => ({ id })) } }) + "\n");
    } else if (message.method === "thread/delete") {
      const state = load();
      delete state.sessions[message.params.threadId];
      save(state);
      process.stdout.write(JSON.stringify({ id: message.id, result: {} }) + "\n");
    } else if (message.method === "turn/start") {
      const prompt = message.params.input[0].text;
      const reply = prompt.match(/Reply with exactly ([0-9]+)/u)?.[1] || "0";
      const turnId = "fake-turn-" + (nextTurn++);
      const state = load();
      const thread = state.sessions[message.params.threadId];
      thread.messages.push({ role: "user", text: prompt }, { role: "assistant", text: reply });
      const turn = { id: turnId, status: "completed", items: [{ type: "agentMessage", text: reply }] };
      thread.turns.push(turn);
      save(state);
      process.stdout.write(JSON.stringify({ id: message.id, result: { turn: { id: turnId } } }) + "\n");
      process.stdout.write(JSON.stringify({ method: "item/completed", params: { threadId: message.params.threadId, turnId, item: { type: "agentMessage", text: reply } } }) + "\n");
      process.stdout.write(JSON.stringify({ method: "turn/completed", params: { threadId: message.params.threadId, turn } }) + "\n");
    }
  }
}

async function sdk() {
  let buffer = Buffer.alloc(0);
  const reply = (id, result) => {
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id, result }));
    process.stdout.write(Buffer.concat([Buffer.from("Content-Length: " + body.length + "\r\n\r\n"), body]));
  };
  for await (const chunk of process.stdin) {
    buffer = Buffer.concat([buffer, chunk]);
    while (true) {
      const headerEnd = buffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) break;
      const match = buffer.subarray(0, headerEnd).toString("ascii").match(/Content-Length:\s*(\d+)/i);
      if (!match) process.exit(3);
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      if (buffer.length < bodyStart + length) break;
      const message = JSON.parse(buffer.subarray(bodyStart, bodyStart + length).toString("utf8"));
      buffer = buffer.subarray(bodyStart + length);
      if (message.method === "connect" || message.method === "ping") {
        reply(message.id, { protocolVersion: 1 });
      } else if (message.method === "session.list") {
        const state = load();
        reply(message.id, { sessions: Object.keys(state.sessions).map((sessionId) => ({ sessionId })) });
      } else if (message.method === "session.delete") {
        const state = load();
        delete state.sessions[message.params.sessionId];
        save(state);
        reply(message.id, { success: true });
      } else {
        process.exit(4);
      }
    }
  }
}

async function claudeStreamJson() {
  const args = process.argv.slice(2);
  const modelIndex = args.indexOf("--model");
  const model = modelIndex >= 0 ? args[modelIndex + 1] : null;
  const state = load();
  const sessionId = "fake-claude-process-" + (++state.counter);
  save(state);
  let remembered = "";
  let turn = 0;
  if (process.env.LICO_FAKE_CLAUDE_RETAIN_DESCENDANT === "1") {
    const retainedPipeScript = [
      'const fs = require("node:fs")',
      'const marker = process.env.LICO_FAKE_PROCESS_LOCAL_MARKER',
      'const gate = process.env.LICO_FAKE_CLAUDE_CLOSE_GATE',
      'if (marker) fs.appendFileSync(marker, "descendant_pipe_open\\n")',
      'const finish = () => {',
      '  if (!gate || fs.existsSync(gate)) {',
      '    if (marker) fs.appendFileSync(marker, "descendant_pipe_closed\\n")',
      '    process.exit(0)',
      '  }',
      '  setTimeout(finish, 2)',
      '}',
      'finish()',
    ].join(";\n");
    spawn(process.execPath, ["-e", retainedPipeScript], {
      env: process.env,
      stdio: ["ignore", "inherit", "inherit"],
    });
  }
  const lines = createInterface({ input: process.stdin });
  for await (const line of lines) {
    if (process.env.LICO_FAKE_REQUIRE_NO_HISTORY === "1"
      && process.env.CLAUDE_CODE_SKIP_PROMPT_HISTORY !== "1") process.exit(9);
    const message = JSON.parse(line);
    if (message.type === "control_request") continue;
    const prompt = message?.message?.content?.[0]?.text || "";
    if (process.env.LICO_FAKE_CLAUDE_PERSIST === "1" && process.env.CLAUDE_CONFIG_DIR) {
      appendFileSync(
        process.env.CLAUDE_CONFIG_DIR + "/persisted-transcript",
        prompt + "\n",
      );
    }
    turn += 1;
    const rememberMatch = prompt.match(/Remember the number ([0-9]+)/u);
    if (rememberMatch) remembered = rememberMatch[1];
    const expectedMatch = prompt.match(/reply with exactly ([A-Z0-9-]+)/iu);
    const output = prompt === "SELFTEST_PROCESS_LOCAL_UTF8_A"
      ? "界".repeat(3000)
      : prompt === "SELFTEST_PROCESS_LOCAL_UTF8_B"
        ? "🙂".repeat(2250)
        : prompt === "SELFTEST_PROCESS_LOCAL_UTF8_C"
          ? "é".repeat(4000)
          : prompt === "SELFTEST_PROCESS_LOCAL_UTF8_D"
            ? "ß".repeat(4000)
          : prompt.includes("previous turn")
              ? remembered
              : (expectedMatch?.[1] || "0");
    if (turn === 1) {
      send({ type: "system", subtype: "init", session_id: sessionId, model });
    }
    send({
      type: "stream_event",
      session_id: sessionId,
      event: {
        type: "content_block_delta",
        delta: { type: "text_delta", text: output },
      },
    });
    send({
      type: "assistant",
      session_id: sessionId,
      message: { role: "assistant", content: [{ type: "text", text: output }] },
    });
    send({
      type: "result",
      subtype: "success",
      is_error: false,
      result: output,
      session_id: sessionId,
      uuid: "fake-process-turn-" + turn,
      permission_denials: [],
    });
  }
  const closeGate = process.env.LICO_FAKE_CLAUDE_CLOSE_GATE;
  if (closeGate) {
    if (process.env.LICO_FAKE_PROCESS_LOCAL_MARKER) {
      appendFileSync(
        process.env.LICO_FAKE_PROCESS_LOCAL_MARKER,
        "child_waiting_for_close_gate\n",
      );
    }
    await new Promise((resolveGate, rejectGate) => {
      if (existsSync(closeGate)) {
        resolveGate();
        return;
      }
      let timer;
      const interval = setInterval(() => {
        if (!existsSync(closeGate)) return;
        clearTimeout(timer);
        clearInterval(interval);
        resolveGate();
      }, 5);
      timer = setTimeout(() => {
        clearInterval(interval);
        rejectGate(new Error("synthetic close gate timeout"));
      }, 5000);
    });
  }
  if (process.env.LICO_FAKE_PROCESS_LOCAL_MARKER) {
    appendFileSync(process.env.LICO_FAKE_PROCESS_LOCAL_MARKER, "child_closed\n");
  }
}

async function rpcStdio() {
  const protocol = "licoup.stdio.v1";
  const transports = new Map();
  const marker = process.env.LICO_FAKE_PROCESS_LOCAL_MARKER;
  const fault = process.env.LICO_FAKE_PROCESS_LOCAL_FAULT || "";
  const mark = (value) => {
    if (marker) appendFileSync(marker, value + "\n");
  };
  const write = (frame) => process.stdout.write(JSON.stringify(frame) + "\n");
  const writeBatch = (frames) => process.stdout.write(
    frames.map((frame) => JSON.stringify(frame)).join("\n") + "\n",
  );
  const response = (request, result) => write({
    protocol,
    id: request.id,
    workflowId: request.workflowId,
    ok: true,
    result,
  });
  const streamEvent = (request, sequence, event) => write({
    protocol,
    id: request.id,
    workflowId: request.workflowId,
    kind: "event",
    sequence,
    event,
  });
  const streamTerminal = (request, sequence, result) => write({
    protocol,
    id: request.id,
    workflowId: request.workflowId,
    kind: "terminal",
    sequence,
    ok: true,
    result,
  });
  const spawnTransport = (params) => {
    const args = [
      "--print", "--input-format", "stream-json", "--output-format", "stream-json",
      "--verbose", "--include-partial-messages", "--no-session-persistence",
      ...(params.model ? ["--model", params.model] : []),
    ];
    const child = spawn(params.binaryPath, args, {
      cwd: params.workingDirectory,
      env: process.env,
      stdio: ["pipe", "pipe", "ignore"],
    });
    const lines = createInterface({ input: child.stdout });
    return {
      child,
      iterator: lines[Symbol.asyncIterator](),
      sessionId: "",
      model: params.model || null,
      turns: [],
      closed: new Promise((resolveClosed) => child.once("close", resolveClosed)),
    };
  };
  const closeTransport = async (transport) => {
    if (!transport) return;
    transport.child.stdin.end();
    await transport.closed;
    mark("io_workers_joined");
  };
  const drainAll = async () => {
    const unique = [...new Set(transports.values())];
    transports.clear();
    for (const transport of unique) await closeTransport(transport);
  };
  const sendTurn = async (request) => {
    const params = request.params || {};
    if (fault === "authentication") {
      streamTerminal(request, 1, {
        ok: false,
        error: { code: "claude_code_authentication_required" },
      });
      return;
    }
    if (params.text === "SELFTEST_PROCESS_LOCAL_SEQUENCE") {
      streamEvent(request, 2, {
        event: "agent.message.chunk",
        sessionId: "synthetic-invalid-session",
        turnId: "synthetic-invalid-turn",
        payload: { text: "invalid" },
      });
      return;
    }
    let transport = params.sessionId ? transports.get(params.sessionId) : null;
    if (!transport) {
      if (params.sessionId) {
        streamTerminal(request, 1, {
          ok: false,
          error: { code: "claude_code_live_session_unavailable" },
        });
        return;
      }
      transport = spawnTransport(params);
    }
    transport.child.stdin.write(JSON.stringify({
      type: "user",
      message: { role: "user", content: [{ type: "text", text: params.text }] },
    }) + "\n");
    let sequence = 0;
    let output = "";
    const turnId = params.text === "SELFTEST_PROCESS_LOCAL_REUSED_TURN"
      ? "fake-rpc-turn-reused"
      : "fake-rpc-turn-" + request.id;
    let started = false;
    while (true) {
      const next = await transport.iterator.next();
      if (next.done) throw new Error("fake claude closed before result");
      const message = JSON.parse(next.value);
      const observedSession = message.session_id || transport.sessionId;
      if (message.type === "system") {
        transport.sessionId = observedSession;
        transports.set(observedSession, transport);
      }
      if (!started && observedSession) {
        started = true;
        streamEvent(request, ++sequence, {
          event: "dispatch.turn.started",
          sessionId: params.text === "SELFTEST_PROCESS_LOCAL_MISSING_SESSION"
            ? ""
            : params.text === "SELFTEST_PROCESS_LOCAL_OVERSIZED_SESSION"
              ? "s".repeat(513)
              : observedSession,
          turnId: params.text === "SELFTEST_PROCESS_LOCAL_MISSING_TURN" ? "" : turnId,
          payload: {},
        });
        if (params.text === "SELFTEST_PROCESS_LOCAL_DUPLICATE_EVENT") {
          streamEvent(request, ++sequence, {
            event: "dispatch.turn.started",
            sessionId: observedSession,
            turnId,
            payload: {},
          });
        }
      }
      if (message.type === "stream_event") {
        const realText = message?.event?.delta?.text || "";
        const text = params.text === "SELFTEST_PROCESS_LOCAL_EMPTY_CHUNK"
          ? ""
          : params.text === "SELFTEST_PROCESS_LOCAL_UNRELATED_CHUNK"
            ? realText + "-unrelated"
            : params.text === "SELFTEST_PROCESS_LOCAL_OUTPUT_OVERFLOW"
              ? "x".repeat(1024 * 1024)
              : realText;
        output += realText;
        streamEvent(request, ++sequence, {
          event: "agent.message.chunk",
          sessionId: params.text === "SELFTEST_PROCESS_LOCAL_CROSS_SESSION"
            ? observedSession + "-other"
            : observedSession,
          turnId: params.text === "SELFTEST_PROCESS_LOCAL_CROSS_TURN"
            ? turnId + "-other"
            : turnId,
          payload: { text },
        });
      }
      if (message.type === "result") {
        output = message.result;
        const events = [
          ["agent.message.completed", { text: output }],
          ["dispatch.turn.completed", {}],
        ];
        for (const [event, payload] of events) {
          streamEvent(request, ++sequence, {
            event,
            sessionId: observedSession,
            turnId,
            payload,
          });
        }
        transport.turns.push({ turnId, output });
        while (transport.turns.length > 64) transport.turns.shift();
        let byteCount = transport.turns.reduce(
          (total, entry) => total + Buffer.byteLength(entry.output),
          0,
        );
        while (byteCount > 32768 && transport.turns.length > 1) {
          transport.turns.shift();
          byteCount = transport.turns.reduce(
            (total, entry) => total + Buffer.byteLength(entry.output),
            0,
          );
        }
        const terminalResult = {
          ok: true,
          nativeSessionId: observedSession,
          sessionId: observedSession,
          threadId: observedSession,
          turnId,
          turnStatus: "completed",
          output,
          effective: { cwd: params.workingDirectory, model: transport.model },
        };
        if (params.text === "SELFTEST_PROCESS_LOCAL_LATE_EVENT") {
          writeBatch([
            {
              protocol,
              id: request.id,
              workflowId: request.workflowId,
              kind: "terminal",
              sequence: ++sequence,
              ok: true,
              result: terminalResult,
            },
            {
              protocol,
              id: request.id,
              workflowId: request.workflowId,
              kind: "event",
              sequence: ++sequence,
              event: {
                event: "agent.message.chunk",
                sessionId: observedSession,
                turnId,
                payload: { text: "late" },
              },
            },
          ]);
        } else {
          streamTerminal(request, ++sequence, terminalResult);
        }
        return;
      }
    }
  };
  const lines = createInterface({ input: process.stdin });
  for await (const line of lines) {
    const request = JSON.parse(line);
    const params = request.params || {};
    if (request.method === "agent.conversation.capabilities") {
      response(request, {
        ok: true,
        agentId: params.agent,
        laneFamily: "stream-json",
        capabilities: {
          officialLane: true,
          exactResume: true,
          processLocalContinuation: true,
          streaming: true,
          structuredEvents: true,
          cancel: false,
        },
      });
    } else if (request.method === "agent.conversation.open") {
      if (params.sessionId && !transports.has(params.sessionId)) {
        response(request, {
          ok: false,
          error: { code: "claude_code_live_session_unavailable" },
        });
      } else {
        response(request, {
          ok: true,
          openMode: params.sessionId ? "resume" : "new",
          nativeSessionId: params.sessionId || "",
          sessionId: params.sessionId || "",
        });
      }
    } else if (request.method === "agent.conversation.send") {
      await sendTurn(request);
    } else if (request.method === "agent.conversation.history") {
      const transport = transports.get(params.sessionId);
      if (!transport) {
        response(request, {
          ok: false,
          error: { code: "claude_code_session_unavailable" },
        });
      } else {
        const turns = transport.turns.slice(-64);
        const history = {
          ok: true,
          continuityScope: "process-local",
          nativeSessionId: params.sessionId,
          turns,
          turnCount: turns.length,
          byteCount: turns.reduce((total, entry) => total + Buffer.byteLength(entry.output), 0),
        };
        if (fault === "history_forged_identity") history.nativeSessionId += "-forged";
        if (fault === "history_turn_count") history.turnCount += 1;
        if (fault === "history_byte_count") history.byteCount += 1;
        if (fault === "history_shape") history.unreviewed = true;
        response(request, history);
      }
    } else if (request.method === "agent.conversation.cleanup") {
      const transport = transports.get(params.sessionId);
      if (!transport) {
        response(request, {
          ok: false,
          error: { code: "claude_code_session_unavailable" },
        });
      } else {
        mark("cleanup_started");
        transports.delete(params.sessionId);
        if (fault === "cleanup_early_ack") {
          mark("cleanup_ack");
          response(request, { ok: true, status: "cleaned" });
          await closeTransport(transport);
          continue;
        }
        await closeTransport(transport);
        mark("cleanup_ack");
        response(request, { ok: true, status: "cleaned" });
      }
    } else if (request.method === "shutdown") {
      mark("shutdown_started");
      await drainAll();
      if (fault === "shutdown_failure") {
        write({
          protocol,
          id: request.id,
          workflowId: request.workflowId,
          ok: false,
          error: { code: "process_local_shutdown_failed" },
        });
        return;
      }
      mark("shutdown_ack");
      response(request, { status: "shutdown" });
      lines.close();
      process.stdin.unref();
      return;
    } else {
      response(request, { ok: false, error: { code: "invalid_method" } });
    }
  }
  await drainAll();
  mark("eof_drained");
}

const args = process.argv.slice(2);
if (args[0] === "agent") {
  await sidecar();
} else if (args[0] === "rpc" && args[1] === "stdio") {
  await rpcStdio();
} else if (args[0] === "app-server") {
  await appServer();
} else if (args[0] === "--mode" && args[1] === "rpc") {
  await piRpc();
} else if (args.includes("--server") || args.includes("--headless")) {
  await sdk();
} else if (args[0] === "create-chat") {
  const state = load();
  state.sessions ||= {};
  state.counter = Number(state.counter || 0) + 1;
  const sessionId = "fake-cursor-session-" + String(state.counter).padStart(12, "0");
  state.sessions[sessionId] = { cwd: process.cwd(), messages: [] };
  save(state);
  process.stdout.write(sessionId);
} else if (args.includes("--print") && args.includes("--resume") && args.includes("stream-json")) {
  const resumeIndex = args.indexOf("--resume");
  const sessionId = args[resumeIndex + 1];
  const prompt = args[args.length - 1];
  const reply = prompt.match(/Reply with exactly ([0-9A-Z-]+)/iu)?.[1] || "0";
  const state = load();
  state.sessions ||= {};
  if (!state.sessions[sessionId]) process.exit(2);
  state.sessions[sessionId].messages.push({ role: "user", text: prompt }, { role: "assistant", text: reply });
  save(state);
  // The real Cursor CLI may report a display label here even though argv used
  // a stable model slug. The parity lane must retain the explicit selector.
  send({ type: "system", subtype: "init", session_id: sessionId, model: "Fixture Display Model" });
  send({ type: "assistant", session_id: sessionId, message: { content: [{ type: "text", text: reply }] } });
  send({ type: "result", subtype: "success", is_error: false, session_id: sessionId, result: reply });
} else if (args[0] === "acp" || args[0] === "serve") {
  // Fixture-only: serve-http agents still exercise ACP JSON-RPC through the
  // fake binary. Live release-UI uses the real serve/HTTP attach path.
  await acp();
} else if (args.includes("--input-format") && args.includes("stream-json")) {
  await claudeStreamJson();
} else if (args[0] === "session" && args[1] === "list") {
  const state = load();
  process.stdout.write(JSON.stringify(Object.entries(state.sessions).map(([id, session]) => ({ id, title: session.messages[0]?.text || "" }))));
} else if (args[0] === "session" && args[1] === "delete") {
  if (args[2] === "--help") process.exit(0);
  const state = load();
  if (!state.sessions[args[2]]) process.exit(2);
  delete state.sessions[args[2]];
  save(state);
} else if (args[0] === "sessions" && args[1] === "list") {
  const state = load();
  for (const [id, session] of Object.entries(state.sessions)) {
    const stamp = id.startsWith("fake-session-")
      ? ("20260711_000000_" + id.replace(/[^a-f0-9]/gu, "").padEnd(6, "a").slice(0, 6))
      : id;
    process.stdout.write((session.messages[0]?.text || "session") + " " + stamp + "\n");
  }
} else if (args[0] === "sessions" && args[1] === "delete") {
  if (args.includes("--help")) process.exit(0);
  const state = load();
  const target = args[2];
  const match = Object.keys(state.sessions).find((id) => id === target || ("20260711_000000_" + id.replace(/[^a-f0-9]/gu, "").padEnd(6, "a").slice(0, 6)) === target);
  if (!match) process.exit(2);
  delete state.sessions[match];
  save(state);
} else if (args[0] === "sessions" && args[1] === "export") {
  const destination = args[2];
  const sessionId = args[args.indexOf("--session-id") + 1];
  const state = load();
  const match = Object.keys(state.sessions).find((id) => id === sessionId || ("20260711_000000_" + id.replace(/[^a-f0-9]/gu, "").padEnd(6, "a").slice(0, 6)) === sessionId);
  if (!match) process.exit(2);
  writeFileSync(destination, JSON.stringify(state.sessions[match]));
} else if (args[0] === "export") {
  const state = load();
  if (!state.sessions[args[1]]) process.exit(2);
  process.stdout.write(JSON.stringify(state.sessions[args[1]]));
} else {
  process.exit(2);
}
`;
