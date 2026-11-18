import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  STDIO_PROTOCOL,
  buildContract,
  cliVersion,
  conversationProtocolDigest,
} from "../../../tools/scripts/release/cli-release-contract.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const fixtureRoot = path.join(repoRoot, "tests/fixtures/conversation_protocol_digest");

// The expected value was computed independently of this implementation: the
// Desktop repository's copy of the function must produce the same hex digest
// over the same fixture tree.
const FIXTURE_DIGEST = "ae29b6ce3f72f22eb30d4813e6b91bec4cd1e2fbbff3595b2dbc4307baa1ef31";

test("the conversation protocol digest matches the shared fixture", () => {
  assert.equal(conversationProtocolDigest(fixtureRoot), FIXTURE_DIGEST);
});

test("the repository protocol tree digests deterministically", () => {
  const first = conversationProtocolDigest(repoRoot);
  assert.match(first, /^[0-9a-f]{64}$/u);
  assert.equal(conversationProtocolDigest(repoRoot), first);
});

test("the contract carries the binary's own catalog and refuses a foreign protocol", () => {
  const catalog = {
    schema: "licoup.cli-catalog/v1",
    commands: [{ path: ["devices", "status"] }],
    rpc: { protocol: STDIO_PROTOCOL },
  };
  const contract = buildContract({
    catalog,
    target: "aarch64-apple-darwin",
    version: cliVersion(repoRoot),
    digest: FIXTURE_DIGEST,
  });
  assert.deepEqual(Object.keys(contract), [
    "cliVersion",
    "target",
    "stdioProtocol",
    "conversationProtocolDigest",
    "commands",
  ]);
  assert.equal(contract.stdioProtocol, "licoup.stdio.v1");
  assert.deepEqual(contract.commands, catalog.commands);
  assert.throws(
    () => buildContract({ catalog: { ...catalog, rpc: { protocol: "other" } }, target: "aarch64-apple-darwin", version: "0.3.0", digest: FIXTURE_DIGEST }),
    /cli_stdio_protocol_mismatch/u,
  );
  assert.throws(
    () => buildContract({ catalog: { ...catalog, commands: [] }, target: "aarch64-apple-darwin", version: "0.3.0", digest: FIXTURE_DIGEST }),
    /cli_command_catalog_invalid/u,
  );
});
