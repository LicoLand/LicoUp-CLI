import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { AcceptanceError, requireFact } from "../errors.mjs";

const protocol = "licoup.stdio.v1";
const maxIdentifierBytes = 128;
const maxOpaqueIdentifierBytes = 512;

function validIdentifier(value) {
  return typeof value === "string"
    && value.length > 0
    && value.length <= maxIdentifierBytes
    && /^[a-z0-9._-]+$/iu.test(value);
}

function validOpaqueIdentifier(value) {
  return typeof value === "string"
    && value.trim().length > 0
    && Buffer.byteLength(value) <= maxOpaqueIdentifierBytes
    && !/[\u0000-\u001f\u007f]/u.test(value);
}

function exactSessionId(result) {
  const nativeSessionId = result?.nativeSessionId;
  const sessionId = result?.sessionId;
  return validOpaqueIdentifier(nativeSessionId)
    && validOpaqueIdentifier(sessionId)
    && nativeSessionId === sessionId
    ? nativeSessionId
    : "";
}

function boundedFrame(value, maxBytes) {
  const encoded = `${JSON.stringify(value)}\n`;
  requireFact(Buffer.byteLength(encoded) <= maxBytes, "stdio_rpc_request_limit");
  return encoded;
}

function responseErrorCode(frame) {
  const code = frame?.error?.code;
  return typeof code === "string" ? code : "stdio_rpc_command_failed";
}

export class StdioRpcClient {
  constructor(executable, context, options = {}) {
    this.executable = executable;
    this.cwd = context.cwd;
    this.environment = context.environment;
    this.timeoutMs = context.timeoutMs;
    this.maxOutputBytes = context.maxOutputBytes;
    this.args = Object.freeze(options.args || ["rpc", "stdio"]);
    // The conversation proxy relays to the persistent host; it is the only
    // admitted alternative to the in-process stdio lane.
    this.conversationProxy = options.conversationProxy === true;
    this.workflowId = `acceptance-${randomUUID()}`;
    this.nextRequest = 1;
    this.child = null;
    this.pending = null;
    this.controlPending = null;
    this.stdoutBuffer = Buffer.alloc(0);
    this.stderrBytes = 0;
    this.closed = false;
    this.closeStatus = null;
    this.closeWaiters = [];
    this.completedTurnIds = new Set();
  }

  async connect() {
    requireFact(this.child === null && !this.closed, "stdio_rpc_client_reused");
    const allowed = this.args[0] === "rpc"
      && (this.args[1] === "stdio"
        || (this.conversationProxy === true && this.args[1] === "conversation"));
    requireFact(
      this.args.length === 2 && allowed,
      "stdio_rpc_launch_args_invalid",
    );
    const child = spawn(this.executable, this.args, {
      cwd: this.cwd,
      env: this.environment,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child = child;
    child.once("error", () => this.#fail(new AcceptanceError("stdio_rpc_start_failed")));
    child.stdout.on("data", (chunk) => this.#onStdout(chunk));
    child.stderr.on("data", (chunk) => {
      this.stderrBytes += chunk.length;
      if (this.stderrBytes > this.maxOutputBytes) {
        this.#fail(new AcceptanceError("stdio_rpc_stderr_limit"));
      }
    });
    child.stdin.on("error", () => {
      this.#fail(new AcceptanceError("stdio_rpc_write_failed"));
    });
    child.once("close", (statusCode) => {
      this.closeStatus = statusCode;
      this.closed = true;
      this.#failPending(new AcceptanceError("stdio_rpc_host_closed"));
      for (const resolveClose of this.closeWaiters.splice(0)) resolveClose(statusCode);
    });
    await new Promise((resolveConnect, rejectConnect) => {
      const timer = setTimeout(
        () => rejectConnect(new AcceptanceError("stdio_rpc_start_timeout")),
        Math.min(this.timeoutMs, 5_000),
      );
      child.once("spawn", () => {
        clearTimeout(timer);
        resolveConnect();
      });
      child.once("error", () => {
        clearTimeout(timer);
        rejectConnect(new AcceptanceError("stdio_rpc_start_failed"));
      });
    });
  }

  async request(method, params = {}) {
    const exchange = await this.#begin(method, params, false);
    return exchange.promise;
  }

  async streamConversation(params) {
    const exchange = await this.#begin("agent.conversation.send", params, true);
    const terminal = await exchange.promise;
    return {
      result: terminal.result,
      events: terminal.events,
      boundedOutput: terminal.boundedOutput,
      streamingSeen: terminal.streamingSeen,
      structuredSeen: terminal.structuredSeen,
      terminalOrdered: terminal.terminalOrdered,
      eventTranscriptMatches: terminal.eventTranscriptMatches,
    };
  }

  /**
   * Issue an in-flight control request (e.g. steer) while a streaming send is
   * pending. The host runs conversation sends on worker threads, so steer can
   * land against the live process-local session before the turn completes.
   */
  async controlWhileStreaming(method, params = {}) {
    requireFact(this.pending?.streaming === true, "stdio_rpc_stream_inactive");
    requireFact(this.controlPending === null, "stdio_rpc_concurrent_control_unsupported");
    requireFact(validIdentifier(method), "stdio_rpc_method_invalid");
    requireFact(params && typeof params === "object" && !Array.isArray(params), "stdio_rpc_params_invalid");
    const requestId = `request-${this.nextRequest++}`;
    const frame = boundedFrame({
      protocol,
      id: requestId,
      workflowId: this.workflowId,
      method,
      params,
    }, this.maxOutputBytes);
    let resolvePending;
    let rejectPending;
    const promise = new Promise((resolve, reject) => {
      resolvePending = resolve;
      rejectPending = reject;
    });
    const timer = setTimeout(
      () => this.#fail(new AcceptanceError("stdio_rpc_control_timeout")),
      this.timeoutMs,
    );
    this.controlPending = {
      requestId,
      timer,
      resolve: resolvePending,
      reject: rejectPending,
    };
    this.child.stdin.write(frame, (error) => {
      if (error) this.#fail(new AcceptanceError("stdio_rpc_write_failed"));
    });
    return promise;
  }

  waitForStreamEvent(predicate, timeoutMs = this.timeoutMs) {
    requireFact(this.pending?.streaming === true, "stdio_rpc_stream_inactive");
    requireFact(typeof predicate === "function", "stdio_rpc_wait_predicate_invalid");
    const deadline = Date.now() + Math.max(1_000, Number(timeoutMs) || this.timeoutMs);
    return new Promise((resolve, reject) => {
      const check = () => {
        if (this.closed) {
          reject(new AcceptanceError("stdio_rpc_host_unavailable"));
          return;
        }
        const pending = this.pending;
        if (!pending?.streaming) {
          reject(new AcceptanceError("stdio_rpc_stream_inactive"));
          return;
        }
        const match = pending.events.find((event) => {
          try {
            return predicate(event) === true;
          } catch {
            return false;
          }
        });
        if (match) {
          resolve(match);
          return;
        }
        if (Date.now() >= deadline) {
          reject(new AcceptanceError("stdio_rpc_stream_event_timeout"));
          return;
        }
        setTimeout(check, 25);
      };
      check();
    });
  }

  async shutdown() {
    if (!this.child || this.closed) {
      return { acknowledged: false, exited: this.closed, statusCode: this.closeStatus };
    }
    const result = await this.request("shutdown");
    requireFact(result?.status === "shutdown", "stdio_rpc_shutdown_invalid");
    const statusCode = await this.#waitForClose();
    return { acknowledged: true, exited: true, statusCode };
  }

  async closeInputAndWait() {
    requireFact(this.child !== null && !this.closed, "stdio_rpc_host_unavailable");
    this.child.stdin.end();
    const statusCode = await this.#waitForClose();
    return { exited: true, statusCode };
  }

  async abort() {
    if (!this.child || this.closed) return;
    this.child.kill();
    await this.#waitForClose().catch(() => {});
  }

  async #begin(method, params, streaming) {
    requireFact(this.child !== null && !this.closed, "stdio_rpc_host_unavailable");
    requireFact(this.pending === null, "stdio_rpc_concurrent_request_unsupported");
    requireFact(validIdentifier(method), "stdio_rpc_method_invalid");
    requireFact(params && typeof params === "object" && !Array.isArray(params), "stdio_rpc_params_invalid");
    const requestId = `request-${this.nextRequest++}`;
    const frame = boundedFrame({
      protocol,
      id: requestId,
      workflowId: this.workflowId,
      method,
      ...(method === "shutdown" ? {} : { params }),
    }, this.maxOutputBytes);
    let resolvePending;
    let rejectPending;
    const promise = new Promise((resolve, reject) => {
      resolvePending = resolve;
      rejectPending = reject;
    });
    const timer = setTimeout(
      () => this.#fail(new AcceptanceError("stdio_rpc_timeout")),
      this.timeoutMs,
    );
    this.pending = {
      requestId,
      streaming,
      nextSequence: 1,
      events: [],
      eventSessionId: "",
      eventTurnId: "",
      chunks: [],
      completedOutput: null,
      startedSeen: false,
      completedSeen: false,
      dispatchCompletedSeen: false,
      terminalFrame: null,
      terminalReceived: false,
      observedBytes: 0,
      timer,
      resolve: resolvePending,
      reject: rejectPending,
    };
    this.child.stdin.write(frame, (error) => {
      if (error) this.#fail(new AcceptanceError("stdio_rpc_write_failed"));
    });
    return { promise };
  }

  #onStdout(chunk) {
    if (this.closed) return;
    this.stdoutBuffer = Buffer.concat([this.stdoutBuffer, chunk]);
    if (this.stdoutBuffer.length > this.maxOutputBytes) {
      this.#fail(new AcceptanceError("stdio_rpc_output_limit"));
      return;
    }
    while (true) {
      const lineEnd = this.stdoutBuffer.indexOf(0x0a);
      if (lineEnd < 0) return;
      const line = this.stdoutBuffer.subarray(0, lineEnd);
      this.stdoutBuffer = this.stdoutBuffer.subarray(lineEnd + 1);
      if (line.length === 0) continue;
      let frame;
      try {
        frame = JSON.parse(line.toString("utf8"));
      } catch {
        this.#fail(new AcceptanceError("stdio_rpc_invalid_json"));
        return;
      }
      try {
        this.#acceptFrame(frame, line.length + 1);
      } catch (error) {
        this.#fail(error instanceof AcceptanceError
          ? error
          : new AcceptanceError("stdio_rpc_invalid_response"));
        return;
      }
    }
  }

  #acceptFrame(frame, bytes) {
    requireFact(frame?.protocol === protocol && frame?.workflowId === this.workflowId, "stdio_rpc_identity_mismatch");
    if (this.controlPending && frame?.id === this.controlPending.requestId) {
      this.#acceptControlFrame(frame);
      return;
    }
    const pending = this.pending;
    requireFact(Boolean(pending), "stdio_rpc_unsolicited_frame");
    requireFact(pending.terminalReceived !== true, "stdio_rpc_frame_after_terminal");
    requireFact(frame?.id === pending.requestId, "stdio_rpc_identity_mismatch");
    pending.observedBytes += bytes;
    requireFact(pending.observedBytes <= this.maxOutputBytes, "stdio_rpc_output_limit");
    if (!pending.streaming) {
      // Conversation ops other than streamed send complete as a single
      // kind=terminal frame; catalog/shutdown keep the legacy kind-less shape.
      if (Object.hasOwn(frame, "kind")) {
        requireFact(frame.kind === "terminal", "stdio_rpc_invalid_response");
        requireFact(Number.isSafeInteger(frame.sequence), "stdio_rpc_sequence_invalid");
      }
      this.#completePending(() => {
        if (frame.ok === true && frame.result && typeof frame.result === "object") {
          pending.resolve(frame.result);
        } else {
          pending.reject(new AcceptanceError(responseErrorCode(frame)));
        }
      });
      return;
    }
    requireFact(frame.kind === "event" || frame.kind === "terminal", "stdio_rpc_invalid_response");
    requireFact(frame.sequence === pending.nextSequence, "stdio_rpc_sequence_invalid");
    pending.nextSequence += 1;
    if (frame.kind === "event") {
      requireFact(frame.event && typeof frame.event === "object", "stdio_rpc_event_invalid");
      this.#acceptConversationEvent(pending, frame.event);
      pending.events.push(frame.event);
      return;
    }
    if (frame.ok !== true || !frame.result || typeof frame.result !== "object") {
      this.#completePending(() => pending.reject(new AcceptanceError(responseErrorCode(frame))));
      return;
    }
    pending.terminalReceived = true;
    pending.terminalFrame = frame;
    setImmediate(() => this.#finalizeStreaming(pending.requestId));
  }

  #acceptControlFrame(frame) {
    const pending = this.controlPending;
    requireFact(Boolean(pending), "stdio_rpc_unsolicited_frame");
    if (Object.hasOwn(frame, "kind")) {
      requireFact(frame.kind === "terminal", "stdio_rpc_invalid_response");
    }
    this.controlPending = null;
    clearTimeout(pending.timer);
    if (frame.ok === true && frame.result && typeof frame.result === "object") {
      pending.resolve(frame.result);
      return;
    }
    pending.reject(new AcceptanceError(responseErrorCode(frame)));
  }

  #acceptConversationEvent(pending, event) {
    requireFact(validOpaqueIdentifier(event.sessionId), "stdio_rpc_event_session_id_invalid");
    requireFact(validOpaqueIdentifier(event.turnId), "stdio_rpc_event_turn_id_invalid");
    if (pending.eventSessionId) {
      requireFact(event.sessionId === pending.eventSessionId, "stdio_rpc_event_identity_mismatch");
      requireFact(event.turnId === pending.eventTurnId, "stdio_rpc_event_identity_mismatch");
    } else {
      pending.eventSessionId = event.sessionId;
      pending.eventTurnId = event.turnId;
    }
    requireFact(pending.dispatchCompletedSeen !== true, "stdio_rpc_event_after_completed");
    requireFact(typeof event.event === "string" && event.event.length > 0, "stdio_rpc_event_invalid");
    const ignoredLifecycle = new Set([
      "dispatch.turn.bound",
      "agent.turn.processing",
      "agent.turn.responding",
    ]);
    if (ignoredLifecycle.has(event.event)) {
      return;
    }
    if (!pending.startedSeen) {
      requireFact(
        event.event === "dispatch.turn.started" || event.event === "agent.turn.accepted",
        "stdio_rpc_event_order_invalid",
      );
      pending.startedSeen = true;
      return;
    }
    requireFact(
      event.event !== "dispatch.turn.started" && event.event !== "agent.turn.accepted",
      "stdio_rpc_event_duplicate",
    );
    if (event.event === "agent.message.chunk") {
      requireFact(!pending.completedSeen, "stdio_rpc_event_order_invalid");
      const text = event?.payload?.text;
      requireFact(typeof text === "string" && text.length > 0, "stdio_rpc_chunk_invalid");
      requireFact(Buffer.byteLength(text) <= this.maxOutputBytes, "stdio_rpc_output_limit");
      pending.chunks.push(text);
      return;
    }
    if (event.event === "agent.message.completed") {
      requireFact(!pending.completedSeen, "stdio_rpc_event_duplicate");
      requireFact(pending.chunks.length > 0, "stdio_rpc_chunk_missing");
      const output = event?.payload?.text;
      requireFact(typeof output === "string", "stdio_rpc_completed_output_invalid");
      pending.completedSeen = true;
      pending.completedOutput = output;
      return;
    }
    if (event.event === "dispatch.turn.completed") {
      requireFact(pending.completedSeen, "stdio_rpc_event_order_invalid");
      pending.dispatchCompletedSeen = true;
    }
  }

  #finalizeStreaming(requestId) {
    const pending = this.pending;
    if (!pending || pending.requestId !== requestId || !pending.terminalReceived) return;
    try {
      const frame = pending.terminalFrame;
      const result = frame.result;
      if (result.ok !== true) {
        this.#completePending(() => {
          pending.reject(new AcceptanceError(responseErrorCode(result)));
        });
        return;
      }
      const sessionId = exactSessionId(result);
      requireFact(sessionId.length > 0, "stdio_rpc_terminal_session_id_invalid");
      requireFact(validOpaqueIdentifier(result.turnId), "stdio_rpc_terminal_turn_id_invalid");
      requireFact(result.threadId === sessionId, "stdio_rpc_terminal_identity_mismatch");
      requireFact(
        result.turnStatus === "completed" || result.turnStatus === "interrupted",
        "stdio_rpc_terminal_status_invalid",
      );
      requireFact(result.turnId === pending.eventTurnId, "stdio_rpc_terminal_identity_mismatch");
      requireFact(sessionId === pending.eventSessionId, "stdio_rpc_terminal_identity_mismatch");
      requireFact(!this.completedTurnIds.has(result.turnId), "stdio_rpc_turn_id_reused");
      requireFact(pending.startedSeen, "stdio_rpc_event_order_invalid");
      requireFact(pending.completedSeen, "stdio_rpc_completed_event_missing");
      requireFact(pending.dispatchCompletedSeen, "stdio_rpc_dispatch_completed_missing");
      requireFact(typeof result.output === "string", "stdio_rpc_terminal_output_invalid");
      const chunks = pending.chunks.join("");
      const completedOutput = pending.completedOutput;
      const chunkMatchesCompleted = chunks === completedOutput;
      const chunkMatchesTerminal = chunks === result.output;
      requireFact(
        chunkMatchesCompleted && chunkMatchesTerminal,
        "stdio_rpc_chunk_output_mismatch",
      );
      this.completedTurnIds.add(result.turnId);
      const events = Object.freeze([...pending.events]);
      this.#completePending(() => pending.resolve({
        result,
        events,
        boundedOutput: pending.observedBytes <= this.maxOutputBytes,
        streamingSeen: true,
        structuredSeen: pending.completedSeen,
        terminalOrdered: true,
        eventTranscriptMatches: chunkMatchesCompleted && chunkMatchesTerminal,
      }));
    } catch (error) {
      this.#fail(error instanceof AcceptanceError
        ? error
        : new AcceptanceError("stdio_rpc_invalid_response"));
    }
  }

  #completePending(complete) {
    const pending = this.pending;
    if (!pending) return;
    this.pending = null;
    clearTimeout(pending.timer);
    complete();
  }

  #fail(error) {
    this.#failPending(error);
    if (this.child && !this.closed) this.child.kill();
  }

  #failPending(error) {
    const pending = this.pending;
    if (pending) {
      this.pending = null;
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    const control = this.controlPending;
    if (control) {
      this.controlPending = null;
      clearTimeout(control.timer);
      control.reject(error);
    }
  }

  #waitForClose() {
    if (this.closed) return Promise.resolve(this.closeStatus);
    return new Promise((resolveClose, rejectClose) => {
      const timer = setTimeout(() => {
        if (this.child && !this.closed) this.child.kill();
        rejectClose(new AcceptanceError("stdio_rpc_exit_timeout"));
      }, Math.min(this.timeoutMs, 10_000));
      this.closeWaiters.push((statusCode) => {
        clearTimeout(timer);
        resolveClose(statusCode);
      });
    });
  }
}
