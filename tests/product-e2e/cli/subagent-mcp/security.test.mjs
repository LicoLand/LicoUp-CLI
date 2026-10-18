import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const supervisor = readFileSync(
  "crates/licoup-public-gateway/src/transport.rs",
  "utf8",
);
const connector = readFileSync(
  "crates/licoup-public-gateway/src/connector.rs",
  "utf8",
);

test("service is bounded authenticated loopback with private discovery", () => {
  assert.match(supervisor, /Ipv4Addr::LOCALHOST/u);
  assert.match(supervisor, /constant_time_eq/u);
  assert.match(supervisor, /Bearer /u);
  assert.match(supervisor, /MAX_HTTP_CONNECTIONS: usize = 32/u);
  assert.match(supervisor, /MAX_SESSIONS: usize = 64/u);
  assert.match(supervisor, /MAX_TOOL_WORKERS: usize = 8/u);
  assert.match(supervisor, /atomic_write_private_text_bounded/u);
  assert.match(supervisor, /impl Drop for SubagentMcpSupervisor/u);
});

test("connector reports uncertain effects without replaying the request", () => {
  assert.doesNotMatch(connector, /fn tool_catalog|fn call_tool|thread::sleep/u);
  const forward = connector.slice(
    connector.indexOf("fn forward("),
    connector.indexOf("fn module_unavailable("),
  );
  assert.equal([...forward.matchAll(/connector_exchange\(/gu)].length, 1);
  assert.match(connector, /mcp_outcome_unknown/u);
  assert.match(connector, /reconcile_before_retry/u);
  assert.match(connector, /connector_exchange/u);
  assert.match(connector, /session_id/u);
  assert.match(connector, /connector_close_session\(&discovery, session_id\)/u);
  assert.match(connector, /202 if response\.is_empty/u);
});
