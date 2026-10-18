#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import {
  adapterIds,
  assertAdapterAndScenario,
  historySource,
  isWithin,
  scenarioClasses,
  schemaVersion,
  sha256,
  syntheticSource,
} from "./shared.mjs";

function usage() {
  throw new Error(`usage: record.mjs --adapter <${adapterIds.join("|")}> --scenario <${scenarioClasses.join("|")}> --output <private-raw.json> [--native-bin <licoup-cli>]`);
}

const options = process.argv.slice(2);
const option = (name) => {
  const index = options.indexOf(name);
  return index >= 0 ? options[index + 1] : undefined;
};
const adapterId = option("--adapter");
const scenario = option("--scenario");
const output = option("--output");
const nativeBin = resolve(option("--native-bin") || "build/crates/licoup-native/target/debug/licoup-cli");
const catalogCache = option("--catalog-cache") ? resolve(option("--catalog-cache")) : null;
if (!adapterId || !scenario || !output) usage();
assertAdapterAndScenario(adapterId, scenario);

const repositoryRoot = realpathSync(resolve(import.meta.dirname, "../../.."));
const outputPath = resolve(output);
if (isWithin(repositoryRoot, outputPath)) {
  throw new Error("raw_transcript_must_be_outside_repository");
}
mkdirSync(dirname(outputPath), { recursive: true, mode: 0o700 });

let sessions = [];
let catalogText = catalogCache && existsSync(catalogCache)
  ? readFileSync(catalogCache, "utf8")
  : null;
if (catalogText === null) {
  const list = spawnSync(nativeBin, [
    "conversations", "list", "--agent", adapterId, "--limit", "20",
  ], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    timeout: 30_000,
  });
  catalogText = list.status === 0 ? list.stdout : JSON.stringify({ sessions: [] });
  if (catalogCache) writeFileSync(catalogCache, catalogText, { mode: 0o600, flag: "wx" });
}
if (catalogText) {
  try {
    sessions = JSON.parse(catalogText).sessions || [];
  } catch {
    sessions = [];
  }
}

const text = (message) => String(message?.text || "");
const eventText = (session) => (session.messages || [])
  .filter((message) => ["event", "error", "metadata"].includes(message?.role))
  .map(text)
  .join("\n");
const scenarioMatch = (session) => {
  const messages = session.messages || [];
  switch (scenario) {
    case "normal-turn":
      return messages.some((message) => message?.role === "user")
        && messages.some((message) => message?.role === "agent");
    case "user-cancel":
      return /\b(cancel(?:led|ed)?|interrupt(?:ed|ion)?|abort(?:ed)?)\b/iu.test(eventText(session));
    case "agent-error":
      return messages.some((message) => message?.role === "error")
        || /\b(error|failed|failure|exception)\b/iu.test(eventText(session));
    case "streaming-interruption":
      return /\b(stream[^\n]{0,32}(?:interrupt|disconnect|incomplete)|unexpected eof|truncated)\b/iu.test(eventText(session));
    case "native-resume":
      return session.resumed === true
        || /\b(resume|reconnect|reattach|restore)\b/iu.test(eventText(session));
    default:
      return false;
  }
};
const selected = sessions.find(scenarioMatch);
const source = selected ? historySource : syntheticSource;
// Raw normalized messages are intentionally kept only in this private file.
// The redactor converts them to content-free replay markers before a candidate
// can enter the repository.
const rawMessages = selected?.messages || [];
const raw = {
  schemaVersion,
  adapterId,
  scenario,
  provenance: {
    source,
    taskContent: "synthetic-engineering-only",
    redacted: false,
    humanReviewed: false,
  },
  ingestion: {
    interface: "native-history-catalog",
    readOnly: true,
    catalogSessionsConsidered: sessions.length,
  },
  privateMessages: rawMessages,
};
const serialized = `${JSON.stringify(raw, null, 2)}\n`;
writeFileSync(outputPath, serialized, { mode: 0o600, flag: "wx" });
chmodSync(outputPath, 0o600);
process.stderr.write(`raw transcript written outside repository; sha256:${sha256(serialized)}\n`);
