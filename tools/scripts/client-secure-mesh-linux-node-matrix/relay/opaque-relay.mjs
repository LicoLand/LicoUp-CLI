import { randomBytes, randomUUID } from "node:crypto";
import http from "node:http";
import { assert } from "../assert.mjs";
import {
  bearerToken,
  publicCommand,
  publicPairing,
  readJsonBody,
  relayResult,
  sendJson,
} from "./protocol.mjs";

export class OpaqueRelay {
  static async start() {
    const relay = new OpaqueRelay();
    await relay.listen();
    return relay;
  }

  constructor() {
    this.pairings = new Map();
    this.plaintextMarkers = new Set();
    this.plaintextObserved = false;
    this.server = http.createServer((request, response) => {
      this.handle(request, response).catch(() => {
        sendJson(response, 500, { ok: false, error: "relay_operation_failed" });
      });
    });
    this.port = 0;
  }

  async listen() {
    await new Promise((resolve, reject) => {
      this.server.once("error", reject);
      this.server.listen(0, "0.0.0.0", () => {
        this.server.off("error", reject);
        resolve();
      });
    });
    const address = this.server.address();
    assert(address && typeof address === "object", "Opaque relay did not bind");
    this.port = address.port;
  }

  containerGateway() {
    assert(this.port > 0, "Opaque relay is unavailable");
    return `http://host.docker.internal:${this.port}`;
  }

  async handle(request, response) {
    if (request.method !== "POST") {
      sendJson(response, 405, { ok: false, error: "method_not_allowed" });
      return;
    }
    const body = await readJsonBody(request);
    this.scanPlaintext(body);
    const pathname = new URL(request.url || "/", "http://relay.invalid").pathname;
    if (pathname === "/api/mobile-relay/pairings") {
      this.createPairing(body, response);
      return;
    }
    if (pathname === "/api/mobile-relay/pairings/claim") {
      this.claimPairing(body, response);
      return;
    }
    if (pathname === "/api/mobile-relay/pairings/status") {
      this.pairingStatus(body, request, response);
      return;
    }
    if (pathname === "/api/mobile-relay/pc/check-in") {
      this.pairingStatus(body, request, response);
      return;
    }
    if (pathname === "/api/mobile-relay/commands") {
      this.createCommand(body, request, response);
      return;
    }
    if (pathname === "/api/mobile-relay/commands/poll") {
      this.pollCommands(body, request, response);
      return;
    }
    const complete = pathname.match(/^\/api\/mobile-relay\/commands\/([^/]+)\/complete$/u);
    if (complete) {
      this.completeCommand(complete[1], body, request, response);
      return;
    }
    const result = pathname.match(/^\/api\/mobile-relay\/commands\/([^/]+)\/result$/u);
    if (result) {
      this.commandResult(result[1], body, request, response);
      return;
    }
    sendJson(response, 404, { ok: false, error: "operation_not_found" });
  }

  createPairing(body, response) {
    const pairingId = `pair_${randomUUID()}`;
    const pairingCode = String(Math.floor(100000 + Math.random() * 900000));
    const pcToken = randomBytes(32).toString("base64url");
    const now = new Date().toISOString();
    const pairing = {
      pairingId,
      pairingCode,
      pcToken,
      mobileToken: "",
      status: "pending",
      createdAt: now,
      updatedAt: now,
      expiresAt: new Date(Date.now() + 10 * 60_000).toISOString(),
      pc: {
        clientId: String(body.pcClientId || ""),
        label: String(body.pcClientName || ""),
        platform: String(body.platform || "linux"),
        capabilities: body.capabilities || {},
        targets: Array.isArray(body.targets) ? body.targets : [],
        secureMesh: body.secureMesh || body.pcSecureMesh || null
      },
      mobile: null,
      commands: []
    };
    this.pairings.set(pairingId, pairing);
    sendJson(response, 200, relayResult({
      pairing: publicPairing(pairing),
      pairingId,
      pairingCode,
      pcToken,
      expiresAt: pairing.expiresAt
    }));
  }

  claimPairing(body, response) {
    const pairing = this.pairings.get(String(body.pairingId || ""));
    if (!pairing || pairing.status !== "pending" || String(body.pairingCode || "") !== pairing.pairingCode) {
      sendJson(response, 404, { ok: false, error: "pairing_not_found" });
      return;
    }
    pairing.mobileToken = randomBytes(32).toString("base64url");
    pairing.status = "paired";
    pairing.updatedAt = new Date().toISOString();
    pairing.mobile = {
      deviceId: String(body.mobileDeviceId || ""),
      label: String(body.mobileDeviceName || ""),
      platform: String(body.platform || "linux"),
      secureMesh: body.secureMesh || body.mobileSecureMesh || null,
      secureMeshClaimProof: String(body.secureMeshClaimProof || "")
    };
    sendJson(response, 200, relayResult({
      pairing: publicPairing(pairing),
      pairingId: pairing.pairingId,
      mobileToken: pairing.mobileToken
    }));
  }

  pairingStatus(body, request, response) {
    const pairing = this.authorizedPairing(body, request, "either");
    if (!pairing) {
      sendJson(response, 401, { ok: false, error: "invalid_pairing_token" });
      return;
    }
    sendJson(response, 200, relayResult({ pairing: publicPairing(pairing) }));
  }

  createCommand(body, request, response) {
    const pairing = this.authorizedPairing(body, request, "mobile");
    if (!pairing) {
      sendJson(response, 401, { ok: false, error: "invalid_mobile_token" });
      return;
    }
    const secureEnvelope = body?.payload?.envelope || body?.secureEnvelope || null;
    if (!secureEnvelope || typeof secureEnvelope !== "object") {
      sendJson(response, 426, { ok: false, error: "secure_envelope_required" });
      return;
    }
    const now = new Date().toISOString();
    const command = {
      commandId: `cmd_${randomUUID()}`,
      pairingId: pairing.pairingId,
      type: String(body.type || "secure_mesh.envelope"),
      secureEnvelope,
      resultEnvelope: null,
      status: "pending",
      createdAt: now,
      updatedAt: now,
      deliveredAt: "",
      completedAt: ""
    };
    pairing.commands.push(command);
    sendJson(response, 200, relayResult({ command: publicCommand(command) }));
  }

  pollCommands(body, request, response) {
    const pairing = this.authorizedPairing(body, request, "pc");
    if (!pairing) {
      sendJson(response, 401, { ok: false, error: "invalid_pc_token" });
      return;
    }
    const now = new Date().toISOString();
    const commands = pairing.commands.filter((command) => command.status === "pending");
    for (const command of commands) {
      command.status = "in_progress";
      command.deliveredAt = now;
      command.updatedAt = now;
    }
    sendJson(response, 200, relayResult({ commands: commands.map(publicCommand) }));
  }

  completeCommand(commandId, body, request, response) {
    const pairing = this.authorizedPairing(body, request, "pc");
    const command = pairing?.commands.find((entry) => entry.commandId === commandId);
    if (!pairing || !command || !body.secureEnvelope) {
      sendJson(response, 404, { ok: false, error: "command_not_found" });
      return;
    }
    command.resultEnvelope = body.secureEnvelope;
    command.status = body.ok === false ? "failed" : "completed";
    command.completedAt = new Date().toISOString();
    command.updatedAt = command.completedAt;
    sendJson(response, 200, relayResult({ command: publicCommand(command) }));
  }

  commandResult(commandId, body, request, response) {
    const pairing = this.authorizedPairing(body, request, "mobile");
    const index = pairing?.commands.findIndex((entry) => entry.commandId === commandId) ?? -1;
    if (!pairing || index < 0) {
      sendJson(response, 404, { ok: false, error: "command_not_found" });
      return;
    }
    const [command] = pairing.commands.splice(index, 1);
    sendJson(response, 200, relayResult({
      command: publicCommand(command),
      ackPurge: { acknowledged: true, purged: true }
    }));
  }

  authorizedPairing(body, request, role) {
    const pairing = this.pairings.get(String(body.pairingId || ""));
    if (!pairing) return null;
    const token = bearerToken(request) || String(body.pcToken || body.mobileToken || body.token || "");
    const pc = token && token === pairing.pcToken;
    const mobile = token && token === pairing.mobileToken;
    if ((role === "pc" && !pc) || (role === "mobile" && !mobile) ||
      (role === "either" && !pc && !mobile)) return null;
    return pairing;
  }

  scanPlaintext(body) {
    const serialized = JSON.stringify(body);
    for (const marker of this.plaintextMarkers) {
      if (serialized.includes(marker)) this.plaintextObserved = true;
    }
  }

  observeMarker(marker) {
    this.plaintextMarkers.add(marker);
  }

  async stop() {
    this.server.closeAllConnections?.();
    const stopped = await Promise.race([
      new Promise((resolve) => this.server.close(() => resolve(true))),
      new Promise((resolve) => setTimeout(() => resolve(false), 5_000))
    ]);
    this.pairings.clear();
    this.plaintextMarkers.clear();
    return stopped;
  }
}
