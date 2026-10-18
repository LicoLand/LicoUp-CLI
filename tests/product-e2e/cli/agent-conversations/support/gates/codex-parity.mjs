import { spawn, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { parityEffortForAgent, parityModelForAgent } from "../parity/agent-ids.mjs";

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../../..");
const timeoutMs = Number(process.env.LICO_CODEX_PARITY_TIMEOUT_MS || 180_000);
const maxOutputBytes = 4 * 1024 * 1024;
const sidecarArgs = ["agent", "conversation", "send", "--stdin-json", "true"];
const acceptanceMode = "dispatch-lane-unified-1";

class ParityFailure extends Error {
  constructor(code) {
    super(code);
    this.code = code;
  }
}

function requireCondition(condition, code) {
  if (!condition) {
    throw new ParityFailure(code);
  }
}

function safeDiagnosticCode(value) {
  const normalized = String(value || "unspecified").toLowerCase();
  return /^[a-z0-9][a-z0-9_-]{0,47}$/u.test(normalized)
    ? normalized
    : "unspecified";
}

function classifyDiagnosticMessage(value) {
  const message = String(value || "").toLowerCase();
  const categories = [
    ["authentication", /auth|login|credential|unauthorized|token/u],
    ["quota_or_rate", /quota|rate.?limit|usage.?limit/u],
    ["model", /model/u],
    ["network", /network|connect|dns|tls|socket/u],
    ["permission", /permission|sandbox|denied|forbidden/u],
    ["cancelled", /cancel/u],
    ["context", /context|too.?long/u],
    ["service", /server|service|internal|unavailable/u],
  ];
  return categories.find(([, pattern]) => pattern.test(message))?.[0] || "unspecified";
}

function resolveCodexBinary() {
  for (const candidate of [process.env.CODEX_PATH, process.env.CODEX_BIN]) {
    if (candidate && existsSync(candidate)) {
      return candidate;
    }
  }
  const located = spawnSync("which", ["codex"], { encoding: "utf8" });
  if (located.status === 0 && located.stdout.trim()) {
    return located.stdout.trim();
  }
  throw new ParityFailure("codex_executable_unavailable");
}

function resolveSidecarBinary() {
  const candidates = [
    process.env.LICO_CLIENT_PATH,
    join(workspaceRoot, "target", "debug", "licoup-cli"),
    join(workspaceRoot, "crates", "licoup-native", "target", "debug", "licoup-cli"),
  ].filter(Boolean);
  const binary = candidates.find((candidate) => existsSync(candidate));
  if (!binary) {
    throw new ParityFailure("lico_client_executable_unavailable");
  }
  return binary;
}

function privateWrapper(tempDirectory, realCodex) {
  const wrapperPath = join(tempDirectory, "codex-parity-wrapper");
  const capturePath = join(tempDirectory, "codex-argv-capture");
  writeFileSync(
    wrapperPath,
    [
      "#!/bin/sh",
      "{",
      "  printf '%s\\n' '__INVOCATION__'",
      "  for arg in \"$@\"; do printf '%s\\n' \"$arg\"; done",
      "} >> \"$LICO_CODEX_ARGV_CAPTURE\"",
      "exec \"$LICO_REAL_CODEX\" \"$@\"",
      "",
    ].join("\n"),
    { mode: 0o700 },
  );
  chmodSync(wrapperPath, 0o700);
  return {
    wrapperPath,
    capturePath,
    environment: {
      ...process.env,
      LICO_CODEX_ARGV_CAPTURE: capturePath,
      LICO_REAL_CODEX: realCodex,
    },
  };
}

class AppServerClient {
  constructor(executable, environment) {
    this.nextId = 1;
    this.pending = new Map();
    this.notifications = [];
    this.notificationWaiters = [];
    this.closedIntentionally = false;
    this.failure = null;
    this.outputBytes = 0;
    this.stderrBytes = 0;
    this.child = spawn(executable, ["app-server", "--stdio"], {
      cwd: workspaceRoot,
      env: environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.once("error", () => this.abort("app_server_start_failed"));
    this.child.once("close", () => {
      if (!this.closedIntentionally) {
        this.abort("app_server_exited_early");
      }
    });
    this.child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > maxOutputBytes) {
        this.abort("app_server_stderr_limit");
      }
    });
    const lines = createInterface({ input: this.child.stdout });
    lines.on("line", (line) => {
      this.outputBytes += Buffer.byteLength(line) + 1;
      if (this.outputBytes > maxOutputBytes) {
        this.abort("app_server_stdout_limit");
        return;
      }
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        this.abort("app_server_invalid_json");
        return;
      }
      this.handleMessage(message);
    });
  }

  handleMessage(message) {
    if (Object.hasOwn(message, "id") && !message.method) {
      const pending = this.pending.get(String(message.id));
      if (!pending) {
        return;
      }
      this.pending.delete(String(message.id));
      clearTimeout(pending.timer);
      if (message.error) {
        pending.reject(new ParityFailure("app_server_request_failed"));
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    if (Object.hasOwn(message, "id") && message.method) {
      this.write({
        id: message.id,
        error: { code: -32001, message: "Live parity acceptance does not approve interactions." },
      });
      this.abort("app_server_interaction_required");
      return;
    }
    if (!message.method) {
      return;
    }
    const waiterIndex = this.notificationWaiters.findIndex((waiter) => waiter.matches(message));
    if (waiterIndex >= 0) {
      const [waiter] = this.notificationWaiters.splice(waiterIndex, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(message);
    } else {
      this.notifications.push(message);
    }
  }

  write(message) {
    if (this.failure || !this.child.stdin.writable) {
      throw new ParityFailure(this.failure || "app_server_stdin_closed");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  request(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        this.pending.delete(String(id));
        rejectRequest(new ParityFailure("app_server_request_timeout"));
        this.abort("app_server_request_timeout");
      }, timeoutMs);
      this.pending.set(String(id), { resolve: resolveRequest, reject: rejectRequest, timer });
      try {
        this.write({ id, method, params });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(String(id));
        rejectRequest(error);
      }
    });
  }

  notify(method, params = undefined) {
    this.write(params === undefined ? { method } : { method, params });
  }

  waitForNotification(matches) {
    const queuedIndex = this.notifications.findIndex(matches);
    if (queuedIndex >= 0) {
      return Promise.resolve(this.notifications.splice(queuedIndex, 1)[0]);
    }
    return new Promise((resolveNotification, rejectNotification) => {
      const waiter = {
        matches,
        resolve: resolveNotification,
        reject: rejectNotification,
        timer: null,
      };
      waiter.timer = setTimeout(() => {
        const index = this.notificationWaiters.indexOf(waiter);
        if (index >= 0) {
          this.notificationWaiters.splice(index, 1);
        }
        rejectNotification(new ParityFailure("app_server_notification_timeout"));
        this.abort("app_server_notification_timeout");
      }, timeoutMs);
      this.notificationWaiters.push(waiter);
    });
  }

  abort(code) {
    if (this.failure) {
      return;
    }
    this.failure = code;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new ParityFailure(code));
    }
    this.pending.clear();
    for (const waiter of this.notificationWaiters) {
      clearTimeout(waiter.timer);
      waiter.reject(new ParityFailure(code));
    }
    this.notificationWaiters = [];
    this.child.kill();
  }

  async close() {
    this.closedIntentionally = true;
    this.child.stdin.end();
    if (this.child.exitCode === null) {
      this.child.kill();
    }
    await Promise.race([
      new Promise((resolveClose) => this.child.once("close", resolveClose)),
      new Promise((resolveClose) => setTimeout(resolveClose, 2_000)),
    ]);
  }
}

async function initialize(client) {
  await client.request("initialize", {
    clientInfo: { name: "lico-up-parity", title: "LicoUp Parity", version: "1" },
    capabilities: { experimentalApi: true },
  });
  client.notify("initialized");
}

function finalAgentMessage(turn) {
  const items = Array.isArray(turn?.items) ? turn.items : [];
  for (let index = items.length - 1; index >= 0; index -= 1) {
    if (items[index]?.type === "agentMessage" && typeof items[index].text === "string") {
      return items[index].text.trim();
    }
  }
  return "";
}

async function runTurn(client, threadId, prompt, model, reasoningEffort) {
  const result = await client.request("turn/start", {
    threadId,
    input: [{ type: "text", text: prompt }],
    ...(model ? { model } : {}),
    ...(reasoningEffort ? { effort: reasoningEffort } : {}),
  });
  const turnId = result?.turn?.id;
  requireCondition(typeof turnId === "string" && turnId.length > 0, "native_turn_id_missing");
  const completed = await client.waitForNotification(
    (message) => message.method === "turn/completed"
      && message.params?.threadId === threadId
      && message.params?.turn?.id === turnId,
  );
  const turnStatus = String(completed.params?.turn?.status || "missing")
    .toLowerCase()
    .replaceAll(/[^a-z0-9_-]/gu, "_")
    .slice(0, 32);
  const nativeTurnError = completed.params?.turn?.error;
  const nativeErrorCode = safeDiagnosticCode(
    nativeTurnError?.code
      || nativeTurnError?.type
      || nativeTurnError?.kind,
  );
  const nativeErrorCategory = nativeErrorCode === "unspecified"
    ? classifyDiagnosticMessage(nativeTurnError?.message)
    : nativeErrorCode;
  requireCondition(
    turnStatus === "completed",
    `native_turn_status_${turnStatus || "missing"}_${nativeErrorCategory}`,
  );
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

function runSidecarCommand(executable, args, environment, stdinText = "") {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(executable, args, {
      cwd: workspaceRoot,
      env: environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = Buffer.alloc(0);
    let stderrBytes = 0;
    let settled = false;
    const timer = setTimeout(() => {
      child.kill();
      if (!settled) {
        settled = true;
        rejectRun(new ParityFailure("lico_client_timeout"));
      }
    }, timeoutMs);
    child.once("error", () => {
      clearTimeout(timer);
      if (!settled) {
        settled = true;
        rejectRun(new ParityFailure("lico_client_start_failed"));
      }
    });
    child.stdout.on("data", (chunk) => {
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.length > maxOutputBytes) {
        child.kill();
      }
    });
    child.stderr.on("data", (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes > maxOutputBytes) {
        child.kill();
      }
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (settled) {
        return;
      }
      settled = true;
      if (stdout.length > maxOutputBytes || stderrBytes > maxOutputBytes) {
        rejectRun(new ParityFailure("lico_client_output_limit"));
        return;
      }
      if (code !== 0) {
        rejectRun(new ParityFailure("lico_client_failed"));
        return;
      }
      try {
        resolveRun(JSON.parse(stdout.toString("utf8")));
      } catch {
        rejectRun(new ParityFailure("lico_client_invalid_json"));
      }
    });
    child.stdin.end(stdinText);
  });
}

function runSidecar(executable, request, environment) {
  return runSidecarCommand(
    executable,
    sidecarArgs,
    environment,
    JSON.stringify(request),
  );
}

function threadAgentMessages(thread) {
  const turns = Array.isArray(thread?.turns) ? thread.turns : [];
  return turns
    .flatMap((turn) => Array.isArray(turn?.items) ? turn.items : [])
    .filter((item) => item?.type === "agentMessage" && typeof item.text === "string")
    .map((item) => item.text.trim());
}

function projectedAgentMessages(result, threadId) {
  const sessions = Array.isArray(result?.sessions) ? result.sessions : [];
  const session = sessions.find((candidate) => candidate?.id === threadId
    || candidate?.nativeSessionId === threadId);
  const messages = Array.isArray(session?.messages) ? session.messages : [];
  return messages
    .filter((message) => ["agent", "assistant"].includes(String(message?.role || "").toLowerCase()))
    .map((message) => String(message?.text || "").trim());
}

function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(stableJson).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

async function deleteThread(executable, environment, threadId) {
  const client = new AppServerClient(executable, environment);
  try {
    await initialize(client);
    await client.request("thread/delete", { threadId });
    return true;
  } catch {
    return false;
  } finally {
    await client.close();
  }
}

let stage = "preflight";
let threadId = "";
let cleanupCompleted = false;
let nativeClient = null;
let inspectionClient = null;
let wrapper = null;
const tempDirectory = mkdtempSync(join(tmpdir(), "lico-codex-parity-"));

try {
  const realCodex = resolveCodexBinary();
  const sidecar = resolveSidecarBinary();
  wrapper = privateWrapper(tempDirectory, realCodex);
  const nativePrompt = "Hi";
  const arcPrompt = "Hi";

  const model = parityModelForAgent("codex");
  // Some Codex models reject turn/start when reasoning.effort is omitted or
  // invalid, so the paired effort comes from the model authority.
  const reasoningEffort = parityEffortForAgent("codex", model);

  stage = "native-entry";
  nativeClient = new AppServerClient(wrapper.wrapperPath, wrapper.environment);
  await initialize(nativeClient);
  const started = await nativeClient.request("thread/start", {
    cwd: workspaceRoot,
    ...(model ? { model } : {}),
  });
  threadId = started?.thread?.id || "";
  requireCondition(threadId.length > 0, "native_thread_id_missing");
  const effectiveModel = typeof started.model === "string" && started.model.length > 0
    ? started.model
    : model;
  const startedEffort = typeof started.reasoningEffort === "string"
    ? started.reasoningEffort
    : "";
  const turnEffort = reasoningEffort || startedEffort;
  const effectiveReasoning = turnEffort;
  const nativeTurn = await runTurn(nativeClient, threadId, nativePrompt, model, turnEffort);
  const nativeOutput = finalAgentMessage(nativeTurn);
  await nativeClient.close();
  nativeClient = null;

  stage = "lico-up-entry";
  const sidecarResult = await runSidecar(
    sidecar,
    {
      agent: "codex",
      text: arcPrompt,
      sessionId: threadId,
      workingDirectory: workspaceRoot,
      binaryPath: wrapper.wrapperPath,
      model,
      ...(turnEffort ? { reasoningEffort: turnEffort } : {}),
      acceptanceMode,
    },
    {
      ...wrapper.environment,
      LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
    },
  );

  stage = "history-read";
  inspectionClient = new AppServerClient(wrapper.wrapperPath, wrapper.environment);
  await initialize(inspectionClient);
  const readResult = await inspectionClient.request("thread/read", {
    threadId,
    includeTurns: true,
  });
  const nativeHistoryMessages = threadAgentMessages(readResult?.thread);
  const projectionResult = await runSidecarCommand(
    sidecar,
    ["conversations", "list", "--agent", "codex", "--limit", "20"],
    wrapper.environment,
  );
  const projectedHistoryMessages = projectedAgentMessages(projectionResult, threadId);
  const sidecarOutput = String(sidecarResult?.output || "");
  const historyParity = nativeHistoryMessages.includes(nativeOutput)
    && nativeHistoryMessages.includes(sidecarOutput)
    && projectedHistoryMessages.includes(nativeOutput)
    && projectedHistoryMessages.includes(sidecarOutput);
  await inspectionClient.request("thread/delete", { threadId });
  cleanupCompleted = true;
  await inspectionClient.close();
  inspectionClient = null;

  stage = "parity-assertions";
  const sameThread = sidecarResult?.threadId === threadId && sidecarResult?.sessionId === threadId;
  const finalReplyParity = nativeOutput.trim().length > 0 && sidecarOutput.trim().length > 0;
  const settingsParity = sidecarResult?.model === effectiveModel
    && (sidecarResult?.reasoningEffort || "") === (effectiveReasoning || "")
    && sidecarResult?.workingDirectory === started.cwd
    && sidecarResult?.approvalPolicy === started.approvalPolicy
    && stableJson(sidecarResult?.sandbox) === stableJson(started.sandbox);
  const capture = existsSync(wrapper.capturePath) ? readFileSync(wrapper.capturePath, "utf8") : "";
  const promptAbsentFromArgv = !capture.split(/\r?\n/u).some((line) => line === "Hi")
    && !sidecarArgs.some((argument) => argument === "Hi");
  const canonicalTransport = sidecarResult?.ok === true
    && sidecarResult?.schemaVersion === 3
    && sidecarResult?.runtimeProtocol === "codex-app-server-stdio-jsonrpc"
    && sidecarResult?.turnStatus === "completed";

  requireCondition(canonicalTransport, "canonical_transport_mismatch");
  requireCondition(sameThread, "thread_identity_mismatch");
  requireCondition(nativeOutput.length > 0, "native_final_reply_missing");
  requireCondition(sidecarOutput.trim().length > 0, "lico_up_final_reply_missing");
  requireCondition(settingsParity, "effective_settings_mismatch");
  requireCondition(historyParity, "history_projection_mismatch");
  requireCondition(promptAbsentFromArgv, "prompt_exposed_in_argv");
  requireCondition(cleanupCompleted, "thread_cleanup_failed");

  console.log(JSON.stringify({
    ok: true,
    nativeEntryCompleted: true,
    licoUpEntryCompleted: true,
    canonicalTransport,
    sameThread,
    finalReplyParity,
    settingsParity,
    historyParity,
    promptAbsentFromArgv,
    cleanupCompleted,
  }));
} catch (error) {
  if (nativeClient) {
    await nativeClient.close();
  }
  if (inspectionClient) {
    await inspectionClient.close();
  }
  if (threadId && !cleanupCompleted && wrapper) {
    cleanupCompleted = await deleteThread(wrapper.wrapperPath, wrapper.environment, threadId);
  }
  console.error(JSON.stringify({
    ok: false,
    stage,
    code: error instanceof ParityFailure ? error.code : "unexpected_failure",
    cleanupCompleted: threadId ? cleanupCompleted : true,
  }));
  process.exitCode = 1;
} finally {
  rmSync(tempDirectory, { recursive: true, force: true });
}
