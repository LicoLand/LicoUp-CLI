#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
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

const defaultCatalogLimit = 100;
const defaultCatalogTimeoutMs = 60_000;

function usage() {
  throw new Error(
    `usage: record.mjs --adapter <${adapterIds.join("|")}> --scenario <${scenarioClasses.join("|")}> --output <private-raw.json> [--native-bin <licoup>] [--catalog-cache <private-catalog.json>] [--catalog-limit <positive-int>] [--catalog-timeout-ms <positive-int>]`,
  );
}

const options = process.argv.slice(2);
const option = (name) => {
  const index = options.indexOf(name);
  if (index < 0) return undefined;
  if (index + 1 >= options.length) usage();
  return options[index + 1];
};

function parsePositiveIntegerOption(flagName, envName, defaultValue, errorCode) {
  const flagValue = option(flagName);
  const envValue = process.env[envName];
  const raw = flagValue !== undefined ? flagValue : (envValue !== undefined && envValue !== "" ? envValue : undefined);
  if (raw === undefined) return defaultValue;
  if (!/^[1-9]\d*$/u.test(raw)) {
    throw new Error(`${errorCode}:${raw}`);
  }
  const parsed = Number(raw);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${errorCode}:${raw}`);
  }
  return parsed;
}

const adapterId = option("--adapter");
const scenario = option("--scenario");
const output = option("--output");
const nativeBin = resolve(option("--native-bin") || "build/crates/licoup/target/debug/licoup");
const rawCatalogCache = option("--catalog-cache");
if (rawCatalogCache !== undefined && rawCatalogCache.length === 0) usage();
if (!adapterId || !scenario || !output) usage();
assertAdapterAndScenario(adapterId, scenario);

const catalogLimit = parsePositiveIntegerOption(
  "--catalog-limit",
  "LICO_TRANSCRIPT_CATALOG_LIMIT",
  defaultCatalogLimit,
  "catalog_limit_must_be_positive_integer",
);
const catalogTimeoutMs = parsePositiveIntegerOption(
  "--catalog-timeout-ms",
  "LICO_TRANSCRIPT_CATALOG_TIMEOUT_MS",
  defaultCatalogTimeoutMs,
  "catalog_timeout_ms_must_be_positive_integer",
);

const repositoryRoot = realpathSync(resolve(import.meta.dirname, "../../.."));

function firstExistingAncestor(targetPath) {
  let current = targetPath;
  while (!existsSync(current)) {
    const parent = dirname(current);
    if (parent === current) break;
    current = parent;
  }
  return current;
}

function resolveOutsideRepository(rawPath, errorCode) {
  const candidate = resolve(rawPath);
  if (isWithin(repositoryRoot, candidate)) {
    throw new Error(errorCode);
  }
  const existingParent = firstExistingAncestor(dirname(candidate));
  if (existsSync(existingParent) && isWithin(repositoryRoot, realpathSync(existingParent))) {
    throw new Error(errorCode);
  }
  mkdirSync(dirname(candidate), { recursive: true, mode: 0o700 });
  const canonicalParent = realpathSync(dirname(candidate));
  const canonicalTarget = resolve(canonicalParent, basename(candidate));
  if (isWithin(repositoryRoot, canonicalParent) || isWithin(repositoryRoot, canonicalTarget)) {
    throw new Error(errorCode);
  }
  if (existsSync(canonicalTarget) && isWithin(repositoryRoot, realpathSync(canonicalTarget))) {
    throw new Error(errorCode);
  }
  return canonicalTarget;
}

const outputPath = resolveOutsideRepository(output, "raw_transcript_must_be_outside_repository");
const catalogCache = rawCatalogCache
  ? resolveOutsideRepository(rawCatalogCache, "catalog_cache_must_be_outside_repository")
  : null;

let sessions = [];
let catalogText = catalogCache && existsSync(catalogCache)
  ? readFileSync(catalogCache, "utf8")
  : null;
if (catalogText === null) {
  const list = spawnSync(nativeBin, [
    "conversations", "list", "--agent", adapterId, "--limit", String(catalogLimit),
  ], {
    cwd: repositoryRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    timeout: catalogTimeoutMs,
  });
  if (list.status !== 0) {
    const detail = list.error?.message || `exit_status:${list.status ?? "signal"}`;
    process.stderr.write(
      `warning: conversation catalog command failed for ${adapterId} (${detail}; timeoutMs=${catalogTimeoutMs}); falling back to ${syntheticSource}\n`,
    );
    catalogText = JSON.stringify({ sessions: [] });
  } else {
    catalogText = list.stdout;
  }
  if (catalogCache) {
    writeFileSync(catalogCache, catalogText, { mode: 0o600, flag: "wx" });
    chmodSync(catalogCache, 0o600);
  }
}
if (catalogText) {
  try {
    sessions = JSON.parse(catalogText).sessions || [];
  } catch {
    process.stderr.write(
      `warning: conversation catalog JSON could not be parsed for ${adapterId}; falling back to ${syntheticSource}\n`,
    );
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
if (!selected) {
  process.stderr.write(
    `warning: scenario ${scenario} not matched within ${sessions.length} catalog session(s) for ${adapterId} (limit=${catalogLimit}, timeoutMs=${catalogTimeoutMs}); falling back to ${syntheticSource}\n`,
  );
}
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
    catalogLimit,
    catalogTimeoutMs,
    catalogSessionsConsidered: sessions.length,
  },
  privateMessages: rawMessages,
};
const serialized = `${JSON.stringify(raw, null, 2)}\n`;
writeFileSync(outputPath, serialized, { mode: 0o600, flag: "wx" });
chmodSync(outputPath, 0o600);
process.stderr.write(`raw transcript written outside repository; sha256:${sha256(serialized)}\n`);
