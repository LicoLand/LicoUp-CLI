import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (relative) => fs.readFile(path.join(root, relative), "utf8");

test("conversation attachments use one typed Rust wire", async () => {
  const rustParams = "crates/licoup-agent-drivers/src/runtime_adapters/params.rs";
  // The Codex protocol, including the attachment lane it carries (`localImage`),
  // is the Codex adapter package's since CODEX-PACKAGE moved it out of the
  // kernel, so the attachment field names are read from their owner.
  const codexSession = "crates/licoup-agent-codex/src/parser/session.rs";
  const [params, session] = await Promise.all([rustParams, codexSession].map(read));

  for (const field of ["id", "name", "mediaType", "path"]) {
    assert.match(params, new RegExp(`"${field}"`));
  }
  assert.match(params, /MAX_IMAGE_ATTACHMENTS:\s*usize\s*=\s*4/u);
  assert.match(session, /"type":\s*"localImage"/u);
});
