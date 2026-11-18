import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

test("MCP execution retains exact one-shot direct approval ownership", async () => {
  const approval = await fs.readFile(
    path.join(repoRoot, "crates/licoup/src/domain/mcp_adapter/approval.rs"),
    "utf8",
  );
  const execution = await fs.readFile(
    path.join(repoRoot, "crates/licoup/src/domain/mcp_adapter/execution.rs"),
    "utf8",
  );

  assert.match(approval, /Some\("direct-user"\)/u);
  assert.match(approval, /Some\(true\)/u);
  assert.match(approval, /supplied\.eq_ignore_ascii_case\(&scope\.approval_digest\)/u);
  assert.match(execution, /require_direct_confirmation\(params, &scope\)\?/u);
  assert.match(execution, /let planned_digest = plans\.claim\(plan_id\)\?/u);
  assert.match(execution, /planned_digest == scope\.approval_digest/u);
});
