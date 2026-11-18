import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { AcceptanceError, safeErrorCode } from "../errors.mjs";

export class PiRpcClient {
  constructor(executable, options) {
    this.timeoutMs = options.timeoutMs;
    this.maxOutputBytes = options.maxOutputBytes;
    this.outputBytes = 0;
    this.stderrBytes = 0;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.eventWaiters = [];
    this.buffer = Buffer.alloc(0);
    this.failure = null;
    this.closed = false;
    this.permissionRequests = 0;
    this.unsupportedRequests = 0;
    this.child = spawn(executable, ["--mode", "rpc", "--offline"], {
      cwd: options.cwd,
      env: options.environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.once("error", () => this.abort("pi_rpc_start_failed"));
    this.child.once("close", () => {
      if (!this.closed && !this.failure) this.abort("pi_rpc_exited_early");
    });
    this.child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > this.maxOutputBytes) this.abort("pi_rpc_stderr_limit");
    });
    this.child.stdout.on("data", (chunk) => this.handleChunk(chunk));
  }

  handleChunk(chunk) {
    this.outputBytes += chunk.length;
    if (this.outputBytes > this.maxOutputBytes) {
      this.abort("pi_rpc_stdout_limit");
      return;
    }
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (true) {
      const newline = this.buffer.indexOf(0x0a);
      if (newline < 0) break;
      let frame = this.buffer.subarray(0, newline);
      this.buffer = this.buffer.subarray(newline + 1);
      if (frame.at(-1) === 0x0d) frame = frame.subarray(0, frame.length - 1);
      if (frame.length === 0) continue;
      let message;
      try {
        message = JSON.parse(frame.toString("utf8"));
      } catch {
        this.abort("pi_rpc_invalid_json");
        return;
      }
      this.handleMessage(message);
    }
  }

  handleMessage(message) {
    if (message?.type === "extension_ui_request") {
      this.permissionRequests += 1;
      this.abort("pi_rpc_interaction_required");
      return;
    }
    if (message?.type === "response") {
      const pending = this.pending.get(String(message.id));
      if (!pending) return;
      this.pending.delete(String(message.id));
      clearTimeout(pending.timer);
      if (message.success !== true || message.command !== pending.command) {
        pending.reject(new AcceptanceError("pi_rpc_request_rejected"));
      } else {
        pending.resolve(message.data ?? {});
      }
      return;
    }
    const waiterIndex = this.eventWaiters.findIndex((waiter) => waiter.matches(message));
    if (waiterIndex >= 0) {
      const [waiter] = this.eventWaiters.splice(waiterIndex, 1);
      clearTimeout(waiter.timer);
      waiter.resolve(message);
    }
    this.events.push(message);
  }

  write(message) {
    if (this.failure || !this.child.stdin.writable) {
      throw new AcceptanceError(this.failure || "pi_rpc_stdin_closed");
    }
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  request(command, fields = {}) {
    const id = `lico-pi-${this.nextId++}`;
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        rejectRequest(new AcceptanceError("pi_rpc_request_timeout"));
        this.abort("pi_rpc_request_timeout");
      }, this.timeoutMs);
      this.pending.set(id, { command, resolve: resolveRequest, reject: rejectRequest, timer });
      try {
        this.write({ id, type: command, ...fields });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        rejectRequest(error);
      }
    });
  }

  waitForEvent(matches) {
    const queued = this.events.find(matches);
    if (queued) return Promise.resolve(queued);
    return new Promise((resolveEvent, rejectEvent) => {
      const waiter = { matches, resolve: resolveEvent, reject: rejectEvent, timer: null };
      waiter.timer = setTimeout(() => {
        const index = this.eventWaiters.indexOf(waiter);
        if (index >= 0) this.eventWaiters.splice(index, 1);
        rejectEvent(new AcceptanceError("pi_rpc_event_timeout"));
        this.abort("pi_rpc_event_timeout");
      }, this.timeoutMs);
      this.eventWaiters.push(waiter);
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
    for (const waiter of this.eventWaiters) {
      clearTimeout(waiter.timer);
      waiter.reject(new AcceptanceError(this.failure));
    }
    this.eventWaiters = [];
    if (this.child.exitCode === null) this.child.kill();
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
