export const MCP_PROTOCOL_REVISION = "2025-06-18";
export const MCP_SERVER_NAME = "lico-up-subagents";
export const MCP_SERVER_VERSION = "0.14.0";
export const DISCOVERY_SCHEMA = "licoup.subagent-mcp.discovery.v1";
const DISCOVERY_PROVIDERS = Object.freeze(["antigravity", "codex", "cursor"]);
export const FROZEN_TOOL_NAMES = Object.freeze([
  "lico_subagents_list",
  "lico_subagent_probe",
  "lico_subagent_delegate",
  "lico_subagent_continue",
  "lico_subagent_cancel",
]);

export function admitDiscoveryDocument(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("discovery_invalid");
  }
  const keys = Object.keys(value).sort();
  const tokenKeys = value.tokens && typeof value.tokens === "object" && !Array.isArray(value.tokens)
    ? Object.keys(value.tokens).sort()
    : [];
  if (
    JSON.stringify(keys) !== JSON.stringify(["controlToken", "endpoint", "generation", "schemaVersion", "tokens"])
    || value.schemaVersion !== DISCOVERY_SCHEMA
    || !/^[0-9a-f]{64}$/u.test(value.controlToken)
    || typeof value.endpoint !== "string"
    || !/^[0-9a-f]{32}$/u.test(value.generation)
    || JSON.stringify(tokenKeys) !== JSON.stringify(DISCOVERY_PROVIDERS)
    || tokenKeys.some((provider) => !/^[0-9a-f]{64}$/u.test(value.tokens[provider]))
  ) {
    throw new Error("discovery_invalid");
  }
  return value;
}

export class DirectMcpClient {
  constructor({ endpoint, token, conversationId = "", membershipId = "", fetchImpl = globalThis.fetch }) {
    const endpointMatch = typeof endpoint === "string"
      ? endpoint.match(/^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/mcp$/u)
      : null;
    let parsed;
    try { parsed = endpointMatch ? new URL(endpoint) : null; } catch { parsed = null; }
    if (
      !endpointMatch
      || Number(endpointMatch[1]) > 65_535
      || !parsed
      || parsed.protocol !== "http:"
      || parsed.hostname !== "127.0.0.1"
      || parsed.pathname !== "/mcp"
      || parsed.username
      || parsed.password
      || parsed.search
      || parsed.hash
    ) {
      throw new Error("discovery_endpoint_invalid");
    }
    if (typeof token !== "string" || !/^[0-9a-f]{64}$/u.test(token)) {
      throw new Error("discovery_token_invalid");
    }
    this.endpoint = endpoint;
    this.token = token;
    this.fetchImpl = fetchImpl;
    this.conversationId = conversationId;
    this.membershipId = membershipId;
    this.session = "";
    this.nextId = 1;
  }

  async initialize() {
    const response = await this.#post("initialize", {
      protocolVersion: MCP_PROTOCOL_REVISION,
      capabilities: {},
      clientInfo: { name: "licoup-verification", version: "1" },
    }, false);
    this.session = response.session;
    return response.body?.result;
  }

  async listTools() {
    return (await this.#post("tools/list", {})).body?.result?.tools ?? [];
  }

  async callTool(name, arguments_) {
    return (await this.#post("tools/call", { name, arguments: arguments_ })).body?.result;
  }

  async close() {
    if (!this.session) return;
    const session = this.session;
    this.session = "";
    const response = await this.fetchImpl(this.endpoint, {
      method: "DELETE",
      headers: this.#headers(session),
      redirect: "error",
    });
    if (response.status !== 204) throw new Error("mcp_close_failed");
  }

  async #post(method, params, requireSession = true) {
    if (requireSession && !this.session) throw new Error("mcp_session_missing");
    const id = this.nextId++;
    const response = await this.fetchImpl(this.endpoint, {
      method: "POST",
      headers: this.#headers(),
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      redirect: "error",
    });
    if (response.status !== 200) throw new Error("mcp_exchange_failed");
    const session = response.headers.get("mcp-session-id") ?? "";
    if (!/^[0-9a-f]{32}$/u.test(session) || (this.session && session !== this.session)) {
      throw new Error("mcp_session_missing");
    }
    if (response.headers.get("mcp-protocol-version") !== MCP_PROTOCOL_REVISION) {
      throw new Error("mcp_exchange_failed");
    }
    if (response.headers.get("content-type")?.split(";", 1)[0].trim().toLowerCase()
      !== "application/json") {
      throw new Error("mcp_exchange_failed");
    }
    if (!requireSession && !this.session) this.session = session;
    const body = await response.json();
    if (
      body?.jsonrpc !== "2.0"
      || body?.id !== id
      || body?.error
      || !Object.hasOwn(body ?? {}, "result")
    ) {
      throw new Error("mcp_application_failed");
    }
    return { body, session };
  }

  #headers(session = this.session) {
    const headers = {
      authorization: `Bearer ${this.token}`,
      "content-type": "application/json",
      "mcp-protocol-version": MCP_PROTOCOL_REVISION,
    };
    if (session) headers["mcp-session-id"] = session;
    if (this.conversationId) headers["x-licoup-conversation-id"] = this.conversationId;
    if (this.membershipId) headers["x-licoup-membership-id"] = this.membershipId;
    return headers;
  }
}

export async function verifyServiceHealth(client) {
  try {
    const initialized = await client.initialize();
    const tools = await client.listTools();
    const names = tools.map((tool) => tool?.name);
    if (
      initialized?.protocolVersion !== MCP_PROTOCOL_REVISION
      || initialized?.serverInfo?.name !== MCP_SERVER_NAME
      || initialized?.serverInfo?.version !== MCP_SERVER_VERSION
      || JSON.stringify(names) !== JSON.stringify(FROZEN_TOOL_NAMES)
    ) {
      throw new Error("service_catalog_mismatch");
    }
    return { result: "passed", reason: "service_healthy" };
  } finally {
    await client.close();
  }
}
