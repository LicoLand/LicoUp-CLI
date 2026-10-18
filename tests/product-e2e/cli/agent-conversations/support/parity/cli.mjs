import { resolve } from "node:path";
import { normalizeAgentId } from "./agent-ids.mjs";
import { defaultMaxOutputBytes, defaultTimeoutMs } from "./constants.mjs";
import { AcceptanceError, requireFact } from "./errors.mjs";

export function parseArguments(argv) {
  const parsed = {
    agent: "",
    strict: false,
    selfTest: false,
    releaseUi: false,
    printLiveGate: false,
    binary: "",
    sidecar: "",
    productReceipt: "",
    cleanupProductSession: "",
    timeoutMs: Number(process.env.LICO_ACP_PARITY_TIMEOUT_MS || defaultTimeoutMs),
    maxOutputBytes: Number(process.env.LICO_ACP_PARITY_MAX_OUTPUT_BYTES || defaultMaxOutputBytes),
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--strict") {
      parsed.strict = true;
    } else if (argument === "--self-test") {
      parsed.selfTest = true;
    } else if (argument === "--release-ui") {
      parsed.releaseUi = true;
    } else if (argument === "--print-live-gate") {
      parsed.printLiveGate = true;
    } else if (["--agent", "--binary", "--sidecar", "--product-receipt", "--cleanup-product-session", "--timeout-ms", "--max-output-bytes"].includes(argument)) {
      const value = argv[index + 1];
      requireFact(typeof value === "string" && value.length > 0, "cli_argument_missing");
      index += 1;
      if (argument === "--agent") parsed.agent = normalizeAgentId(value);
      if (argument === "--binary") parsed.binary = value;
      if (argument === "--sidecar") parsed.sidecar = value;
      if (argument === "--product-receipt") parsed.productReceipt = resolve(value);
      if (argument === "--cleanup-product-session") parsed.cleanupProductSession = resolve(value);
      if (argument === "--timeout-ms") parsed.timeoutMs = Number(value);
      if (argument === "--max-output-bytes") parsed.maxOutputBytes = Number(value);
    } else {
      throw new AcceptanceError("cli_argument_unsupported");
    }
  }
  requireFact(Number.isFinite(parsed.timeoutMs) && parsed.timeoutMs >= 1_000, "timeout_invalid");
  requireFact(
    Number.isSafeInteger(parsed.maxOutputBytes)
      && parsed.maxOutputBytes >= 4 * 1024
      && parsed.maxOutputBytes <= 16 * 1024 * 1024,
    "output_limit_invalid",
  );
  if (parsed.releaseUi) {
    requireFact(parsed.strict, "release_ui_requires_strict");
    requireFact(parsed.productReceipt.length > 0, "release_ui_product_receipt_required");
  }
  if (!parsed.selfTest && !parsed.printLiveGate) {
    requireFact(parsed.agent.length > 0, "agent_required");
  }
  return parsed;
}
