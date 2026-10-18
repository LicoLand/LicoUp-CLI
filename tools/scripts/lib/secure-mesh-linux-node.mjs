import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import path from "node:path";

const RPC_PROTOCOL = "licoup.stdio.v1";
const MAX_FRAME_BYTES = 16 * 1024 * 1024;
const MAX_STDERR_BYTES = 64 * 1024;
const REQUEST_TIMEOUT_MS = 120_000;
const PROCESS_STOP_TIMEOUT_MS = 5_000;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function dockerInvocation(value = process.env.LICO_DOCKER_COMMAND_JSON || "") {
  if (!value) return { command: "docker", prefix: [] };
  let parsed;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error("Linux node Docker command is invalid");
  }
  assert(Array.isArray(parsed) && parsed.length >= 1 &&
    parsed.every((entry) => typeof entry === "string" && entry.trim()),
  "Linux node Docker command is invalid");
  return { command: parsed[0], prefix: parsed.slice(1) };
}

export class LinuxClientNode {
  constructor({ label, image, network, cli = "./licoup", dockerCommand = "" }) {
    assert(["linux-a", "linux-b", "linux-c"].includes(label),
      "Linux node label is not an allowed stable participant label");
    assert(typeof image === "string" && image.trim(), "Linux node image is required");
    assert(typeof network === "string" && network.trim(), "Linux node network is required");
    this.label = label;
    this.image = image;
    this.network = network;
    this.cli = cli;
    this.stateRoot = `/state/${label}`;
    this.docker = dockerInvocation(dockerCommand);
    this.containerId = "";
    this.rpc = null;
    this.rpcProcessCount = 0;
    this.mountIsolationVerified = false;
    this.rpcStopped = false;
    this.removed = false;
  }

  async start() {
    assert(!this.containerId, "Linux node is already started");
    const run = this.runDocker([
      "run",
      "--detach",
      "--rm",
      "--read-only",
      "--network",
      this.network,
      "--add-host",
      "host.docker.internal:host-gateway",
      "--tmpfs",
      "/state:rw,noexec,nosuid,nodev,size=64m,mode=1777",
      this.image,
      "sleep",
      "infinity"
    ]);
    this.containerId = String(run.stdout || "").trim();
    assert(/^[a-f0-9]{12,64}$/u.test(this.containerId), "Linux node container did not start");
    this.verifyMountIsolation();
    await this.startRpc();
    const status = await this.execute(["secure-mesh", "status"]);
    assert(status?.ok === true, "Linux node public readiness operation failed");
    return status;
  }

  async startRpc() {
    assert(this.containerId, "Linux node container is unavailable");
    assert(!this.rpc, "Linux node RPC is already started");
    this.rpc = new ContainerCliRpc({
      docker: this.docker,
      containerId: this.containerId,
      cli: this.cli,
      stateRoot: this.stateRoot
    });
    await this.rpc.start();
    this.rpcProcessCount += 1;
  }

  async execute(args) {
    assert(this.rpc, "Linux node RPC is unavailable");
    return this.rpc.execute(args);
  }

  async restartRpc() {
    assert(this.rpc, "Linux node RPC is unavailable");
    await this.rpc.shutdown();
    this.rpc = null;
    await this.startRpc();
  }

  async stop() {
    if (this.rpc) {
      try {
        await this.rpc.shutdown();
      } catch {
        assert(await this.rpc.forceCloseAndWait(), "Linux node RPC force-close exceeded its bound");
      }
      this.rpc = null;
    }
    this.rpcStopped = true;
    if (!this.containerId) {
      this.removed = true;
      return;
    }
    const containerId = this.containerId;
    this.containerId = "";
    const stopped = this.runDocker(["stop", "--time", "5", containerId], {
      allowFailure: true,
      timeout: 10_000
    });
    if (stopped.status !== 0) {
      this.runDocker(["rm", "--force", containerId], {
        allowFailure: true,
        timeout: 10_000
      });
    }
    const inspect = this.runDocker(["inspect", containerId], {
      allowFailure: true,
      timeout: 5_000
    });
    this.removed = inspect.status !== 0;
    assert(this.removed, "Linux node container remained after teardown");
  }

  verifyMountIsolation() {
    const inspect = this.runDocker([
      "inspect",
      "--format",
      "{{json .Mounts}}|{{json .HostConfig.Tmpfs}}",
      this.containerId
    ]);
    const [mountsText, tmpfsText] = String(inspect.stdout || "").trim().split("|", 2);
    const mounts = JSON.parse(mountsText || "[]");
    const tmpfs = JSON.parse(tmpfsText || "{}");
    assert(Array.isArray(mounts) && mounts.every((mount) =>
      mount?.Type === "tmpfs" && mount?.Destination === "/state"),
    "Linux node container has a host or named-volume mount");
    assert(tmpfs && typeof tmpfs === "object" && Object.keys(tmpfs).length === 1 &&
      Object.hasOwn(tmpfs, "/state"), "Linux node container state is not isolated tmpfs");
    this.mountIsolationVerified = true;
  }

  runDocker(args, options = {}) {
    const result = spawnSync(this.docker.command, [...this.docker.prefix, ...args], {
      encoding: "utf8",
      maxBuffer: 8 * 1024 * 1024,
      timeout: options.timeout || 30_000
    });
    if (!options.allowFailure) {
      assert(result.status === 0, "Linux node Docker operation failed");
    }
    return result;
  }
}

class ContainerCliRpc {
  constructor({ docker, containerId, cli, stateRoot }) {
    this.docker = docker;
    this.containerId = containerId;
    this.cli = cli;
    this.stateRoot = stateRoot;
    this.workflowId = `linux-node-${crypto.randomUUID()}`;
    this.child = null;
    this.nextRequestId = 1;
    this.pending = new Map();
    this.stdoutBuffer = Buffer.alloc(0);
    this.stderrBytes = 0;
    this.queue = Promise.resolve();
    this.exitPromise = null;
  }

  async start() {
    const child = spawn(this.docker.command, [
      ...this.docker.prefix,
      "exec",
      "-i",
      this.containerId,
      "env",
      `LICOUP_HOME=${this.stateRoot}`,
      this.cli,
      "rpc",
      "stdio"
    ], { stdio: ["pipe", "pipe", "pipe"] });
    this.child = child;
    child.stdout.on("data", (chunk) => this.onStdout(chunk));
    child.stderr.on("data", (chunk) => {
      this.stderrBytes = Math.min(MAX_STDERR_BYTES, this.stderrBytes + Buffer.byteLength(chunk));
    });
    this.exitPromise = new Promise((resolve) => {
      child.once("exit", () => {
        this.failPending("Linux node RPC exited");
        resolve();
      });
    });
    await new Promise((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", () => reject(new Error("Linux node RPC could not start")));
    });
  }

  execute(args) {
    assert(Array.isArray(args) && args.length > 0, "Linux node public operation is required");
    return this.enqueue(() => this.request({
      method: "execute",
      args,
      portableDataDir: this.stateRoot
    }));
  }

  shutdown() {
    return this.enqueue(async () => {
      if (!this.child || this.child.exitCode !== null) return;
      await this.request({ method: "shutdown" });
      const exited = await Promise.race([
        this.exitPromise.then(() => true),
        new Promise((resolve) => setTimeout(() => resolve(false), PROCESS_STOP_TIMEOUT_MS))
      ]);
      if (!exited) {
        this.forceClose();
        throw new Error("Linux node RPC bounded shutdown failed");
      }
      assert(this.stderrBytes <= MAX_STDERR_BYTES, "Linux node RPC stderr exceeded its bound");
    });
  }

  forceClose() {
    if (this.child && this.child.exitCode === null) this.child.kill("SIGKILL");
  }

  async forceCloseAndWait(timeoutMs = 1_000) {
    this.forceClose();
    if (!this.child || this.child.exitCode !== null) return true;
    return Promise.race([
      this.exitPromise.then(() => true),
      new Promise((resolve) => setTimeout(() => resolve(false), timeoutMs))
    ]);
  }

  enqueue(operation) {
    const current = this.queue.then(operation, operation);
    this.queue = current.catch(() => undefined);
    return current;
  }

  request(payload) {
    const child = this.child;
    if (!child || child.exitCode !== null || child.stdin.destroyed) {
      return Promise.reject(new Error("Linux node RPC is unavailable"));
    }
    const id = `request-${this.nextRequestId++}`;
    const frame = Buffer.from(`${JSON.stringify({
      protocol: RPC_PROTOCOL,
      id,
      workflowId: this.workflowId,
      ...payload
    })}\n`, "utf8");
    assert(frame.length <= MAX_FRAME_BYTES, "Linux node RPC request exceeded its bound");
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return;
        this.forceClose();
        reject(new Error("Linux node RPC request timed out"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
      child.stdin.write(frame, (error) => {
        if (!error) return;
        const pending = this.pending.get(id);
        if (!pending) return;
        this.pending.delete(id);
        clearTimeout(pending.timer);
        pending.reject(new Error("Linux node RPC request write failed"));
      });
    });
  }

  onStdout(chunk) {
    this.stdoutBuffer = Buffer.concat([this.stdoutBuffer, Buffer.from(chunk)]);
    while (true) {
      const newline = this.stdoutBuffer.indexOf(0x0a);
      if (newline < 0) {
        if (this.stdoutBuffer.length > MAX_FRAME_BYTES) this.failProtocol();
        return;
      }
      const line = this.stdoutBuffer.subarray(0, newline);
      this.stdoutBuffer = this.stdoutBuffer.subarray(newline + 1);
      if (line.length > MAX_FRAME_BYTES) {
        this.failProtocol();
        return;
      }
      this.onResponse(line);
    }
  }

  onResponse(line) {
    let response;
    try {
      response = JSON.parse(line.toString("utf8"));
    } catch {
      this.failProtocol();
      return;
    }
    const pending = this.pending.get(String(response?.id || ""));
    if (!pending || response?.protocol !== RPC_PROTOCOL || response?.workflowId !== this.workflowId) {
      this.failProtocol();
      return;
    }
    this.pending.delete(response.id);
    clearTimeout(pending.timer);
    if (response.ok === true) {
      pending.resolve(response.result);
      return;
    }
    const code = typeof response?.error?.code === "string" &&
      /^[a-z0-9_]{1,64}$/u.test(response.error.code)
      ? response.error.code
      : "command_failed";
    const error = new Error(`Linux node public operation failed: ${code}`);
    error.code = code;
    pending.reject(error);
  }

  failProtocol() {
    this.failPending("Linux node RPC protocol failed");
    this.forceClose();
  }

  failPending(message) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error(message));
    }
    this.pending.clear();
  }
}

export function createDockerNetwork({ dockerCommand = "" } = {}) {
  const docker = dockerInvocation(dockerCommand);
  const name = `lico-linux-node-${crypto.randomUUID()}`;
  const result = spawnSync(docker.command, [...docker.prefix, "network", "create", name], {
    encoding: "utf8",
    timeout: 15_000
  });
  assert(result.status === 0, "Linux node network creation failed");
  return Object.freeze({ name, docker });
}

export function removeDockerNetwork(network) {
  if (!network?.name || !network?.docker) return false;
  const result = spawnSync(network.docker.command, [
    ...network.docker.prefix,
    "network",
    "rm",
    network.name
  ], { encoding: "utf8", timeout: 15_000 });
  return result.status === 0;
}

export function buildLinuxNodeImage({ context, dockerfile, dockerCommand = "" }) {
  const docker = dockerInvocation(dockerCommand);
  const image = `lico-linux-current-client:${crypto.randomUUID()}`;
  const result = spawnSync(docker.command, [
    ...docker.prefix,
    "build",
    "--quiet",
    "--file",
    path.resolve(dockerfile),
    "--tag",
    image,
    path.resolve(context)
  ], { encoding: "utf8", maxBuffer: 16 * 1024 * 1024, timeout: 15 * 60_000 });
  assert(result.status === 0, "Linux current-client node image build failed");
  return Object.freeze({ image, docker });
}

export function removeLinuxNodeImage(imageRecord) {
  if (!imageRecord?.image || !imageRecord?.docker) return false;
  const result = spawnSync(imageRecord.docker.command, [
    ...imageRecord.docker.prefix,
    "image",
    "rm",
    "--force",
    imageRecord.image
  ], { encoding: "utf8", timeout: 30_000 });
  return result.status === 0;
}
