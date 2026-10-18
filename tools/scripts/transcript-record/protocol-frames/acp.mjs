// Recorded vendor frames for this protocol family. See ../protocol-frames.mjs.
//
// Shape: { "<adapterId>": { channel: "<AdapterContract::framing>", scenarios: {
//   "<scenario>": [ <frame object> | "<raw wire string>" ] } } }
//
// The channel of every entry must equal the adapter's own `CONTRACT.framing`
// (`crates/licoup-native/src/platform/native_agent_parser/adapters/<id>.rs`).
// Content is synthetic: no real conversation, identity, or path is recorded.

const PROTOCOL_VERSION = 1;

// ACP v1 request ids exactly as the shared ACP state machine issues them.
const INITIALIZE_REQUEST_ID = 1;
const SESSION_REQUEST_ID = 2;
const PROMPT_REQUEST_ID = 3;
// Hermes and OpenClaw reserve request id 3 for their native model/mode request.
const SESSION_PROMPT_REQUEST_ID = 4;

// The conversation the recorded transcripts open and resume. Every adapter in
// this family replays the driver's exact-resume path, so the identity is the
// one the client already knew.
const NATIVE_SESSION = "native-session";
// OpenClaw resumes a Gateway conversation by its own resumable key.
const GATEWAY_SESSION_KEY = "agent:main:acp:native-session";

// The initialize result every ACP-family agent returns before a session opens.
function initializeResponse(agentInfo) {
  return {
    jsonrpc: "2.0",
    id: INITIALIZE_REQUEST_ID,
    result: {
      protocolVersion: PROTOCOL_VERSION,
      agentCapabilities: {
        loadSession: true,
        sessionCapabilities: { resume: {} },
      },
      agentInfo,
    },
  };
}

function agentMessageChunk(sessionId, text) {
  return {
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId,
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text },
      },
    },
  };
}

function promptResponse(stopReason) {
  return {
    jsonrpc: "2.0",
    id: SESSION_PROMPT_REQUEST_ID,
    result: { stopReason },
  };
}

function promptError(code, message) {
  return { jsonrpc: "2.0", id: SESSION_PROMPT_REQUEST_ID, error: { code, message } };
}

// Copilot and Kimi Code share one ACP v1 wire shape; only the message unit the
// parser reports differs, which is why the frames are built from one factory.
function acpScenarios(label) {
  const initialize = () => initializeResponse({ name: label, version: "synthetic" });
  const sessionOpened = (result) => ({
    jsonrpc: "2.0",
    id: SESSION_REQUEST_ID,
    result: { sessionId: NATIVE_SESSION, configOptions: [], ...result },
  });
  const chunk = (text) => agentMessageChunk(NATIVE_SESSION, text);
  return {
    "normal-turn": [
      initialize(),
      sessionOpened({}),
      chunk(`synthetic ${label} `),
      chunk("reply"),
      { jsonrpc: "2.0", id: PROMPT_REQUEST_ID, result: { stopReason: "end_turn" } },
    ],
    "native-resume": [
      initialize(),
      // The resumed conversation reports the native controls it already had.
      sessionOpened({ modes: { currentModeId: "agent", availableModes: [] } }),
      chunk(`synthetic resumed ${label} reply`),
      { jsonrpc: "2.0", id: PROMPT_REQUEST_ID, result: { stopReason: "end_turn" } },
    ],
    "user-cancel": [
      initialize(),
      sessionOpened({}),
      chunk(`synthetic ${label} reply`),
      { jsonrpc: "2.0", id: PROMPT_REQUEST_ID, result: { stopReason: "cancelled" } },
    ],
    "agent-error": [
      initialize(),
      sessionOpened({}),
      {
        jsonrpc: "2.0",
        id: PROMPT_REQUEST_ID,
        error: { code: -32603, message: "synthetic agent failure" },
      },
    ],
    "streaming-interruption": [
      initialize(),
      sessionOpened({}),
      chunk(`synthetic ${label} reply`),
      // Truncated wire: the parser must reject it at its own framing boundary.
      "{",
    ],
  };
}

function hermesScenarios() {
  const initialize = () =>
    initializeResponse({ name: "hermes-agent", version: "synthetic" });
  const sessionOpened = (result) => ({
    jsonrpc: "2.0",
    id: SESSION_REQUEST_ID,
    result: { sessionId: NATIVE_SESSION, ...result },
  });
  const chunk = (text) => agentMessageChunk(NATIVE_SESSION, text);
  return {
    "normal-turn": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic hermes "),
      chunk("reply"),
      promptResponse("end_turn"),
    ],
    "native-resume": [
      initialize(),
      // The resumed conversation exposes the native model and mode it already had.
      sessionOpened({
        models: { currentModelId: "synthetic-model", availableModels: [] },
        modes: { currentModeId: "agent", availableModes: [] },
      }),
      chunk("synthetic resumed hermes reply"),
      promptResponse("end_turn"),
    ],
    "user-cancel": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic hermes reply"),
      promptResponse("cancelled"),
    ],
    "agent-error": [
      initialize(),
      sessionOpened({}),
      promptError(-32603, "synthetic agent failure"),
    ],
    "streaming-interruption": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic hermes reply"),
      // Truncated wire: the parser must reject it at its own framing boundary.
      "{",
    ],
  };
}

function openclawScenarios() {
  const initialize = () =>
    initializeResponse({ name: "openclaw-acp", version: "synthetic" });
  const sessionOpened = (result) => ({
    jsonrpc: "2.0",
    id: SESSION_REQUEST_ID,
    result: { sessionId: GATEWAY_SESSION_KEY, ...result },
  });
  const chunk = (text) => agentMessageChunk(GATEWAY_SESSION_KEY, text);
  const sessionKeyAnnouncement = {
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId: GATEWAY_SESSION_KEY,
      update: {
        sessionUpdate: "session_info_update",
        _meta: { sessionKey: GATEWAY_SESSION_KEY },
      },
    },
  };
  return {
    "normal-turn": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic openclaw "),
      chunk("reply"),
      promptResponse("end_turn"),
    ],
    "native-resume": [
      initialize(),
      // The Gateway announces the resumable key of the conversation being loaded.
      sessionKeyAnnouncement,
      // The resumed conversation reports the thought level it already had.
      sessionOpened({ modes: { currentModeId: "medium", availableModes: [] } }),
      chunk("synthetic resumed openclaw reply"),
      promptResponse("end_turn"),
    ],
    "user-cancel": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic openclaw reply"),
      promptResponse("cancelled"),
    ],
    "agent-error": [
      initialize(),
      sessionOpened({}),
      promptError(-32603, "synthetic agent failure"),
    ],
    "streaming-interruption": [
      initialize(),
      sessionOpened({}),
      chunk("synthetic openclaw reply"),
      // Truncated wire: the parser must reject it at its own framing boundary.
      "{",
    ],
  };
}

export const acpFrames = {
  copilot: { channel: "lf-ndjson-acp", scenarios: acpScenarios("copilot") },
  "kimi-code": { channel: "lf-ndjson-acp", scenarios: acpScenarios("kimi-code") },
  hermes: { channel: "stdio-jsonrpc-acp", scenarios: hermesScenarios() },
  openclaw: { channel: "gateway-jsonrpc-acp", scenarios: openclawScenarios() },
};
