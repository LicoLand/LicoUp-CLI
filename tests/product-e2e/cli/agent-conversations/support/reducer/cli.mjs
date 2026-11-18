import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { READINESS_FILE, READINESS_SCHEMA_VERSION } from "./constants.mjs";
import { auditEvidenceDigests } from "./digest-audit.mjs";
import { ReducerError, fail } from "./errors.mjs";
import { maybeReloadGatewayInventory } from "./gateway-reload.mjs";
import { loadCanonicalInputs } from "./inputs.mjs";
import { canonicalJson, parseJson } from "./json.mjs";
import { reduceConversationParity } from "./reduce.mjs";

export function parseArguments(argv) {
  const options = { evidenceFile: null, write: false, check: false, requireReady: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--evidence") {
      const value = argv[index + 1];
      if (!value || value.startsWith("--")) fail("cli_arguments_invalid");
      options.evidenceFile = value;
      index += 1;
    } else if (argument === "--write") {
      options.write = true;
    } else if (argument === "--check") {
      options.check = true;
    } else if (argument === "--require-ready") {
      options.requireReady = true;
    } else {
      fail("cli_arguments_invalid");
    }
  }
  if (options.write && options.check) fail("cli_arguments_invalid");
  if (options.requireReady && !options.check) fail("cli_arguments_invalid");
  return options;
}

export function receipt(operation, result) {
  return {
    schemaVersion: "v0.0.1:client-agent-conversation-readiness-receipt-1",
    ok: true,
    operation,
    summary: result.summary,
  };
}

export function assertReadinessMatchesReduction(current, reduced) {
  if (canonicalJson(current) !== canonicalJson(reduced)) {
    fail("readiness_resource_mismatch");
  }
}

export function assertReleaseReady(result) {
  const adapters = Array.isArray(result?.adapters) ? result.adapters : [];
  const summary = result?.summary ?? {};
  const complete =
    adapters.length > 0 &&
    summary.total === adapters.length &&
    summary.ready === adapters.length &&
    summary.sendEnabled === adapters.length &&
    ["partial", "failed", "blocked", "unverified", "historyOnly"]
      .every((field) => summary[field] === 0) &&
    adapters.every(
      (adapter) => adapter?.status === "ready" && adapter?.sendEnabled === true,
    );
  if (!complete) fail("release_readiness_incomplete");
}

export function runCli(argv = process.argv.slice(2)) {
  const options = parseArguments(argv);
  const canonical = loadCanonicalInputs();
  const evidence = options.evidenceFile
    ? parseJson(readFileSync(resolve(options.evidenceFile), "utf8"), "evidence_json_invalid")
    : canonical.evidence;
  const result = reduceConversationParity({
    packagingRegistry: canonical.packagingRegistry,
    inventory: canonical.inventory,
    evidence,
  });

  if (options.write) {
    // Reconcile the record before it becomes the readiness the runtime embeds.
    // A record whose digests do not describe what they claim can never be
    // promoted, and `--write` can never launder one.
    auditEvidenceDigests({
      evidence,
      packagingRegistry: canonical.packagingRegistry,
      inventory: canonical.inventory,
      readiness: result,
    });
    const nextText = `${JSON.stringify(result, null, 2)}\n`;
    let previousText = null;
    if (existsSync(READINESS_FILE)) {
      previousText = readFileSync(READINESS_FILE, "utf8");
    }
    writeFileSync(READINESS_FILE, nextText, { mode: 0o600 });
    const receiptBody = receipt("write", result);
    if (previousText !== nextText) {
      receiptBody.gatewayReload = maybeReloadGatewayInventory(nextText);
    } else {
      receiptBody.gatewayReload = { attempted: false, reason: "unchanged" };
    }
    return receiptBody;
  }
  if (options.check) {
    const current = parseJson(
      readFileSync(READINESS_FILE, "utf8"),
      "readiness_resource_invalid",
    );
    assertReadinessMatchesReduction(current, result);
    // The reduction is blind to a digest that cannot change any status, so the
    // recorded digests are reconciled directly against the frozen projection.
    auditEvidenceDigests({
      evidence,
      packagingRegistry: canonical.packagingRegistry,
      inventory: canonical.inventory,
      readiness: current,
    });
    if (options.requireReady) assertReleaseReady(result);
    return receipt("check", result);
  }
  return result;
}

export function sanitizedFailure(error) {
  return {
    schemaVersion: "v0.0.1:client-agent-conversation-readiness-receipt-1",
    ok: false,
    errorCode: error instanceof ReducerError ? error.code : "reducer_error",
  };
}
