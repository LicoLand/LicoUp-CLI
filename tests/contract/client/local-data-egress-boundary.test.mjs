import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

const productionRoots = Object.freeze([
  ["crates/licoup-agent-drivers/src", ".rs"],
  ["crates/licoup-native/src", ".rs"],
  ["crates/licoup-public-gateway/src", ".rs"],
  ["crates/licoup-agent-targets/src", ".rs"],
  ["crates/licoup-private-gateway/src", ".rs"],
  ["crates/licoup-model-catalog/src", ".rs"],
  ["apps/desktop/lib", ".dart"],
  ["apps/desktop/android/app/src/main", ".kt"],
  ["apps/desktop/ios/Runner", ".swift"],
]);

const networkTokensByExtension = Object.freeze({
  ".rs": ["ureq::", "reqwest::", "TcpStream", "UdpSocket"],
  ".dart": ["HttpClient", "WebSocket.connect", "package:http", "package:dio"],
  ".kt": ["HttpURLConnection", "OkHttpClient", "java.net.URL", "java.net.Socket"],
  ".swift": ["URLSession", "NWConnection", "URLSessionWebSocketTask"],
});

const reviewedRustEgressFiles = Object.freeze([
  "crates/licoup-agent-drivers/src/local_service/http.rs",
  "crates/licoup-agent-drivers/src/local_service/sse.rs",
  "crates/licoup-agent-targets/src/domain/lico_agent/transport.rs",
  "crates/licoup-model-catalog/src/identity/source.rs",
  "crates/licoup-model-catalog/src/pricing.rs",
  "crates/licoup-native/src/domain/agent_hub/acquisition.rs",
  "crates/licoup-native/src/domain/agent_usage/agent_usage_native/cursor.rs",
  "crates/licoup-native/src/domain/client_update/github_source.rs",
  "crates/licoup-native/src/domain/collaboration_plugin/assembly/runtime/probe.rs",
  "crates/licoup-native/src/domain/collaboration_plugin/assembly/runtime/shutdown.rs",
  "crates/licoup-native/src/domain/collaboration_plugin/source.rs",
  "crates/licoup-native/src/domain/provider_quota/http.rs",
  "crates/licoup-native/src/platform/badtower_station/http_io.rs",
  "crates/licoup-native/src/platform/llm_gateway_service.rs",
  "crates/licoup-native/src/platform/mcp_streamable_http.rs",
  "crates/licoup-private-gateway/src/http/server.rs",
  "crates/licoup-private-gateway/src/http/transport.rs",
  "crates/licoup-public-gateway/src/lifecycle.rs",
  "crates/licoup-public-gateway/src/telegram/transport.rs",
  "crates/licoup-public-gateway/src/transport.rs",
]);

async function sourceFiles(relativeRoot, extension) {
  const discovered = [];
  async function visit(relativeDirectory) {
    const entries = await fs.readdir(path.join(repoRoot, relativeDirectory), {
      withFileTypes: true,
    });
    for (const entry of entries) {
      const relativePath = path.posix.join(relativeDirectory, entry.name);
      if (entry.isDirectory()) {
        await visit(relativePath);
      } else if (entry.isFile() && entry.name.endsWith(extension)) {
        discovered.push(relativePath);
      }
    }
  }
  await visit(relativeRoot);
  return discovered.sort();
}

async function networkCapableSources() {
  const matches = [];
  for (const [relativeRoot, extension] of productionRoots) {
    const tokens = networkTokensByExtension[extension];
    for (const relativePath of await sourceFiles(relativeRoot, extension)) {
      if (relativePath.includes("/tests/")) continue;
      const source = await fs.readFile(path.join(repoRoot, relativePath), "utf8");
      if (tokens.some((token) => source.includes(token))) matches.push(relativePath);
    }
  }
  return matches.sort();
}

test("production network capability stays inside the reviewed client egress boundary", async () => {
  assert.deepEqual(await networkCapableSources(), reviewedRustEgressFiles);
});

test("GitHub package fetchers are bounded inbound GET-only sources", async () => {
  const collaborationPath =
    "crates/licoup-native/src/domain/collaboration_plugin/source.rs";
  const updatePath =
    "crates/licoup-native/src/domain/client_update/github_source.rs";
  const registryPath =
    "crates/licoup-model-catalog/src/identity/source.rs";
  const [collaboration, update, registry] = await Promise.all(
    [collaborationPath, updatePath, registryPath].map((relativePath) =>
      fs.readFile(path.join(repoRoot, relativePath), "utf8"),
    ),
  );

  assert.match(collaboration, /Url::parse\("https:\/\/api\.github\.com"\)/u);
  assert.match(collaboration, /\.get\(archive_url\.as_str\(\)\)/u);
  assert.match(collaboration, /MAX_GITHUB_ARCHIVE_BYTES/u);
  assert.match(collaboration, /MAX_GITHUB_ARCHIVE_ENTRIES/u);
  assert.match(collaboration, /MAX_GITHUB_ARCHIVE_DEPTH/u);
  assert.match(collaboration, /api\.github\.com.*github\.com.*codeload\.github\.com/su);

  assert.match(update, /DEFAULT_API_BASE: &str = "https:\/\/api\.github\.com"/u);
  assert.match(update, /agent\.get\(&current\)/u);
  assert.match(update, /MAX_REDIRECTS/u);
  assert.match(update, /MAX_RELEASE_METADATA_BYTES/u);
  assert.match(update, /MAX_ARTIFACT_DOWNLOAD_BYTES/u);
  assert.match(update, /\.take\(max_bytes\.saturating_add\(1\)\)/u);

  assert.ok(registry.includes('"https://codeload.github.com/anomalyco/models.dev/tar.gz/refs/heads/dev"'));
  assert.ok(registry.includes("fetch(REPOSITORY_SOURCE)"));
  assert.ok(registry.includes(".get(url)"));
  assert.ok(registry.includes(".take(MAX_DOWNLOAD_BYTES + 1)"));
  assert.ok(registry.includes("decoded_bytes <= MAX_DOWNLOAD_BYTES"));

  for (const [relativePath, source] of [
    [collaborationPath, collaboration],
    [updatePath, update],
    [registryPath, registry],
  ]) {
    for (const forbidden of [".post(", ".send_json(", 'set("Authorization"']) {
      assert.equal(source.includes(forbidden), false, `${relativePath} contains ${forbidden}`);
    }
  }
});

test("reviewed runtime owners retain direction, endpoint, and data bounds", async () => {
  const expectations = new Map([
    ["crates/licoup-native/src/domain/agent_hub/acquisition.rs", [
      "MAX_ARTIFACT_BYTES", "MAX_DIGEST_DOCUMENT_BYTES", "MAX_REDIRECTS",
      ".redirects(0)", ".get(&current)",
      "origin.admit(hop, &current)?",
      ".take(max_bytes.saturating_add(1))",
    ]],
    ["crates/licoup-native/src/domain/agent_usage/agent_usage_native/cursor.rs", [
      'EVENTS_URL: &str = "https://cursor.com/api/dashboard/get-filtered-usage-events"',
      "is_https_or_loopback_http_url(EVENTS_URL)",
      "MAX_PAGES", "MAX_PAGE_BYTES", ".take(MAX_PAGE_BYTES.saturating_add(1))",
      '"Origin"',
    ]],
    ["crates/licoup-agent-targets/src/domain/lico_agent/transport.rs", [
      'strip_prefix("http://")', 'host != "127.0.0.1"',
      "TcpStream::connect_timeout", "set_read_timeout", "Content-Length",
    ]],
    ["crates/licoup-model-catalog/src/pricing.rs", [
      'parsed.scheme() != "https"', ".timeout_connect(REFRESH_TIMEOUT)",
      ".get(url)", "MAX_PRICE_PAGE_BYTES", ".take(MAX_PRICE_PAGE_BYTES + 1)",
    ]],
    ["crates/licoup-native/src/domain/provider_quota/http.rs", [
      "HOSTED_FETCH_TIMEOUT", "LOOPBACK_FETCH_TIMEOUT", "MAX_RESPONSE_BYTES",
      "quota_endpoint_url_rejected", "quota_loopback_url_rejected",
      ".take(MAX_RESPONSE_BYTES.saturating_add(1))",
    ]],
    ["crates/licoup-native/src/platform/badtower_station/http_io.rs", [
      "HTTP_TIMEOUT_SECONDS", "MAX_ERROR_RESPONSE_BYTES", "read_bounded",
      ".take(take_limit)",
    ]],
    ["crates/licoup-public-gateway/src/telegram/transport.rs", [
      'DEFAULT_API_ROOT: &str = "https://api.telegram.org"',
      "MAX_RESPONSE_BYTES", ".timeout_connect", ".send_json(body)",
    ]],
    ["crates/licoup-private-gateway/src/http/server.rs", [
      "if !address.ip().is_loopback()", "MAX_HEADER_BYTES", "MAX_HEADERS",
      "MAX_REQUESTS_PER_CONNECTION", "MAX_GATEWAY_BODY_BYTES",
    ]],
    ["crates/licoup-native/src/platform/llm_gateway_service.rs", [
      "MAX_CONFIG_BYTES", "MAX_PID_BYTES", "TcpStream::connect_timeout",
      'GET /health HTTP/1.1',
    ]],
    ["crates/licoup-private-gateway/src/http/transport.rs", [
      "MAX_IN_FLIGHT", "MAX_COALESCED_WRITE_BYTES", ".post(&prepared.endpoint)",
      'request.set("authorization"', "MAX_GATEWAY_BODY_BYTES",
    ]],
    ["crates/licoup-agent-drivers/src/local_service/http.rs", [
      "MAX_HTTP_REQUEST_BODY_BYTES", "MAX_HTTP_RESPONSE_BODY_BYTES",
      "MAX_HTTP_HEADER_BYTES", "MAX_HTTP_IN_FLIGHT", "is_https_or_loopback_http_url",
    ]],
    ["crates/licoup-agent-drivers/src/local_service/sse.rs", [
      "MAX_SSE_LINE_BYTES", "MAX_SSE_FRAME_BYTES", "MAX_SSE_EVENTS_PER_STREAM",
      "MAX_SSE_STREAMS", "http::validate_url", "http::validate_headers",
    ]],
    ["crates/licoup-native/src/platform/mcp_streamable_http.rs", [
      "DEFAULT_MAX_MESSAGE_BYTES", "MAX_HTTP_HEADERS", "MAX_HTTP_HEADER_BYTES",
      "MAX_HTTP_IN_FLIGHT", "validate_endpoint", ".post(endpoint.as_str())",
    ]],
  ]);

  for (const [relativePath, required] of expectations) {
    const source = await fs.readFile(path.join(repoRoot, relativePath), "utf8");
    for (const token of required) {
      assert.equal(source.includes(token), true, `${relativePath} missing ${token}`);
    }
  }
});

test("local assembly runtime networking is synthetic loopback inspection only", async () => {
  const applyPath =
    "crates/licoup-native/src/domain/collaboration_plugin/assembly/apply.rs";
  const probePath =
    "crates/licoup-native/src/domain/collaboration_plugin/assembly/runtime/probe.rs";
  const shutdownPath =
    "crates/licoup-native/src/domain/collaboration_plugin/assembly/runtime/shutdown.rs";
  const sandboxPath =
    "crates/licoup-native/src/domain/collaboration_plugin/assembly/runtime/sandbox.rs";
  const sandboxOwnerPath =
    "crates/licoup-native/src/platform/process_sandbox/seatbelt.rs";
  const [apply, probe, shutdown, sandbox, sandboxOwner] = await Promise.all(
    [applyPath, probePath, shutdownPath, sandboxPath, sandboxOwnerPath].map((relativePath) =>
      fs.readFile(path.join(repoRoot, relativePath), "utf8"),
    ),
  );

  assert.match(apply, /TcpListener::bind\(\("127\.0\.0\.1", 0\)\)/u);
  assert.match(apply, /TcpListener::bind\(\("127\.0\.0\.1", port\)\)/u);
  assert.match(apply, /"runtimeCapability": super::runtime::SANDBOX_CAPABILITY/u);

  assert.match(probe, /get_json\(record\.port, "\/health"\)/u);
  assert.match(probe, /get_json\(record\.port, "\/v1\/capabilities"\)/u);
  assert.match(probe, /SocketAddr::from\(\(\[127, 0, 0, 1\], port\)\)/u);
  assert.match(probe, /GET \{path\} HTTP\/1\.1/u);
  assert.match(probe, /"assemblyManifestDigestSha256"/u);
  assert.match(probe, /"runtimePid"/u);

  assert.match(shutdown, /SocketAddr::from\(\(\[127, 0, 0, 1\], record\.port\)\)/u);
  assert.match(shutdown, /POST \/v1\/shutdown HTTP\/1\.1/u);
  assert.match(shutdown, /"assemblyManifestDigestSha256"/u);
  assert.match(shutdown, /TcpListener::bind\(\("127\.0\.0\.1", 0\)\)/u);

  assert.match(sandbox, /process_sandbox::CAPABILITY_COLLABORATION_LOOPBACK/u);
  assert.match(sandbox, /process_sandbox::collaboration_loopback_command/u);
  assert.match(sandboxOwner, /"platform-loopback-isolated-runtime-v1"/u);
  assert.match(sandboxOwner, /allow network-bind \(local tcp \\"localhost:\{port\}\\"\)/u);
  assert.match(sandboxOwner, /allow network-inbound \(local tcp \\"localhost:\{port\}\\"\)/u);
  const runtimeLeaves = [
    [applyPath, apply],
    [probePath, probe],
    [shutdownPath, shutdown],
    [sandboxPath, sandbox],
    [sandboxOwnerPath, sandboxOwner],
  ];
  for (const forbidden of [
    'params.get("url")',
    'params.get("remoteAddress")',
    'params.get("upload")',
    'TcpListener::bind(("0.0.0.0"',
    "Url::parse",
    "reqwest::",
    "ureq::",
    "UdpSocket",
    ".post(",
    "upload_file",
  ]) {
    for (const [relativePath, source] of runtimeLeaves) {
      assert.equal(source.includes(forbidden), false, `${relativePath} contains ${forbidden}`);
    }
  }
});

test("MCP HTTP egress is reachable only through an exact one-shot direct approval", async () => {
  const approval = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/domain/mcp_adapter/approval.rs"),
    "utf8",
  );
  const execution = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/domain/mcp_adapter/execution.rs"),
    "utf8",
  );
  const command = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/ffi/commands/mcp.rs"),
    "utf8",
  );
  const transport = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/platform/mcp_streamable_http.rs"),
    "utf8",
  );

  assert.match(approval, /Some\("direct-user"\)/u);
  assert.match(approval, /Some\(true\)/u);
  assert.match(approval, /supplied\.eq_ignore_ascii_case\(&scope\.approval_digest\)/u);
  assert.match(execution, /require_direct_confirmation\(params, &scope\)\?/u);
  assert.match(execution, /let planned_digest = plans\.claim\(plan_id\)\?/u);
  assert.match(execution, /planned_digest == scope\.approval_digest/u);
  assert.match(command, /execute_http_transfer\(&params, &plans/u);
  assert.match(command, /mcp_streamable_http::exchange\(packet, session_id\)/u);
  assert.equal(transport.includes("direct-user"), false);
  assert.equal(transport.includes("approvalDigest"), false);
});
