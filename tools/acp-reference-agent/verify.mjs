#!/usr/bin/env node
/**
 * Focused client↔reference-agent roundtrip for ACP stdio NDJSON.
 * Synthetic prompts only; no live vendor binaries required.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";
import { AcpClient } from "../../tests/product-e2e/cli/agent-conversations/support/parity/clients/acp-client.mjs";

const agentPath = resolve(fileURLToPath(new URL("./agent.mjs", import.meta.url)));
const timeoutMs = 30_000;
const maxOutputBytes = 4 * 1024 * 1024;

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

function assert(condition, message) {
  if (!condition) fail(message);
}

async function main() {
  const tempRoot = mkdtempSync(join(tmpdir(), "lico-acp-reference-"));
  const statePath = join(tempRoot, "state.json");
  writeFileSync(statePath, JSON.stringify({ counter: 0, sessions: {} }));

  const client = new AcpClient(
    process.execPath,
    [agentPath, "acp"],
    {
      cwd: tempRoot,
      timeoutMs,
      maxOutputBytes,
      environment: {
        ...process.env,
        LICO_ACP_REFERENCE_STATE: statePath,
      },
    },
  );

  try {
    const init = await client.initialize();
    assert(init?.agentInfo?.name === "lico-acp-reference-agent", "initialize agentInfo mismatch");
    assert(init?.agentCapabilities?.loadSession === true, "loadSession unavailable");

    const created = await client.request("session/new", { cwd: tempRoot, mcpServers: [] });
    const sessionId = created?.sessionId;
    assert(typeof sessionId === "string" && sessionId.length > 0, "session/new missing sessionId");

    const startIndex = client.notifications.length;
    const hardDeadlineAt = performance.now() + timeoutMs;
    client.beginPromptNotificationValidation(startIndex, sessionId);
    const turn = await client.request("session/prompt", {
      sessionId,
      prompt: [{ type: "text", text: "Reply with exactly 8642" }],
    });
    const updates = await client.waitForPromptNotificationQuiescence(
      startIndex,
      sessionId,
      hardDeadlineAt,
    );
    const chunks = updates
      .filter((entry) => entry?.params?.update?.sessionUpdate === "agent_message_chunk")
      .map((entry) => entry.params.update.content.text);
    assert(turn?.stopReason === "end_turn", "prompt stopReason mismatch");
    assert(chunks.join("") === "8642", "streamed reply mismatch");

    const listed = await client.request("session/list", {});
    assert(
      listed?.sessions?.some((entry) => entry.sessionId === sessionId),
      "session/list missing created session",
    );

    const loaded = await client.request("session/load", { sessionId, cwd: tempRoot, mcpServers: [] });
    assert(loaded?.sessionId === sessionId, "session/load sessionId mismatch");

    const resumed = await client.request("session/resume", { sessionId, cwd: tempRoot, mcpServers: [] });
    assert(resumed?.sessionId === sessionId, "session/resume sessionId mismatch");

    const closed = await client.request("session/close", { sessionId });
    assert(closed !== undefined, "session/close failed");

    const afterClose = await client.request("session/list", {});
    assert(
      !afterClose?.sessions?.some((entry) => entry.sessionId === sessionId),
      "session/close did not remove session",
    );
  } finally {
    await client.close();
    rmSync(tempRoot, { recursive: true, force: true });
  }

  process.stdout.write("acp-reference-agent:self-test passed\n");
}

main().catch((error) => {
  fail(error?.message || "acp-reference-agent:self-test failed");
});
