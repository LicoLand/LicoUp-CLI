#!/usr/bin/env node
/**
 * Vendor-neutral Agent Client Protocol (ACP) reference agent.
 * Transport: JSON-RPC 2.0 over stdio, one message per NDJSON line.
 *
 * Launch: node tools/acp-reference-agent/agent.mjs [acp]
 * State:  LICO_ACP_REFERENCE_STATE (optional JSON persistence path)
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";

const statePath = process.env.LICO_ACP_REFERENCE_STATE || "";
const loadState = () => (
  statePath && existsSync(statePath)
    ? JSON.parse(readFileSync(statePath, "utf8"))
    : { counter: 0, sessions: {} }
);
const saveState = (state) => {
  if (statePath) writeFileSync(statePath, JSON.stringify(state));
};

const configOptions = Object.freeze([
  {
    id: "model",
    name: "Model",
    type: "select",
    currentValue: "reference-model",
    options: [{ value: "reference-model", name: "Reference model" }],
  },
  {
    id: "mode",
    name: "Mode",
    type: "select",
    currentValue: "reference-mode",
    options: [{ value: "reference-mode", name: "Reference mode" }],
  },
]);

const modes = Object.freeze({
  currentModeId: "reference-mode",
  availableModes: [{ id: "reference-mode", name: "Reference mode" }],
});

const agentCapabilities = Object.freeze({
  loadSession: true,
  sessionCapabilities: {
    delete: {},
    list: {},
    resume: {},
    close: {},
  },
});

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function respond(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function respondError(id, message, code = -32000) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function sessionPayload(sessionId) {
  return { sessionId, configOptions, modes };
}

function replayAssistantMessages(sessionId, session) {
  for (const entry of session.messages.filter((item) => item.role === "assistant")) {
    send({
      jsonrpc: "2.0",
      method: "session/update",
      params: {
        sessionId,
        update: {
          sessionUpdate: "agent_message_chunk",
          content: { type: "text", text: entry.text },
        },
      },
    });
  }
}

function extractReply(promptBlocks) {
  const prompt = promptBlocks?.[0]?.text || "";
  return prompt.match(/Reply with exactly ([0-9]+)/u)?.[1] || "0";
}

function streamReply(sessionId, reply) {
  const split = Math.max(1, Math.ceil(reply.length / 2));
  const chunks = [reply.slice(0, split), reply.slice(split)].filter(Boolean);
  for (const chunk of chunks) {
    send({
      jsonrpc: "2.0",
      method: "session/update",
      params: {
        sessionId,
        update: {
          sessionUpdate: "agent_message_chunk",
          content: { type: "text", text: chunk },
        },
      },
    });
  }
  return reply;
}

function requireSession(state, sessionId) {
  const session = state.sessions[sessionId];
  if (!session) return null;
  return session;
}

async function handlePrompt(message, activeSessionId, cancelled) {
  const sessionId = message.params?.sessionId;
  if (sessionId !== activeSessionId) {
    respondError(message.id, "session mismatch");
    return;
  }
  const state = loadState();
  const session = requireSession(state, sessionId);
  if (!session) {
    respondError(message.id, "missing");
    return;
  }
  const prompt = message.params?.prompt?.[0]?.text || "";
  session.messages.push({ role: "user", text: prompt });
  if (cancelled.current) {
    respond(message.id, { stopReason: "cancelled" });
    saveState(state);
    cancelled.current = false;
    return;
  }
  const reply = extractReply(message.params?.prompt);
  session.messages.push({ role: "assistant", text: reply });
  saveState(state);
  respond(message.id, { stopReason: "end_turn" });
  if (!cancelled.current) streamReply(sessionId, reply);
}

async function runAcpServer() {
  const lines = createInterface({ input: process.stdin });
  let activeSessionId = "";
  const cancelled = { current: false };

  for await (const line of lines) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      process.exit(3);
    }
    const method = message.method;
    if (method === "initialize") {
      respond(message.id, {
        protocolVersion: 1,
        agentInfo: {
          name: "lico-acp-reference-agent",
          title: "LicoUp ACP Reference Agent",
          version: "1.0.0",
        },
        agentCapabilities,
      });
      continue;
    }
    if (method === "session/new") {
      const state = loadState();
      activeSessionId = `reference-session-${++state.counter}`;
      state.sessions[activeSessionId] = {
        cwd: message.params?.cwd || process.cwd(),
        messages: [],
        config: {},
      };
      saveState(state);
      respond(message.id, sessionPayload(activeSessionId));
      continue;
    }
    if (method === "session/load" || method === "session/resume") {
      const sessionId = message.params?.sessionId;
      const state = loadState();
      const session = requireSession(state, sessionId);
      if (!session) {
        respondError(message.id, "missing");
        continue;
      }
      activeSessionId = sessionId;
      replayAssistantMessages(sessionId, session);
      respond(message.id, sessionPayload(sessionId));
      continue;
    }
    if (method === "session/list") {
      const state = loadState();
      respond(message.id, {
        sessions: Object.entries(state.sessions).map(([sessionId, value]) => ({
          sessionId,
          cwd: value.cwd,
        })),
      });
      continue;
    }
    if (method === "session/close") {
      const state = loadState();
      const sessionId = message.params?.sessionId;
      if (!requireSession(state, sessionId)) {
        respondError(message.id, "missing");
        continue;
      }
      delete state.sessions[sessionId];
      saveState(state);
      if (activeSessionId === sessionId) activeSessionId = "";
      respond(message.id, {});
      continue;
    }
    if (method === "session/set_config_option") {
      const state = loadState();
      const session = requireSession(state, message.params?.sessionId);
      if (!session) {
        respondError(message.id, "missing");
        continue;
      }
      session.config ||= {};
      session.config[message.params.configId] = message.params.value;
      saveState(state);
      respond(message.id, {
        configOptions: configOptions.map((option) => (
          option.id === message.params.configId
            ? { ...option, currentValue: message.params.value }
            : option
        )),
      });
      continue;
    }
    if (method === "session/prompt") {
      await handlePrompt(message, activeSessionId, cancelled);
      continue;
    }
    if (method === "session/cancel") {
      cancelled.current = true;
      continue;
    }
    if (Object.hasOwn(message, "id")) {
      respondError(message.id, `unsupported method: ${method}`, -32601);
    }
  }
}

const args = process.argv.slice(2);
if (args.length === 0 || args[0] === "acp") {
  await runAcpServer();
} else {
  process.stderr.write("Usage: agent.mjs [acp]\n");
  process.exit(2);
}
