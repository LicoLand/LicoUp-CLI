#!/usr/bin/env node

/**
 * Front-end real conversation test for kimi-code:
 * launch the LicoUp UI, submit a message from the Composer widget, and assert
 * the agent's reply echoes back in the conversation timeline.
 */

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runUiConversation } from "../support/ui-conversation.mjs";

async function main() {
  const output = await runUiConversation({
    agent: "kimi-code",
    output: "",
  });
  process.stdout.write(`${JSON.stringify(output)}\n`);
  if (output.status !== "passed") process.exitCode = 1;
}

const invoked = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) {
  await main();
}
