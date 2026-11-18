#!/usr/bin/env node
import { executeProbe, isDirectExecution, printReceipt } from "./common.mjs";

export function probeAntigravityStartup(options = {}) {
  return executeProbe({
    agent: "antigravity",
    executable: options.executable ?? process.env.LICOUP_ANTIGRAVITY_EXECUTABLE ?? "agy",
    args: ["mcp", "list"],
    format: "text",
    installerOnly: true,
    inspect: options.inspect,
  });
}

if (isDirectExecution(import.meta.url)) printReceipt(await probeAntigravityStartup());
