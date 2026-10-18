// Recorded vendor frames for this protocol family. See ../protocol-frames.mjs.
//
// Shape: { "<adapterId>": { channel: "<AdapterContract::framing>", scenarios: {
//   "<scenario>": [ <frame object> | "<raw wire string>" ] } } }
//
// `channel` is each adapter's own `AdapterContract::framing`: Cursor's turn
// stream is strict LF NDJSON, Claude Code's is LF NDJSON, and the Codex
// app-server session is stdio JSON-RPC. Payloads are the agent-to-client frames
// that crossed that boundary, copied from this repository's own parser and
// driver tests; the truncated final frame is a raw string so the parser's own
// framing failure is what the replay reaches. Every value is synthetic.

const content = "synthetic reply";
const prompt = "synthetic-user-prompt";

const cursorInit = (session) => ({
  type: "system",
  subtype: "init",
  apiKeySource: "synthetic",
  cwd: "/workspace/project",
  session_id: session,
  model: "synthetic-model",
  permissionMode: "default",
});

const cursorEcho = (session) => ({
  type: "user",
  session_id: session,
  message: {
    role: "user",
    content: [{ type: "text", text: prompt }],
  },
});

const cursorDelta = (session, timestamp, text) => ({
  type: "assistant",
  session_id: session,
  timestamp_ms: timestamp,
  message: {
    role: "assistant",
    content: [{ type: "text", text }],
  },
});

const cursorTerminal = (session, subtype, requestId, isError) => ({
  type: "result",
  subtype,
  is_error: isError,
  request_id: requestId,
  result: content,
  session_id: session,
});

const claudeInit = (session) => ({
  type: "system",
  subtype: "init",
  session_id: session,
  model: "synthetic-model",
  permissionMode: "plan",
});

const claudeAssistant = (session) => ({
  type: "assistant",
  session_id: session,
  message: {
    role: "assistant",
    content: [{ type: "text", text: content }],
  },
});

const claudeTerminal = (session, subtype, result, isError) => ({
  type: "result",
  subtype,
  is_error: isError,
  result,
  session_id: session,
  permission_denials: [],
});

const codexInitialize = {
  id: 1,
  result: {
    userAgent: "codex-test",
    platformFamily: "test",
    platformOs: "test",
    codexHome: "/redacted",
  },
};

const codexRateLimits = {
  id: 4,
  result: { rateLimits: { primary: { usedPercent: 0.0 } } },
};

const codexThread = (threadId) => ({
  id: 2,
  result: {
    thread: { id: threadId, cwd: "/workspace/project" },
    cwd: "/workspace/project",
    sandbox: { type: "workspaceWrite", writableRoots: [] },
    approvalPolicy: "on-request",
  },
});

const codexTurn = {
  id: 3,
  result: { turn: { id: "synthetic-turn", status: "inProgress", items: [] } },
};

const codexDelta = (threadId) => ({
  method: "item/agentMessage/delta",
  params: {
    threadId,
    turnId: "synthetic-turn",
    delta: content,
  },
});

const codexCompleted = (threadId, items) => ({
  method: "turn/completed",
  params: {
    threadId,
    turn: { id: "synthetic-turn", status: "completed", items },
  },
});

const codexAgentMessage = { id: "agent-1", type: "agentMessage", text: content };

const codexHandshake = (threadId) => [
  codexInitialize,
  codexRateLimits,
  codexThread(threadId),
  codexTurn,
];

export const streamJsonFrames = {
  cursor: {
    channel: "strict-lf-ndjson",
    scenarios: {
      // A fresh chat: the CLI's created identity is what the turn binds to.
      "normal-turn": [
        cursorInit("fresh-synthetic-session"),
        cursorEcho("fresh-synthetic-session"),
        cursorDelta("fresh-synthetic-session", 1, "synthetic "),
        cursorDelta("fresh-synthetic-session", 2, "reply"),
        cursorTerminal("fresh-synthetic-session", "success", "req-1", false),
      ],
      // The same turn resumed on the identity the launch already knew.
      "native-resume": [
        cursorInit("synthetic-session"),
        cursorEcho("synthetic-session"),
        cursorDelta("synthetic-session", 1, "synthetic "),
        cursorDelta("synthetic-session", 2, "reply"),
        cursorTerminal("synthetic-session", "success", "req-2", false),
      ],
      // The terminal Cursor emits for an interrupted turn.
      "user-cancel": [
        cursorInit("synthetic-session"),
        cursorEcho("synthetic-session"),
        cursorDelta("synthetic-session", 1, "synthetic "),
        cursorTerminal(
          "synthetic-session",
          "error_during_execution",
          "req-3",
          true,
        ),
      ],
      // The terminal Cursor emits when the provider rejects the turn.
      "agent-error": [
        cursorInit("synthetic-session"),
        cursorEcho("synthetic-session"),
        cursorTerminal(
          "synthetic-session",
          "authentication_required",
          "req-4",
          true,
        ),
      ],
      // Assistant content, then a result frame cut off mid-write.
      "streaming-interruption": [
        cursorInit("synthetic-session"),
        cursorEcho("synthetic-session"),
        cursorDelta("synthetic-session", 1, "synthetic "),
        '{"type":"result"',
      ],
    },
  },
  "claude-code": {
    channel: "lf-ndjson",
    scenarios: {
      "normal-turn": [
        claudeInit("fresh-synthetic-session"),
        claudeAssistant("fresh-synthetic-session"),
        claudeTerminal("fresh-synthetic-session", "success", content, false),
      ],
      "native-resume": [
        claudeInit("synthetic-session"),
        claudeAssistant("synthetic-session"),
        claudeTerminal("synthetic-session", "success", content, false),
      ],
      "user-cancel": [
        claudeInit("synthetic-session"),
        claudeAssistant("synthetic-session"),
        claudeTerminal(
          "synthetic-session",
          "error_during_execution",
          "interrupted",
          true,
        ),
      ],
      "agent-error": [
        claudeInit("synthetic-session"),
        claudeTerminal(
          "synthetic-session",
          "authentication_required",
          "synthetic authentication failure",
          true,
        ),
      ],
      "streaming-interruption": [
        claudeInit("synthetic-session"),
        claudeAssistant("synthetic-session"),
        '{"type":"result"',
      ],
    },
  },
  codex: {
    channel: "stdio-jsonrpc",
    scenarios: {
      "normal-turn": [
        ...codexHandshake("fresh-synthetic-thread"),
        codexDelta("fresh-synthetic-thread"),
        codexCompleted("fresh-synthetic-thread", [codexAgentMessage]),
      ],
      "native-resume": [
        ...codexHandshake("synthetic-session"),
        codexDelta("synthetic-session"),
        codexCompleted("synthetic-session", [codexAgentMessage]),
      ],
      "user-cancel": [
        ...codexHandshake("synthetic-session"),
        {
          method: "turn/completed",
          params: {
            threadId: "synthetic-session",
            turn: { id: "synthetic-turn", status: "interrupted", items: [] },
          },
        },
      ],
      "agent-error": [
        ...codexHandshake("synthetic-session"),
        {
          method: "turn/completed",
          params: {
            threadId: "synthetic-session",
            turn: {
              id: "synthetic-turn",
              status: "failed",
              items: [],
              error: {
                message: "synthetic provider failure",
                codexErrorInfo: "Unauthorized",
                additionalDetails: "synthetic detail",
              },
            },
          },
        },
      ],
      "streaming-interruption": [
        ...codexHandshake("synthetic-session"),
        codexDelta("synthetic-session"),
        '{"method":"turn/completed","params":{"threadId":"synthetic-session"',
      ],
    },
  },
};
