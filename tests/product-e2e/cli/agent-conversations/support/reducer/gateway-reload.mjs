import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const moduleRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(moduleRoot, "../../../../../..");

function resolveLicoupCli() {
  const candidates = [
    process.env.LICO_CLIENT_PATH,
    "/Applications/LicoUp.app/Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli",
    join(repoRoot, "build", "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(repoRoot, "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(repoRoot, "target", "debug", "licoup-cli"),
    join(repoRoot, "build", "crates", "licoup-native", "target", "release", "licoup-cli"),
    join(repoRoot, "target", "release", "licoup-cli"),
    join(
      repoRoot,
      "build",
      "apps",
      "desktop",
      "runnable",
      "macos",
      "release",
      "LicoUp.app",
      "Contents",
      "MacOS",
      "licoup-cli",
    ),
  ].filter(Boolean);
  return candidates.find((path) => existsSync(path)) || "";
}

/**
 * Best-effort partial hot-reload of verified readiness into a running Gateway.
 * Admits newly ready agents; does not restart the process or clear in-use
 * sessions. Never fails the reducer write: missing CLI or stopped gateway are
 * skipped.
 */
export function maybeReloadGatewayInventory(readinessJson) {
  const cli = resolveLicoupCli();
  if (!cli) {
    return { attempted: false, reason: "cli_missing" };
  }
  const result = spawnSync(
    cli,
    ["gateway", "inventory", "reload", "--stdin-json", "true"],
    {
      cwd: repoRoot,
      env: process.env,
      input: readinessJson,
      encoding: "utf8",
      timeout: 30_000,
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  if (result.status !== 0) {
    return {
      attempted: true,
      ok: false,
      reason: "gateway_inventory_reload_failed",
      exitCode: result.status,
    };
  }
  let report = null;
  try {
    report = JSON.parse(String(result.stdout || "").trim() || "null");
  } catch {
    report = null;
  }
  return {
    attempted: true,
    ok: report?.ok === true,
    mode: report?.mode ?? null,
  };
}
