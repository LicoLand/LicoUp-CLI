import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const frontendRoot = "apps/desktop/lib/src/frontend";

async function dartSources(relativeDirectory) {
  const sources = [];
  async function visit(directory) {
    const entries = await fs.readdir(path.join(repoRoot, directory), {
      withFileTypes: true,
    });
    for (const entry of entries) {
      const relativePath = path.posix.join(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(relativePath);
      } else if (entry.isFile() && entry.name.endsWith(".dart")) {
        sources.push([
          relativePath,
          await fs.readFile(path.join(repoRoot, relativePath), "utf8"),
        ]);
      }
    }
  }
  await visit(relativeDirectory);
  return sources;
}

test("MLS, KT, and MCP transfer execution stay outside the product frontend", async () => {
  const forbiddenConsumers = [
    "SecureMeshProtocolController",
    "SecureMeshMlsRequest",
    "SecureMeshKtRequest",
    ".executeMls(",
    ".executeKt(",
    "McpTransferController",
    ".executeHttpTransfer(",
    "mcp_transfer_controller.dart",
  ];

  for (const [relativePath, source] of await dartSources(frontendRoot)) {
    for (const forbidden of forbiddenConsumers) {
      assert.equal(
        source.includes(forbidden),
        false,
        `${relativePath} exposes internal capability consumer ${forbidden}`,
      );
    }
  }
});

test("MCP execution retains exact one-shot direct approval ownership", async () => {
  const approval = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/domain/mcp_adapter/approval.rs"),
    "utf8",
  );
  const execution = await fs.readFile(
    path.join(repoRoot, "crates/licoup-native/src/domain/mcp_adapter/execution.rs"),
    "utf8",
  );

  assert.match(approval, /Some\("direct-user"\)/u);
  assert.match(approval, /Some\(true\)/u);
  assert.match(approval, /supplied\.eq_ignore_ascii_case\(&scope\.approval_digest\)/u);
  assert.match(execution, /require_direct_confirmation\(params, &scope\)\?/u);
  assert.match(execution, /let planned_digest = plans\.claim\(plan_id\)\?/u);
  assert.match(execution, /planned_digest == scope\.approval_digest/u);
});
