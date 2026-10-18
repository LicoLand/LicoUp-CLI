import path from "node:path";
import process from "node:process";

import { sanitizeError } from "../lib/sanitize-error.mjs";
import { MigrationStateError } from "./errors.mjs";
import { REPO_ROOT, loadEmbeddedFrontier } from "./frontier.mjs";
import {
  REPORT_SCHEMA,
  USAGE_EXIT_CODE,
  evaluateMigrationState,
  exitCodeForVerdict,
  humanDoctor,
  humanReport,
  migrationEnvelope,
} from "./report.mjs";
import { repairDomain } from "./repair.mjs";
import { parseProductVersion, readJsonArtifact, requireValue } from "./util.mjs";

export const COMMANDS = Object.freeze(["status", "doctor", "repair"]);
const PRODUCT_VERSION_REF = "tools/client-version.json";

const USAGE = `usage: client-state-migration <status|doctor|repair> --root <data-root> [options]

  status   report every domain's ledger, marker and durable store version; read-only
  doctor   validate the ledger against the embedded frontier; fails closed
  repair   apply one frontier step to one domain, then reconcile its marker and
           ledger entry

options
  --root <data-root>      absolute client data root; required, and never guessed
  --domain <id>           the single domain repair acts on; required by repair
  --binary-version <ver>  product version the ledger is compared against
                          (default: the repository's client product version)
  --json                  print the versioned JSON envelope instead of the report
  --help                  print this text

exit codes
  0 healthy   2 behind   3 ahead   4 invalid   64 usage
`;

export function parseArgs(argv) {
  const options = {
    command: COMMANDS.includes(argv[0]) ? argv[0] : null,
    help: argv[0] === "--help" || argv[0] === "help",
    root: null,
    domain: null,
    binaryVersion: null,
    json: false,
  };
  if (options.help) return Object.freeze(options);
  requireValue(options.command !== null, "migration_arguments_invalid");
  for (let index = 1; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = argv[index + 1];
    if (arg === "--json") {
      options.json = true;
    } else if (arg === "--root" && next !== undefined) {
      options.root = next;
      index += 1;
    } else if (arg.startsWith("--root=")) {
      options.root = arg.slice("--root=".length);
    } else if (arg === "--domain" && next !== undefined) {
      options.domain = next;
      index += 1;
    } else if (arg.startsWith("--domain=")) {
      options.domain = arg.slice("--domain=".length);
    } else if (arg === "--binary-version" && next !== undefined) {
      options.binaryVersion = next;
      index += 1;
    } else if (arg.startsWith("--binary-version=")) {
      options.binaryVersion = arg.slice("--binary-version=".length);
    } else {
      throw new MigrationStateError("migration_arguments_invalid");
    }
  }
  requireValue(options.root !== null && path.isAbsolute(options.root), "migration_arguments_invalid");
  requireValue(
    options.command !== "repair" || (options.domain !== null && options.domain.length > 0),
    "migration_arguments_invalid",
  );
  requireValue(
    options.command === "repair" || options.domain === null,
    "migration_arguments_invalid",
  );
  if (options.binaryVersion !== null) parseProductVersion(options.binaryVersion);
  return Object.freeze(options);
}

export function loadBinaryProductVersion() {
  const document = readJsonArtifact(path.join(REPO_ROOT, PRODUCT_VERSION_REF));
  requireValue(
    typeof document?.productVersion === "string",
    "migration_arguments_invalid",
  );
  parseProductVersion(document.productVersion);
  return document.productVersion;
}

export function runClientStateMigrationCli(argv = process.argv.slice(2)) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    return emitFailure({ command: argv[0] ?? null, error, json: argv.includes("--json") });
  }
  if (options.help) {
    process.stdout.write(USAGE);
    return undefined;
  }
  try {
    const frontier = loadEmbeddedFrontier();
    const binaryProductVersion = options.binaryVersion ?? loadBinaryProductVersion();
    const input = {
      root: options.root,
      frontier,
      binaryProductVersion,
      platform: process.platform,
    };
    const outcome =
      options.command === "repair"
        ? repairDomain({ ...input, domainId: options.domain })
        : { report: evaluateMigrationState(input), mutations: [] };
    const envelope = migrationEnvelope({
      command: options.command,
      report: outcome.report,
      mutations: outcome.mutations,
    });
    const human =
      options.command === "doctor"
        ? humanDoctor(outcome.report)
        : humanReport({
            command: options.command,
            report: outcome.report,
            mutations: outcome.mutations,
          });
    process.stdout.write(options.json ? `${JSON.stringify(envelope, null, 2)}\n` : human);
    process.exitCode = exitCodeForVerdict(outcome.report.verdict);
    return undefined;
  } catch (error) {
    return emitFailure({ command: options.command, error, json: options.json });
  }
}

/**
 * A refusal is reported with the same versioned envelope, so a caller reads one
 * contract whether the tool certified a state or declined to.
 */
function emitFailure({ command, error, json }) {
  const code = error instanceof MigrationStateError ? error.code : "migration_tool_failed";
  const verdict = code === "state_newer_than_binary" ? "ahead" : "invalid";
  const exitCode =
    code === "migration_arguments_invalid" ? USAGE_EXIT_CODE : exitCodeForVerdict(verdict);
  const envelope = {
    schemaVersion: REPORT_SCHEMA,
    command: command ?? "unknown",
    verdict,
    exitCode,
    codes: [{ code, domainId: null }],
  };
  if (Array.isArray(error?.mutations) && error.mutations.length > 0) {
    // A refused repair that already applied its idempotent store step says so.
    envelope.mutations = error.mutations.map((mutation) => ({ ...mutation }));
  }
  if (code === "migration_tool_failed") envelope.error = sanitizeError(error);
  if (json) {
    process.stdout.write(`${JSON.stringify(envelope, null, 2)}\n`);
  } else if (code === "migration_arguments_invalid") {
    process.stderr.write(`${USAGE}\n`);
  } else {
    process.stderr.write(
      `client-state-migration ${command ?? ""}: ${code}`.trimEnd() + "\n",
    );
  }
  process.exitCode = exitCode;
  return exitCode;
}
