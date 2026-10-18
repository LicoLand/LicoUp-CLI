// Recorded vendor frames for the serve and Pi RPC protocol families. See
// ../protocol-frames.mjs.
//
// Shape: { "<adapterId>": { channel: "<AdapterContract::framing>", scenarios: {
//   "<scenario>": [ <frame object> | "<raw wire string>" ] } } }
//
// kilo-code and opencode are the same HTTP serve wire (Kilo Code serves the
// OpenCode HTTP API): the session endpoints answer the identity document, the
// event stream carries `message.updated` / `message.part.updated` SSE payloads,
// and the message endpoint answers a whole-message document. Neither adapter
// observes a cancel in band: a cancelled turn is the out-of-band
// `POST /session/{id}/abort` document followed by the message endpoint's
// partial answer, which is what the user-cancel scenarios record.
//
// pi frames are JSONL lines of `pi --mode rpc`.

const kiloSession = { id: "kilo-1", title: "synthetic-session" };
const kiloResumed = { id: "existing-kilo-native", title: "t" };
const openSession = { id: "open-1", title: "synthetic-session" };
const openResumed = { id: "expected-open-native", title: "t" };

function kiloAssistantSeen(sessionId, messageId) {
  return {
    type: "message.updated",
    properties: { info: { id: messageId, role: "assistant", sessionID: sessionId } },
  };
}

function kiloTextPart(sessionId, messageId, partId, text) {
  return {
    type: "message.part.updated",
    properties: {
      sessionID: sessionId,
      part: { id: partId, messageID: messageId, type: "text", text },
    },
  };
}

function openAssistantSeen(sessionId, messageId) {
  return {
    type: "message.updated",
    properties: { info: { id: messageId, role: "assistant", sessionID: sessionId } },
  };
}

function openTextPart(sessionId, messageId, partId, text) {
  return {
    type: "message.part.updated",
    properties: {
      sessionId,
      part: { id: partId, messageID: messageId, type: "text", text },
    },
  };
}

const kiloAgentError = { parts: [{ type: "reasoning", text: "<REDACTED_ERROR>" }] };
const openAgentError = { parts: [{ type: "reasoning", text: "<REDACTED_ERROR>" }] };

export const serveFrames = {
  "kilo-code": {
    channel: "http-sse",
    scenarios: {
      "normal-turn": [
        kiloSession,
        kiloAssistantSeen("kilo-1", "msg-kilo-1"),
        kiloTextPart("kilo-1", "msg-kilo-1", "prt-kilo-1", "synthetic "),
        kiloTextPart("kilo-1", "msg-kilo-1", "prt-kilo-2", "reply"),
        { parts: [{ type: "text", text: "synthetic reply" }] },
      ],
      "native-resume": [
        kiloResumed,
        kiloAssistantSeen("existing-kilo-native", "msg-kilo-2"),
        kiloTextPart("existing-kilo-native", "msg-kilo-2", "prt-kilo-3", "resumed "),
        kiloTextPart("existing-kilo-native", "msg-kilo-2", "prt-kilo-4", "reply"),
        { parts: [{ type: "text", text: "resumed reply" }] },
      ],
      "user-cancel": [
        kiloSession,
        kiloAssistantSeen("kilo-1", "msg-kilo-3"),
        kiloTextPart("kilo-1", "msg-kilo-3", "prt-kilo-5", "synthetic "),
        // The abort endpoint's own document; the serve parser has no ingress
        // for the out-of-band control lane.
        { aborted: true },
        { parts: [{ type: "text", text: "synthetic " }] },
      ],
      "agent-error": [
        kiloSession,
        kiloAssistantSeen("kilo-1", "msg-kilo-4"),
        kiloTextPart("kilo-1", "msg-kilo-4", "prt-kilo-6", "synthetic partial"),
        kiloAgentError,
      ],
      "streaming-interruption": [
        kiloSession,
        kiloAssistantSeen("kilo-1", "msg-kilo-5"),
        kiloTextPart("kilo-1", "msg-kilo-5", "prt-kilo-7", "synthetic partial"),
        "{",
      ],
    },
  },
  opencode: {
    channel: "http-sse",
    scenarios: {
      "normal-turn": [
        openSession,
        openAssistantSeen("open-1", "msg-open-1"),
        openTextPart("open-1", "msg-open-1", "prt-open-1", "synthetic "),
        openTextPart("open-1", "msg-open-1", "prt-open-2", "reply"),
        { parts: [{ type: "text", text: "synthetic reply" }] },
      ],
      "native-resume": [
        openResumed,
        openAssistantSeen("expected-open-native", "msg-open-2"),
        openTextPart("expected-open-native", "msg-open-2", "prt-open-3", "resumed "),
        openTextPart("expected-open-native", "msg-open-2", "prt-open-4", "reply"),
        { parts: [{ type: "text", text: "resumed reply" }] },
      ],
      "user-cancel": [
        openSession,
        openAssistantSeen("open-1", "msg-open-3"),
        openTextPart("open-1", "msg-open-3", "prt-open-5", "synthetic "),
        // The abort endpoint's own document; the serve parser has no ingress
        // for the out-of-band control lane.
        { aborted: true },
        { parts: [{ type: "text", text: "synthetic " }] },
      ],
      "agent-error": [
        openSession,
        openAssistantSeen("open-1", "msg-open-4"),
        openTextPart("open-1", "msg-open-4", "prt-open-6", "synthetic partial"),
        openAgentError,
      ],
      "streaming-interruption": [
        openSession,
        openAssistantSeen("open-1", "msg-open-5"),
        openTextPart("open-1", "msg-open-5", "prt-open-7", "synthetic partial"),
        "{",
      ],
    },
  },
  pi: {
    channel: "lf-jsonl-rpc",
    scenarios: {
      "normal-turn": [
        {
          id: "lico-pi-initial-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-synthetic-1" },
        },
        { id: "lico-pi-prompt", type: "response", command: "prompt", success: true },
        {
          type: "message_update",
          assistantMessageEvent: { type: "text_delta", delta: "synthetic " },
        },
        {
          type: "message_update",
          assistantMessageEvent: { type: "text_delta", delta: "reply" },
        },
        { type: "agent_settled" },
        {
          id: "lico-pi-assistant",
          type: "response",
          command: "get_last_assistant_text",
          success: true,
          data: { text: "synthetic reply" },
        },
        {
          id: "lico-pi-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-synthetic-1" },
        },
      ],
      "native-resume": [
        {
          id: "lico-pi-switch",
          type: "response",
          command: "switch_session",
          success: true,
          data: { cancelled: false },
        },
        {
          id: "lico-pi-switched-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-native-resume-1" },
        },
        { id: "lico-pi-prompt", type: "response", command: "prompt", success: true },
        {
          type: "message_update",
          assistantMessageEvent: { type: "text_delta", delta: "resumed reply" },
        },
        { type: "agent_settled" },
        {
          id: "lico-pi-assistant",
          type: "response",
          command: "get_last_assistant_text",
          success: true,
          data: { text: "resumed reply" },
        },
        {
          id: "lico-pi-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-native-resume-1" },
        },
      ],
      // Pi reports the cancelled switch in band; the driver maps it to the
      // `pi_session_switch_cancelled` terminal.
      "user-cancel": [
        {
          id: "lico-pi-switch",
          type: "response",
          command: "switch_session",
          success: true,
          data: { cancelled: true },
        },
      ],
      "agent-error": [
        {
          id: "lico-pi-initial-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-error-1" },
        },
        { id: "lico-pi-prompt", type: "response", command: "prompt", success: true },
        {
          type: "message_end",
          message: {
            role: "assistant",
            content: [],
            stopReason: "error",
            errorMessage: "503: {\"code\":\"gateway_credential_unavailable\"}",
          },
        },
        { type: "agent_settled" },
        {
          id: "lico-pi-assistant",
          type: "response",
          command: "get_last_assistant_text",
          success: true,
          data: { text: null },
        },
        {
          id: "lico-pi-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-error-1" },
        },
      ],
      "streaming-interruption": [
        {
          id: "lico-pi-initial-state",
          type: "response",
          command: "get_state",
          success: true,
          data: { sessionId: "pi-synthetic-2" },
        },
        { id: "lico-pi-prompt", type: "response", command: "prompt", success: true },
        {
          type: "message_update",
          assistantMessageEvent: { type: "text_delta", delta: "synthetic partial" },
        },
        "{",
      ],
    },
  },
};
