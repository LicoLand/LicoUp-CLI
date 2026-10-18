#!/usr/bin/env node
import { executeProbe, isDirectExecution, printReceipt } from "./common.mjs";

export function probeCursorStartup(options = {}) {
  return executeProbe({
    agent: "cursor",
    executable: options.executable ?? process.env.LICOUP_CURSOR_EXECUTABLE ?? "cursor-agent",
    args: ["mcp", "list"],
    format: "text",
    installerOnly: true,
    inspect: options.inspect,
  });
}

if (isDirectExecution(import.meta.url)) printReceipt(await probeCursorStartup());
