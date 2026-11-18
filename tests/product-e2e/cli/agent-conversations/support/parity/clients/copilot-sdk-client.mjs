import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { AcceptanceError, requireFact, safeErrorCode } from "../errors.mjs";

export class CopilotSdkRpcClient {
  constructor(context, launchArgs) {
    this.context = context;
    this.nextId = 1;
    this.pending = new Map();
    this.buffer = Buffer.alloc(0);
    this.stdoutBytes = 0;
    this.stderrBytes = 0;
    this.failure = null;
    this.closed = false;
    this.child = spawn(context.binary, launchArgs, {
      cwd: context.cwd,
      env: context.environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child.once("error", () => this.abort("copilot_sdk_start_failed"));
    this.child.once("close", () => {
      if (!this.closed && !this.failure) this.abort("copilot_sdk_exited_early");
    });
    this.child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > context.maxOutputBytes) this.abort("copilot_sdk_stderr_limit");
    });
    this.child.stdout.on("data", (chunk) => this.handleChunk(chunk));
  }

  handleChunk(chunk) {
    this.stdoutBytes += chunk.length;
    if (this.stdoutBytes > this.context.maxOutputBytes) {
      this.abort("copilot_sdk_stdout_limit");
      return;
    }
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (true) {
      const headerEnd = this.buffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) return;
      const header = this.buffer.subarray(0, headerEnd).toString("ascii");
      const match = header.match(/(?:^|\r\n)Content-Length:\s*(\d+)(?:\r\n|$)/iu);
      if (!match) {
        this.abort("copilot_sdk_invalid_frame");
        return;
      }
      const bodyLength = Number(match[1]);
      if (!Number.isSafeInteger(bodyLength) || bodyLength < 0 || bodyLength > this.context.maxOutputBytes) {
        this.abort("copilot_sdk_frame_limit");
        return;
      }
      const bodyStart = headerEnd + 4;
      if (this.buffer.length < bodyStart + bodyLength) return;
      const body = this.buffer.subarray(bodyStart, bodyStart + bodyLength);
      this.buffer = this.buffer.subarray(bodyStart + bodyLength);
      let message;
      try {
        message = JSON.parse(body.toString("utf8"));
      } catch {
        this.abort("copilot_sdk_invalid_json");
        return;
      }
      this.handleMessage(message);
    }
  }

  handleMessage(message) {
    if (message && Object.hasOwn(message, "id") && typeof message.method === "string") {
      this.write({
        jsonrpc: "2.0",
        id: message.id,
        error: { code: -32601, message: "Client capability is unavailable." },
      });
      this.abort("copilot_sdk_client_request_unsupported");
      return;
    }
    if (!message || !Object.hasOwn(message, "id")) return;
    const pending = this.pending.get(String(message.id));
    if (!pending) return;
    this.pending.delete(String(message.id));
    clearTimeout(pending.timer);
    if (message.error) {
      pending.reject(new AcceptanceError("copilot_sdk_request_rejected", {
        rpcCode: Number(message.error.code),
      }));
    } else {
      pending.resolve(message.result);
    }
  }

  write(message) {
    if (this.failure || !this.child.stdin.writable) {
      throw new AcceptanceError(this.failure || "copilot_sdk_stdin_closed");
    }
    const body = Buffer.from(JSON.stringify(message), "utf8");
    const header = Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii");
    this.child.stdin.write(Buffer.concat([header, body]));
  }

  request(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        this.pending.delete(String(id));
        rejectRequest(new AcceptanceError("copilot_sdk_request_timeout"));
        this.abort("copilot_sdk_request_timeout");
      }, Math.min(this.context.timeoutMs, 30_000));
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

  async connect() {
    let result;
    try {
      result = await this.request("connect", { enableGitHubTelemetryForwarding: false });
    } catch (error) {
      if (!(error instanceof AcceptanceError) || error.details?.rpcCode !== -32601) throw error;
      result = await this.request("ping", {});
    }
    requireFact(Number.isInteger(result?.protocolVersion), "copilot_sdk_protocol_missing");
  }

  abort(code) {
    if (this.failure) return;
    this.failure = safeErrorCode(code);
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new AcceptanceError(this.failure));
    }
    this.pending.clear();
    this.child.kill();
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
