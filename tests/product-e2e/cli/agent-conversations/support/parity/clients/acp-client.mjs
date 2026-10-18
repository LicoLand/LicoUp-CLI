import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { createInterface } from "node:readline";
import { AcceptanceError, requireFact, safeErrorCode } from "../errors.mjs";

export const promptQuietMs = 100;
const maxAcpMessageBytes = 1024 * 1024;
const maxSessionIdBytes = 1024;
const sessionUpdateKinds = new Set([
  "user_message_chunk",
  "agent_message_chunk",
  "agent_thought_chunk",
  "tool_call",
  "tool_call_update",
  "plan",
  "available_commands_update",
  "current_mode_update",
  "config_option_update",
  "session_info_update",
  "usage_update",
]);

function isJsonObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function hasExactKeys(value, required, optional = []) {
  if (!isJsonObject(value)) return false;
  const keys = Object.keys(value);
  const allowed = new Set([...required, ...optional]);
  return required.every((key) => Object.hasOwn(value, key))
    && keys.every((key) => allowed.has(key));
}

function isNormalizedBoundedText(value, maxBytes) {
  return typeof value === "string"
    && value.length > 0
    && value.trim() === value
    && Buffer.byteLength(value) <= maxBytes;
}

function isU64(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function isSupportedContentBlock(content) {
  if (!isJsonObject(content) || typeof content.type !== "string") return false;
  switch (content.type) {
    case "text":
      return typeof content.text === "string";
    case "image":
    case "audio":
      return typeof content.data === "string" && typeof content.mimeType === "string";
    case "resource_link":
      return typeof content.uri === "string" && typeof content.name === "string";
    case "resource": {
      const resource = content.resource;
      return isJsonObject(resource)
        && typeof resource.uri === "string"
        && (typeof resource.text === "string" || typeof resource.blob === "string");
    }
    default:
      return false;
  }
}

function hasValidMeta(value) {
  return !Object.hasOwn(value, "_meta")
    || value._meta === null
    || isJsonObject(value._meta);
}

function hasBoundedText(value, key, { nullable = false } = {}) {
  if (!Object.hasOwn(value, key)) return true;
  if (nullable && value[key] === null) return true;
  return isNormalizedBoundedText(value[key], maxSessionIdBytes);
}

function isSessionConfigSelectValue(value) {
  return isJsonObject(value)
    && isNormalizedBoundedText(value.value, maxSessionIdBytes)
    && isNormalizedBoundedText(value.name, maxSessionIdBytes)
    && hasBoundedText(value, "description", { nullable: true })
    && hasValidMeta(value);
}

function isSessionConfigSelectGroup(value) {
  return isJsonObject(value)
    && isNormalizedBoundedText(value.group, maxSessionIdBytes)
    && isNormalizedBoundedText(value.name, maxSessionIdBytes)
    && Array.isArray(value.options)
    && value.options.every(isSessionConfigSelectValue)
    && hasValidMeta(value);
}

function isSessionConfigOption(value) {
  if (!isJsonObject(value)
    || !isNormalizedBoundedText(value.id, maxSessionIdBytes)
    || !isNormalizedBoundedText(value.name, maxSessionIdBytes)
    || !hasBoundedText(value, "description", { nullable: true })
    || !hasBoundedText(value, "category", { nullable: true })
    || !hasValidMeta(value)) return false;
  if (value.type === "boolean") return typeof value.currentValue === "boolean";
  if (value.type !== "select"
    || !isNormalizedBoundedText(value.currentValue, maxSessionIdBytes)
    || !Array.isArray(value.options)) return false;
  const grouped = value.options.some((option) => Object.hasOwn(option || {}, "group"));
  return value.options.every(grouped ? isSessionConfigSelectGroup : isSessionConfigSelectValue);
}

function isToolCallContent(value) {
  if (!isJsonObject(value) || !hasValidMeta(value)) return false;
  if (value.type === "content") return isSupportedContentBlock(value.content);
  if (value.type === "terminal") {
    return isNormalizedBoundedText(value.terminalId, maxSessionIdBytes);
  }
  if (value.type === "diff") {
    return isNormalizedBoundedText(value.path, maxSessionIdBytes)
      && typeof value.newText === "string"
      && (!Object.hasOwn(value, "oldText")
        || value.oldText === null
        || typeof value.oldText === "string");
  }
  return false;
}

function hasValidToolCallFields(update, { titleRequired }) {
  if (!isNormalizedBoundedText(update.toolCallId, maxSessionIdBytes)
    || (titleRequired && !isNormalizedBoundedText(update.title, maxSessionIdBytes))
    || (!titleRequired && !hasBoundedText(update, "title", { nullable: true }))
    || !hasBoundedText(update, "kind", { nullable: true })
    || !hasBoundedText(update, "status", { nullable: true })
    || !hasValidMeta(update)) return false;
  if (Object.hasOwn(update, "content")
    && (!Array.isArray(update.content) || !update.content.every(isToolCallContent))) return false;
  if (Object.hasOwn(update, "locations")
    && (!Array.isArray(update.locations) || !update.locations.every((location) => (
      isJsonObject(location)
      && isNormalizedBoundedText(location.path, maxSessionIdBytes)
      && (!Object.hasOwn(location, "line")
        || location.line === null
        || (isU64(location.line) && location.line <= 0xffff_ffff))
      && hasValidMeta(location)
    )))) return false;
  return true;
}

function isPlanEntry(entry) {
  return isJsonObject(entry)
    && isNormalizedBoundedText(entry.content, maxSessionIdBytes)
    && ["high", "medium", "low"].includes(entry.priority)
    && ["pending", "in_progress", "completed"].includes(entry.status)
    && hasValidMeta(entry);
}

function isAvailableCommand(command) {
  if (!isJsonObject(command)
    || !isNormalizedBoundedText(command.name, maxSessionIdBytes)
    || !isNormalizedBoundedText(command.description, maxSessionIdBytes)
    || !hasValidMeta(command)) return false;
  if (!Object.hasOwn(command, "input") || command.input === null) return true;
  return isJsonObject(command.input)
    && isNormalizedBoundedText(command.input.hint, maxSessionIdBytes)
    && hasValidMeta(command.input);
}

function isStructuredSessionUpdate(update) {
  switch (update.sessionUpdate) {
    case "tool_call":
      return hasValidToolCallFields(update, { titleRequired: true });
    case "tool_call_update":
      return hasValidToolCallFields(update, { titleRequired: false });
    case "plan":
      return Array.isArray(update.entries)
        && update.entries.every(isPlanEntry)
        && hasValidMeta(update);
    case "available_commands_update":
      return Array.isArray(update.availableCommands)
        && update.availableCommands.every(isAvailableCommand)
        && hasValidMeta(update);
    case "current_mode_update":
      return isNormalizedBoundedText(update.currentModeId, maxSessionIdBytes)
        && hasValidMeta(update);
    case "config_option_update":
      return Array.isArray(update.configOptions)
        && update.configOptions.every(isSessionConfigOption)
        && hasValidMeta(update);
    case "session_info_update":
      return hasBoundedText(update, "title", { nullable: true })
        && hasBoundedText(update, "updatedAt", { nullable: true })
        && hasValidMeta(update);
    case "usage_update":
      return isU64(update.used)
        && isU64(update.size)
        && (!Object.hasOwn(update, "cost")
          || update.cost === null
          || (isJsonObject(update.cost)
            && Number.isFinite(update.cost.amount)
            && update.cost.amount >= 0
            && isNormalizedBoundedText(update.cost.currency, maxSessionIdBytes)
            && hasValidMeta(update.cost)))
        && hasValidMeta(update);
    default:
      return true;
  }
}

export function promptNotificationError(notification, expectedSessionId) {
  if (notification?.method !== "session/update") return null;
  if (!isJsonObject(notification)) return "acp_notification_envelope_invalid";
  if (Buffer.byteLength(JSON.stringify(notification)) > maxAcpMessageBytes) {
    return "acp_message_too_large";
  }
  if (!hasExactKeys(notification, ["jsonrpc", "method", "params"])) {
    return "acp_notification_envelope_invalid";
  }
  if (notification.jsonrpc !== "2.0") return "acp_jsonrpc_version_invalid";
  if (!hasExactKeys(notification.params, ["sessionId", "update"], ["_meta"])) {
    return "acp_session_update_invalid";
  }
  const meta = notification.params._meta;
  if (meta !== undefined && meta !== null && !isJsonObject(meta)) {
    return "acp_session_update_invalid";
  }
  const sessionId = notification.params.sessionId;
  if (typeof sessionId !== "string") return "acp_session_update_invalid";
  if (!isNormalizedBoundedText(sessionId, maxSessionIdBytes)
    || !isNormalizedBoundedText(expectedSessionId, maxSessionIdBytes)) {
    return "acp_session_id_invalid";
  }
  if (sessionId !== expectedSessionId) {
    return "acp_session_mismatch";
  }
  const update = notification.params.update;
  if (!isJsonObject(update) || !sessionUpdateKinds.has(update.sessionUpdate)) {
    return "acp_session_update_invalid";
  }
  if (["user_message_chunk", "agent_message_chunk", "agent_thought_chunk"]
    .includes(update.sessionUpdate)
    && (!isSupportedContentBlock(update.content) || !hasValidMeta(update))) {
    return "acp_session_update_invalid";
  }
  if (!isStructuredSessionUpdate(update)) {
    return "acp_session_update_invalid";
  }
  return "";
}

export function createPromptQuiescenceBudget(promptResponseAt, hardDeadlineAt) {
  requireFact(
    Number.isFinite(promptResponseAt)
      && Number.isFinite(hardDeadlineAt)
      && promptResponseAt <= hardDeadlineAt,
    "acp_prompt_deadline_invalid",
  );
  return Object.freeze({
    hardDeadlineAt,
    lastValidNotificationAt: promptResponseAt,
    quietDeadlineAt: Math.min(promptResponseAt + promptQuietMs, hardDeadlineAt),
  });
}

export function resetPromptQuiescenceBudget(budget, validNotificationAt) {
  requireFact(
    budget
      && Number.isFinite(budget.hardDeadlineAt)
      && Number.isFinite(budget.lastValidNotificationAt)
      && Number.isFinite(validNotificationAt)
      && validNotificationAt >= budget.lastValidNotificationAt,
    "acp_prompt_deadline_invalid",
  );
  const boundedNotificationAt = Math.min(validNotificationAt, budget.hardDeadlineAt);
  return Object.freeze({
    hardDeadlineAt: budget.hardDeadlineAt,
    lastValidNotificationAt: boundedNotificationAt,
    quietDeadlineAt: Math.min(
      boundedNotificationAt + promptQuietMs,
      budget.hardDeadlineAt,
    ),
  });
}

export function promptQuiescenceExpiration(budget, now) {
  requireFact(budget && Number.isFinite(now), "acp_prompt_deadline_invalid");
  if (now >= budget.hardDeadlineAt) return "hard";
  if (now >= budget.quietDeadlineAt) return "quiet";
  return "pending";
}

export class AcpClient {
  constructor(executable, args, options) {
    this.timeoutMs = options.timeoutMs;
    this.maxOutputBytes = options.maxOutputBytes;
    this.outputBytes = 0;
    this.stderrBytes = 0;
    this.nextId = 1;
    this.messageSequence = 0;
    this.lastResponseSequence = 0;
    this.pending = new Map();
    this.notifications = [];
    this.notificationSequences = [];
    this.notificationObservers = new Set();
    this.promptNotificationValidation = null;
    this.failure = null;
    this.closed = false;
    this.permissionRequests = 0;
    this.unsupportedRequests = 0;
    this.child = spawn(executable, args, {
      cwd: options.cwd,
      env: options.environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.once("error", () => this.abort("acp_start_failed"));
    this.child.once("close", () => {
      if (!this.closed && !this.failure) this.abort("acp_exited_early");
    });
    this.child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > this.maxOutputBytes) this.abort("acp_stderr_limit");
    });
    const lines = createInterface({ input: this.child.stdout });
    lines.on("line", (line) => {
      this.outputBytes += Buffer.byteLength(line) + 1;
      if (this.outputBytes > this.maxOutputBytes) {
        this.abort("acp_stdout_limit");
        return;
      }
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        this.abort("acp_invalid_json");
        return;
      }
      this.messageSequence += 1;
      this.handleMessage(message, this.messageSequence);
    });
  }

  handleMessage(message, sequence) {
    if (message && Object.hasOwn(message, "id") && typeof message.method === "string") {
      this.handleServerRequest(message);
      return;
    }
    if (message && Object.hasOwn(message, "id")) {
      const pending = this.pending.get(String(message.id));
      if (!pending) return;
      this.pending.delete(String(message.id));
      clearTimeout(pending.timer);
      this.lastResponseSequence = sequence;
      if (message.error) {
        pending.reject(new AcceptanceError("acp_request_rejected"));
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    if (message && typeof message.method === "string") {
      this.notifications.push(message);
      this.notificationSequences.push(sequence);
      this.validatePromptNotifications();
      this.notifyNotificationObservers();
    }
  }

  validatePromptNotifications() {
    const validation = this.promptNotificationValidation;
    if (!validation || this.failure) return;
    while (validation.nextIndex < this.notifications.length) {
      const notification = this.notifications[validation.nextIndex];
      validation.nextIndex += 1;
      const notificationError = promptNotificationError(
        notification,
        validation.expectedSessionId,
      );
      if (notificationError === null || notificationError === "") continue;
      this.abort(notificationError);
      return;
    }
  }

  beginPromptNotificationValidation(startIndex, expectedSessionId) {
    requireFact(
      Number.isInteger(startIndex) && startIndex >= 0 && startIndex <= this.notifications.length,
      "acp_notification_cursor_invalid",
    );
    requireFact(
      isNormalizedBoundedText(expectedSessionId, maxSessionIdBytes),
      "acp_session_id_invalid",
    );
    requireFact(!this.promptNotificationValidation, "acp_notification_validation_active");
    this.promptNotificationValidation = {
      startIndex,
      nextIndex: startIndex,
      expectedSessionId,
    };
    this.validatePromptNotifications();
  }

  notifyNotificationObservers() {
    for (const observer of [...this.notificationObservers]) observer();
  }

  handleServerRequest(message) {
    const method = message.method;
    const responseId = message.id;
    if (method === "session/request_permission") {
      this.permissionRequests += 1;
      this.write({
        jsonrpc: "2.0",
        id: responseId,
        result: { outcome: { outcome: "cancelled" } },
      });
      const sessionId = message?.params?.sessionId;
      if (typeof sessionId === "string" && sessionId.length > 0) {
        this.write({
          jsonrpc: "2.0",
          method: "session/cancel",
          params: { sessionId },
        });
      }
      this.abort("acp_permission_required");
      return;
    }
    this.unsupportedRequests += 1;
    this.write({
      jsonrpc: "2.0",
      id: responseId,
      error: { code: -32601, message: "Client capability is unavailable." },
    });
    this.abort("acp_client_request_unsupported");
  }

  write(message) {
    if (this.failure || !this.child.stdin.writable) {
      throw new AcceptanceError(this.failure || "acp_stdin_closed");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  request(method, params) {
    const id = this.nextId++;
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        this.pending.delete(String(id));
        rejectRequest(new AcceptanceError("acp_request_timeout"));
        this.abort("acp_request_timeout");
      }, this.timeoutMs);
      this.pending.set(String(id), { resolve: resolveRequest, reject: rejectRequest, timer });
      try {
        this.write({ jsonrpc: "2.0", id, method, params });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(String(id));
        rejectRequest(error);
      }
    });
  }

  waitForPromptNotificationQuiescence(startIndex, expectedSessionId, hardDeadlineAt) {
    requireFact(
      Number.isInteger(startIndex) && startIndex >= 0 && startIndex <= this.notifications.length,
      "acp_notification_cursor_invalid",
    );
    requireFact(Number.isFinite(hardDeadlineAt), "acp_prompt_deadline_invalid");
    const validation = this.promptNotificationValidation;
    requireFact(
      validation?.startIndex === startIndex
        && validation.expectedSessionId === expectedSessionId,
      "acp_notification_validation_inactive",
    );
    if (this.failure) return Promise.reject(new AcceptanceError(this.failure));

    return new Promise((resolveWait, rejectWait) => {
      let budget = null;
      let quietTimer = null;
      let hardTimer = null;
      let observedCount = startIndex;
      let settled = false;
      const cleanup = () => {
        if (quietTimer) clearTimeout(quietTimer);
        if (hardTimer) clearTimeout(hardTimer);
        this.notificationObservers.delete(onNotification);
        if (this.promptNotificationValidation === validation) {
          this.promptNotificationValidation = null;
        }
      };
      const reject = (code) => {
        if (settled) return;
        settled = true;
        cleanup();
        rejectWait(new AcceptanceError(code));
      };
      const resolve = () => {
        if (settled) return;
        settled = true;
        cleanup();
        resolveWait(this.notifications.slice(startIndex));
      };
      const expire = () => {
        const expiration = promptQuiescenceExpiration(budget, performance.now());
        if (expiration === "hard") {
          this.abort("acp_request_timeout");
          reject("acp_request_timeout");
          return;
        }
        if (expiration === "quiet") {
          resolve();
          return;
        }
        armQuietDeadline();
      };
      const armQuietDeadline = () => {
        if (quietTimer) clearTimeout(quietTimer);
        const now = performance.now();
        const expiration = promptQuiescenceExpiration(budget, now);
        if (expiration !== "pending") return expire();
        quietTimer = setTimeout(expire, Math.max(0, budget.quietDeadlineAt - now));
      };
      const onNotification = () => {
        this.validatePromptNotifications();
        if (this.failure) {
          reject(this.failure);
          return;
        }
        let validNotificationSeen = false;
        while (observedCount < this.notifications.length) {
          const notification = this.notifications[observedCount];
          observedCount += 1;
          if (notification?.method !== "session/update") continue;
          validNotificationSeen = true;
        }
        if (validNotificationSeen) {
          budget = resetPromptQuiescenceBudget(budget, performance.now());
          armQuietDeadline();
        }
      };

      const responseAt = performance.now();
      const remainingMs = hardDeadlineAt - responseAt;
      if (remainingMs <= 0) {
        this.abort("acp_request_timeout");
        reject("acp_request_timeout");
        return;
      }
      budget = createPromptQuiescenceBudget(responseAt, hardDeadlineAt);
      this.notificationObservers.add(onNotification);
      hardTimer = setTimeout(() => {
        this.abort("acp_request_timeout");
        reject("acp_request_timeout");
      }, remainingMs);
      armQuietDeadline();
      onNotification();
    });
  }

  abort(code) {
    if (this.failure) return;
    this.failure = safeErrorCode(code);
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new AcceptanceError(this.failure));
    }
    this.pending.clear();
    this.notifyNotificationObservers();
    this.child.kill();
  }

  async initialize() {
    const result = await this.request("initialize", {
      protocolVersion: 1,
      clientCapabilities: {
        fs: { readTextFile: false, writeTextFile: false },
        terminal: false,
        auth: { terminal: false },
      },
      clientInfo: { name: "lico-up-parity", title: "LicoUp Parity", version: "1" },
    });
    requireFact(result?.protocolVersion === 1, "acp_protocol_version_mismatch");
    requireFact(result?.agentCapabilities?.loadSession === true, "acp_load_session_unavailable");
    return result;
  }

  async close() {
    if (this.closed) return;
    this.closed = true;
    this.child.stdin.end();
    if (this.child.exitCode === null) this.child.kill();
    await Promise.race([
      new Promise((resolveClose) => this.child.once("close", resolveClose)),
      new Promise((resolveClose) => setTimeout(resolveClose, 1_000)),
    ]);
  }
}
