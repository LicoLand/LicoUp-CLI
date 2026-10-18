#!/usr/bin/env node
import { executeProbe, isDirectExecution, printReceipt } from "./common.mjs";

const TRANSIENT_SERVER_DECLARATION = [
  "-c",
  'mcp_servers={ "land.lico.licoup.subagents" = { command = "lico-subagent-mcp", args = ["--caller", "codex"] } }',
  "mcp",
  "list",
  "--json",
];

export function probeCodexStartup(options = {}) {
  return executeProbe({
    agent: "codex",
    executable: options.executable ?? process.env.LICOUP_CODEX_EXECUTABLE ?? "codex",
    args: TRANSIENT_SERVER_DECLARATION,
    inspect: options.inspect,
  });
}

if (isDirectExecution(import.meta.url)) printReceipt(await probeCodexStartup());
