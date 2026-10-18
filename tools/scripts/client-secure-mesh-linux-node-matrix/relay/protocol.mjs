import { assert } from "../assert.mjs";

export function relayResult(payload) {
  return {
    ok: true,
    schemaVersion: "licomesh.mobile-relay.response-schema.v1",
    protocolVersion: "licomesh.mobile-relay.v1",
    ...payload
  };
}

export function publicPairing(pairing) {
  return {
    pairingId: pairing.pairingId,
    status: pairing.status,
    createdAt: pairing.createdAt,
    updatedAt: pairing.updatedAt,
    expiresAt: pairing.expiresAt,
    pc: { ...pairing.pc, tokenConfigured: true },
    mobile: pairing.mobile ? { ...pairing.mobile, tokenConfigured: true } : null
  };
}

export function publicCommand(command) {
  return {
    commandId: command.commandId,
    pairingId: command.pairingId,
    type: command.type,
    payload: {},
    secureEnvelope: command.secureEnvelope,
    envelope: command.secureEnvelope,
    resultEnvelope: command.resultEnvelope,
    status: command.status,
    createdAt: command.createdAt,
    updatedAt: command.updatedAt,
    deliveredAt: command.deliveredAt,
    completedAt: command.completedAt,
    result: null,
    error: ""
  };
}

export async function readJsonBody(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    assert(size <= 4 * 1024 * 1024, "Opaque relay request exceeded its bound");
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  } catch {
    throw new Error("Opaque relay received invalid JSON");
  }
}

export function sendJson(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
    "cache-control": "no-store"
  });
  response.end(body);
}

export function bearerToken(request) {
  const header = String(request.headers.authorization || "");
  return header.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
}
