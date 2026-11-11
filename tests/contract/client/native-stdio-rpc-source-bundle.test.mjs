import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

test("native conversation RPC uses a client-local persistent owner", async () => {
  const host = await read("crates/licoup/src/bin/licoup/conversation_host.rs");
  const server = await read("crates/licoup/src/bin/licoup/stdio_rpc/server/conversation.rs");
  for (const token of [
    "rpc\", \"conversation-host",
    "serve_stdio_rpc_with_persistent_conversation",
    "client_disconnected",
    "runtime.request_host_stop()",
    "runtime.wait_for_admitted_turns()",
    "runtime.wait_for_clients_to_disconnect()",
  ]) assert.ok(host.includes(token), `missing persistent host contract: ${token}`);
  assert.equal(host.includes("runtime.idle()"), false);
  for (const token of [
    "PersistentConversationRuntime",
    "turnHandle",
    "afterCursor",
    "record_event",
  ]) assert.ok(server.includes(token), `missing persistent replay contract: ${token}`);
});
