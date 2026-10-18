#!/usr/bin/env node

import path from "node:path";
import { fileURLToPath } from "node:url";
import { stableReadFile } from "../../tools/scripts/lib/client-release-artifact-digest.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));

function requireValue(condition, code) {
  if (!condition) throw new Error(code);
}

const source = stableReadFile(
  path.join(repoRoot, "tests/smoke/native-client-smoke.mjs"),
  { maxBytes: 512 * 1024 },
).toString("utf8");
const packageJson = JSON.parse(stableReadFile(
  path.join(repoRoot, "package.json"),
  { maxBytes: 2 * 1024 * 1024 },
).toString("utf8"));

requireValue(source.includes('argumentsSet.has("--runtime-data")'),
  "native_smoke_runtime_data_authorization_missing");
requireValue(source.includes('"--include-accessible-environments"') &&
  source.includes('"--include-history-model-catalog"') &&
  source.includes("String(runtimeDataAuthorized)"),
"native_smoke_default_probe_policy_missing");
requireValue(!source.includes("result.stderr || result.stdout") &&
  !source.includes("result.stdout || result.stderr") &&
  !source.includes("args.join"),
"native_smoke_failure_exposes_runtime_output");
requireValue(packageJson.scripts?.["client:native:smoke"] ===
  "node tests/smoke/native-client-smoke.mjs",
"default_native_smoke_is_not_noninteractive");
requireValue(packageJson.scripts?.["client:native:smoke:runtime-data"] ===
  "node tests/smoke/native-client-smoke.mjs --runtime-data",
"authorized_runtime_data_smoke_command_missing");

console.log(JSON.stringify({
  ok: true,
  caseCount: 5,
  runtimeDataProbeInDefault: false,
  privatePathsIncluded: false,
}));
