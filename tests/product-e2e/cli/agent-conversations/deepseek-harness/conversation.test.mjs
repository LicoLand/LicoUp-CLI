#!/usr/bin/env node
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import deepseekHarness from "../../../../../../tools/regression/client-regression-entries/agents/deepseek-harness.mjs";

export async function deepseekHarnessConversationReadiness() {
  const probe = await deepseekHarness.probe();
  return Object.freeze({
    schemaVersion: "licoup.deepseek-harness-conversation-regression.v1",
    agentId: "deepseek-harness",
    status: probe.eligible ? "eligible" : "unverified",
    reason: probe.reason,
  });
}

const invoked = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) process.stdout.write(`${JSON.stringify(await deepseekHarnessConversationReadiness())}\n`);
