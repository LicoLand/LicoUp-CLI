#!/usr/bin/env node
const args = process.argv.slice(2);
const isCodexList = args.length === 5
  && args[0] === "-c"
  && args[1].includes("land.lico.licoup.subagents")
  && JSON.stringify(args.slice(-3)) === JSON.stringify(["mcp", "list", "--json"]);
if (!isCodexList) {
  process.exitCode = 2;
} else {
  process.stdout.write(`${JSON.stringify({
    version: "1.2.3",
    mcpServers: [{ name: "land.lico.licoup.subagents" }],
  })}\n`);
}
