import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const root = "crates/licoup-agent-drivers/src/local_service";
const facadePath = `${root}.rs`;
const productionLeaves = Object.freeze([
  "bounds.rs",
  "concurrency.rs",
  "endpoint.rs",
  "executable.rs",
  "http.rs",
  "params.rs",
  "port.rs",
  "process.rs",
  "serve.rs",
  "sse.rs",
  "state.rs",
  "turn_control.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

test("local service facade owns an exact target-neutral production bundle", async () => {
  const facade = await read(facadePath);
  for (const leaf of productionLeaves) {
    assert.match(facade, new RegExp(`mod ${leaf.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, root, leaf));
  }
  const entries = await fs.readdir(path.join(repoRoot, root), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    [...productionLeaves].sort(),
  );
});

test("HTTP body headers timeout and concurrency remain explicitly bounded", async () => {
  const bounds = await read(`${root}/bounds.rs`);
  const http = await read(`${root}/http.rs`);
  for (const token of [
    "MAX_HTTP_REQUEST_BODY_BYTES",
    "MAX_HTTP_RESPONSE_BODY_BYTES",
    "MAX_HTTP_HEADER_COUNT",
    "MAX_HTTP_HEADER_BYTES",
    "MAX_HTTP_IN_FLIGHT",
    "CONCURRENCY_WAIT",
  ]) {
    assert.match(bounds, new RegExp(token, "u"));
    assert.match(http, new RegExp(token, "u"));
  }
  assert.match(http, /\.take\(\(MAX_HTTP_RESPONSE_BODY_BYTES as u64\)\.saturating_add\(1\)\)/u);
  assert.match(http, /validate_headers/u);
  assert.match(http, /control_agent/u);
  assert.match(http, /timeout_connect/u);
  assert.match(http, /\.timeout\(timeout\)/u);
  assert.match(http, /try_proxy_from_env\(false\)/u);
  assert.match(http, /is_https_or_loopback_http_url/u);
});

test("SSE line frame event timeout and stream concurrency remain bounded", async () => {
  const bounds = await read(`${root}/bounds.rs`);
  const sse = await read(`${root}/sse.rs`);
  for (const token of [
    "MAX_SSE_STREAMS",
    "MAX_SSE_LINE_BYTES",
    "MAX_SSE_FRAME_BYTES",
    "MAX_SSE_DATA_LINES",
    "MAX_SSE_EVENTS_PER_STREAM",
  ]) {
    assert.match(bounds, new RegExp(token, "u"));
    assert.match(sse, new RegExp(token, "u"));
  }
  assert.match(sse, /fill_buf\(\)/u);
  assert.equal(sse.includes("read_line("), false);
  assert.match(sse, /timeout_connect/u);
  assert.match(sse, /timeout_read/u);
});

test("state PID process remain private and neutral serve owns no event decoding", async () => {
  const state = await read(`${root}/state.rs`);
  const process = await read(`${root}/process.rs`);
  const serve = await read(`${root}/serve.rs`);
  assert.match(state, /read_private_text_bounded/u);
  assert.match(state, /atomic_write_private_text_bounded/u);
  assert.match(state, /try_lock_exclusive/u);
  assert.match(process, /stdout\(Stdio::null\(\)\)/u);
  assert.match(process, /stderr\(Stdio::null\(\)\)/u);
  assert.equal(serve.includes("message.updated"), false);
  assert.equal(serve.includes("message.part.updated"), false);
  assert.equal(serve.includes("assistant_messages"), false);
  assert.equal(serve.includes("serde_json::from_str"), false);
  assert.equal(serve.includes('"state": service_state'), false);
  assert.equal(serve.includes('"stateDir"'), false);
});

test("turn control remains exact-session bounded and owns no target policy", async () => {
  const turnControl = await read(`${root}/turn_control.rs`);
  for (const token of [
    "MAX_ACTIVE_TURNS",
    "CONTROL_TIMEOUT",
    "ActiveTurnGuard",
    "session_action_url",
    "generation",
    "impl Drop",
  ]) assert.match(turnControl, new RegExp(token, "u"));
  assert.match(turnControl, /#\[cfg\(test\)\]\s+mod tests/u);
  assert.equal(turnControl.includes("opencode"), false);
  assert.equal(turnControl.includes("kilo"), false);
});

test("HTTP and SSE foundation does not absorb ACP JSONL or target policy", async () => {
  const sources = await Promise.all([
    read(facadePath),
    ...productionLeaves.map((leaf) => read(`${root}/${leaf}`)),
  ]);
  const joined = sources.join("\n");
  for (const forbidden of [
    "licoup_foundation::core::acp",
    "decode_json_line",
    "MAX_JSON_LINE_BYTES",
    "opencode_serve",
    "kilo_code_serve",
    "openclaw_gateway",
    "unsafe {",
  ]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
  const actualEgress = [];
  for (const leaf of productionLeaves) {
    if ((await read(`${root}/${leaf}`)).includes("ureq::")) actualEgress.push(leaf);
  }
  assert.deepEqual(actualEgress, ["http.rs", "sse.rs"]);
});
