import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (relative) => fs.readFile(path.join(root, relative), "utf8");

test("conversation attachments use one typed Dart-to-Rust wire", async () => {
  const dartContract = "apps/desktop/lib/src/contracts/agent_conversation_attachment.dart";
  const dartService = "apps/desktop/lib/src/platform/native_client/native_conversation_port.dart";
  const rustParams = "crates/licoup-agent-drivers/src/runtime_adapters/params.rs";
  // The Codex protocol, including the attachment lane it carries (`localImage`),
  // is the Codex adapter package's since CODEX-PACKAGE moved it out of the
  // kernel, so the attachment field names are read from their owner.
  const codexSession = "crates/licoup-agent-codex/src/parser/session.rs";
  const [contract, service, params, session] = await Promise.all(
    [dartContract, dartService, rustParams, codexSession].map(read),
  );

  for (const field of ["id", "name", "mediaType", "path"]) {
    assert.match(contract, new RegExp(`'${field}'\\s*:`));
    assert.match(params, new RegExp(`"${field}"`));
  }
  assert.match(service, /'attachments':\s*\[/u);
  assert.match(params, /MAX_IMAGE_ATTACHMENTS:\s*usize\s*=\s*4/u);
  assert.match(session, /"type":\s*"localImage"/u);
  assert.doesNotMatch(service, /data:image|attach-url|base64Encode/u);
});

test("frontend image rendering keeps byte reads behind composition, never dart:io", async () => {
  const rendererPath = "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_image_attachments.dart";
  const workspacePath = "apps/desktop/lib/src/frontend/features/agents/ui/agent_conversation_workspace.dart";
  const compositionPath = "apps/desktop/lib/src/composition/features/conversation/conversation_feature_composition.dart";
  const [renderer, workspace, composition] = await Promise.all([
    read(rendererPath),
    read(workspacePath),
    read(compositionPath),
  ]);

  assert.match(renderer, /ConversationImageLoaderScope/u);
  assert.match(renderer, /ConversationImageLoaderScope\.maybeOf\(context\)/u);
  assert.match(composition, /conversationImageByteReader/u);
  assert.match(composition, /cacheAttachmentBytes\(bytesById/u);
  assert.doesNotMatch(renderer, /dart:io|File\s*\(/u);
  assert.doesNotMatch(workspace, /dart:io|File\s*\(/u);
});
