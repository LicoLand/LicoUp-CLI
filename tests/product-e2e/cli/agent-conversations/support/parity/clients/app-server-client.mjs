import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { createInterface } from "node:readline";
import { AcceptanceError, safeErrorCode } from "../errors.mjs";

export class AppServerClient {
  constructor(executable, args, options) {
    this.timeoutMs = options.timeoutMs;
    this.maxOutputBytes = options.maxOutputBytes;
    this.outputBytes = 0;
    this.stderrBytes = 0;
    this.nextId = 1;
    this.pending = new Map();
    this.notifications = [];
    this.notificationWaiters = [];
    this.failure = null;
    this.closed = false;
    this.permissionRequests = 0;
    this.unsupportedRequests = 0;
    this.child = spawn(executable, args, {
      cwd: options.cwd,
      env: options.environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.once("error", () => this.abort("app_server_start_failed"));
    this.child.once("close", () => {
      if (!this.closed && !this.failure) this.abort("app_server_exited_early");
    });
    this.child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > this.maxOutputBytes) this.abort("app_server_stderr_limit");
    });
    const lines = createInterface({ input: this.child.stdout });
    lines.on("line", (line) => {
      this.outputBytes += Buffer.byteLength(line) + 1;
      if (this.outputBytes > this.maxOutputBytes) {
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
    if (message && Object.hasOwn(message, "id") && typeof message.method === "string") {
      this.permissionRequests += 1;
      this.write({
        id: message.id,
        error: { code: -32001, message: "Parity acceptance does not approve interactions." },
      });
      this.abort("app_server_interaction_required");
      return;
    }
    if (message && Object.hasOwn(message, "id")) {
      const pending = this.pending.get(String(message.id));
      if (!pending) return;
      this.pending.delete(String(message.id));
      clearTimeout(pending.timer);
      if (message.error) {
        pending.reject(new AcceptanceError("app_server_request_failed"));
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    if (!message?.method) return;
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
      throw new AcceptanceError(this.failure || "app_server_stdin_closed");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  request(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        this.pending.delete(String(id));
        rejectRequest(new AcceptanceError("app_server_request_timeout"));
        this.abort("app_server_request_timeout");
      }, this.timeoutMs);
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
        if (index >= 0) this.notificationWaiters.splice(index, 1);
        rejectNotification(new AcceptanceError("app_server_notification_timeout"));
        this.abort("app_server_notification_timeout");
      }, this.timeoutMs);
      this.notificationWaiters.push(waiter);
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
    for (const waiter of this.notificationWaiters) {
      clearTimeout(waiter.timer);
      waiter.reject(new AcceptanceError(this.failure));
    }
    this.notificationWaiters = [];
    this.child.kill();
  }

  async initialize() {
    await this.request("initialize", {
      clientInfo: { name: "lico-up-parity", title: "LicoUp Parity", version: "1" },
      capabilities: { experimentalApi: true },
    });
    this.notify("initialized");
    return { protocolVersion: 1 };
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
