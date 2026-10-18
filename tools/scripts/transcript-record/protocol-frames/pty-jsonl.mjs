// Recorded vendor frames for this protocol family. See ../protocol-frames.mjs.
//
// Shape: { "<adapterId>": { channel: "<AdapterContract::framing>", scenarios: {
//   "<scenario>": [ <frame object> | "<raw wire string>" ] } } }
//
// The channel of every entry must equal the adapter's own `CONTRACT.framing`
// (`crates/licoup-native/src/platform/native_agent_parser/adapters/<id>.rs`).
// Content is synthetic: no real conversation, identity, or path is recorded.

// The conversation identities these transcripts use. Antigravity ids are 8..=128
// ASCII alphanumerics, `_` or `-`, so each is a valid native session id.
const NATIVE_SESSION = "11111111-2222-3333-4444-555555555555";
const RESUMED_SESSION = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const LICO_SESSION = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const DEEPSEEK_SESSION = "session-1";
const DEEPSEEK_RESUMED_SESSION = "resumed-session-1";

/// The Stop-hook receipt the Antigravity CLI and its LicoUp hook bridge write:
/// one direct JSON object carrying the native conversation id.
function hookReceipt(conversationId) {
  return { conversationId };
}

/// The PTY lane's terminal frame: the process outcome `classify_terminal`
/// consumes together with the launch-time resume binding. The vendor wire
/// carries neither — the driver takes them from the process it supervises — so
/// the turn's last recorded frame is where they are written down.
function terminal({ requestedSession, exitSuccess = true, timedOut = false } = {}) {
  return requestedSession === undefined
    ? { exitSuccess, timedOut }
    : { requestedSession, exitSuccess, timedOut };
}

/// A lico-agent RPC response line. `data` is the response's own payload.
function licoResponse(id, data) {
  return data === undefined
    ? { id, type: "response", success: true }
    : { id, type: "response", success: true, data };
}

/// One lico-agent assistant text delta, exactly as the RPC lane emits it.
function licoDelta(delta) {
  return { type: "message_update", assistantMessageEvent: { type: "text_delta", delta } };
}

/// The DeepSeek Harness SDK handshake response (`initialize_accepted` reads
/// `/result/serverInfo/name`).
const deepseekInitialize = {
  jsonrpc: "2.0",
  id: "initialize",
  result: { serverInfo: { name: "deepseek-harness-sdk-runtime" } },
};

function deepseekPromptAdmitted(requestId, messageId) {
  return { jsonrpc: "2.0", id: requestId, result: { messageId } };
}

function deepseekInboxSpliced(sessionId, messageId) {
  return {
    jsonrpc: "2.0",
    method: "session.event",
    params: {
      sessionId,
      event: { type: "agent/inbox/spliced", data: { inserted: [{ id: messageId }] } },
    },
  };
}

function deepseekAssistantMessage(sessionId, text) {
  return {
    jsonrpc: "2.0",
    method: "session.event",
    params: {
      sessionId,
      event: { type: "assistant/message", data: { message: { content: [{ type: "text", text }] } } },
    },
  };
}

function deepseekIdle(sessionId) {
  return { jsonrpc: "2.0", method: "session.status", params: { sessionId, status: "idle" } };
}

export const ptyJsonlFrames = {
  antigravity: {
    channel: "pty-hook-json",
    scenarios: {
      // The vendor CLI streams its answer on the pty, writes the Stop-hook
      // receipt its hook bridge installed, and exits successfully.
      "normal-turn": [
        "[32msynthetic reply[0m\n",
        hookReceipt(NATIVE_SESSION),
        terminal(),
      ],
      // The same turn resumed on the identity the caller already knew: the
      // receipt echoes the requested conversation and the terminal classifies
      // against it.
      "native-resume": [
        "[32msynthetic resumed reply[0m\n",
        hookReceipt(RESUMED_SESSION),
        terminal({ requestedSession: RESUMED_SESSION }),
      ],
      // Antigravity has no in-band cancel: cancellation is a signal to the
      // supervised process (`antigravity_cli_cancelled`), which `classify_terminal`
      // never sees. The interrupted CLI still writes its receipt and then exits
      // without a successful turn, which is the closest terminal fact the real
      // parser produces for a cancelled turn.
      "user-cancel": [
        "pre-cancel synthetic output\n",
        hookReceipt(NATIVE_SESSION),
        terminal({ exitSuccess: false }),
      ],
      // The CLI resumed a different native conversation than requested, which
      // is the drift the terminal classification rejects.
      "agent-error": [
        "synthetic drifted reply\n",
        hookReceipt(NATIVE_SESSION),
        terminal({ requestedSession: RESUMED_SESSION }),
      ],
      // Content, then a truncated receipt: the receipt boundary refuses the
      // malformed document, so its bytes stay ordinary pty output and the turn
      // can bind no native identity at all.
      "streaming-interruption": [
        "[32msynthetic partial reply[0m\n",
        '{"conversationId":"11111111-2222',
        terminal(),
      ],
    },
  },
  "lico-agent": {
    channel: "lf-jsonl-jsonrpc",
    scenarios: {
      // The readiness handshake answers with the native session id, the prompt
      // is admitted, the answer streams, and the turn ends.
      "normal-turn": [
        licoResponse("lico-1", {
          isRunning: false,
          profile: "base",
          sessionId: NATIVE_SESSION,
        }),
        licoResponse("lico-2"),
        licoDelta("synthetic reply"),
        { type: "agent_end" },
      ],
      // Resumed on the identity the caller already knew: the handshake response
      // reports it, and the driver fails the send if it differs.
      "native-resume": [
        licoResponse("lico-1", {
          isRunning: false,
          profile: "base",
          sessionId: LICO_SESSION,
        }),
        licoResponse("lico-2"),
        licoDelta("synthetic resumed reply"),
        { type: "agent_end" },
      ],
      // The RPC lane has no in-band cancel frame: cancellation tears the
      // supervised process down (the lane reports `agent_cancel_transport_unavailable`),
      // so the parser only ever sees the turn's own end. The recorded terminal is
      // its failure report for an aborted turn, which `code` leaves unset —
      // the driver maps exactly that shape to `lico_agent_turn_failed`.
      "user-cancel": [licoDelta("partial synthetic reply"), { type: "error" }],
      // The agent's own error terminal: the turn could not be persisted.
      "agent-error": [
        licoDelta("synthetic reply"),
        { type: "error", code: "lico_agent_transcript_persist_failed" },
      ],
      // The wire is truncated inside a frame, so the line parser rejects it at
      // its own framing boundary (`lico_agent_rpc_invalid_frame` live).
      "streaming-interruption": [licoDelta("partial synthetic reply"), "{"],
    },
  },
  "deepseek-harness": {
    channel: "lf-jsonl-jsonrpc",
    scenarios: {
      // The SDK handshake, then one admitted turn: the prompt response names
      // the message, the inbox receipt attributes it, the assistant message
      // carries the text, and the idle status settles the turn.
      "normal-turn": [
        deepseekInitialize,
        deepseekPromptAdmitted("prompt-1", "message-1"),
        deepseekInboxSpliced(DEEPSEEK_SESSION, "message-1"),
        deepseekAssistantMessage(DEEPSEEK_SESSION, "synthetic reply"),
        deepseekIdle(DEEPSEEK_SESSION),
      ],
      // The same turn resumed on an already-known native session: the transport
      // handshake runs again for that session, and its message counter continues
      // from the turn the session already had.
      "native-resume": [
        deepseekInitialize,
        deepseekPromptAdmitted("prompt-1", "message-2"),
        deepseekInboxSpliced(DEEPSEEK_RESUMED_SESSION, "message-2"),
        deepseekAssistantMessage(DEEPSEEK_RESUMED_SESSION, "synthetic resumed reply"),
        deepseekIdle(DEEPSEEK_RESUMED_SESSION),
      ],
      // Cancel is unavailable on this lane (`capabilityMatrix.cancel` is false),
      // so nothing in band ever says "cancelled". A turn the user stops before
      // the harness admits a message is what the driver's loop observes as an
      // incomplete turn: the prompt response carries no message id, which is the
      // real parser's `TurnParseError::Incomplete`.
      "user-cancel": [
        deepseekInitialize,
        deepseekInboxSpliced(DEEPSEEK_SESSION, "message-1"),
        { jsonrpc: "2.0", id: "prompt-1", result: {} },
      ],
      // Protocol activity bound to a different session than the turn's, which is
      // the real parser's `TurnParseError::SessionMismatch`.
      "agent-error": [
        deepseekInitialize,
        deepseekPromptAdmitted("prompt-1", "message-1"),
        deepseekInboxSpliced(DEEPSEEK_SESSION, "message-1"),
        deepseekAssistantMessage(DEEPSEEK_SESSION, "partial synthetic reply"),
        deepseekAssistantMessage("other-session", "synthetic reply"),
      ],
      // The stream is truncated inside a frame, which the real `FrameParser`
      // rejects at its own framing boundary before any turn fact exists.
      "streaming-interruption": [
        deepseekInitialize,
        deepseekPromptAdmitted("prompt-1", "message-1"),
        deepseekInboxSpliced(DEEPSEEK_SESSION, "message-1"),
        deepseekAssistantMessage(DEEPSEEK_SESSION, "partial synthetic reply"),
        "{",
      ],
    },
  },
};
