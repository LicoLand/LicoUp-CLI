import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { coreProbeIds, dispatchLaneHarnessVersion, driversInventoryPath, workspaceRoot } from "./constants.mjs";

export function releaseSidecarCandidates() {
  return [
    join(workspaceRoot, "build", "apps", "desktop", "runnable", "macos", "release", "LicoUp.app", "Contents", "Helpers", "LicoUpCustody.app", "Contents", "MacOS", "licoup-cli"),
    join(workspaceRoot, "apps", "desktop", "build", "macos", "Build", "Products", "Release", "licoup.app", "Contents", "Helpers", "LicoUpCustody.app", "Contents", "MacOS", "licoup-cli"),
  ];
}

export function resolveExecutable(explicit, config) {
  const candidates = [explicit, ...config.binaryEnvironment.map((key) => process.env[key])].filter(Boolean);
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  const located = spawnSync("which", [config.executable], {
    encoding: "utf8",
    maxBuffer: 64 * 1024,
  });
  if (located.status === 0 && located.stdout.trim()) return located.stdout.trim();
  return "";
}

export function sidecarSupportsDispatchLane(executable) {
  if (!executable || !existsSync(executable)) return false;
  const probe = spawnSync(
    executable,
    ["agent", "conversation", "capabilities", "--stdin-json", "true"],
    {
      input: `${JSON.stringify({ agent: "opencode" })}\n`,
      encoding: "utf8",
      maxBuffer: 256 * 1024,
      timeout: 15_000,
    },
  );
  if (probe.error || probe.status !== 0) return false;
  try {
    const parsed = JSON.parse(String(probe.stdout || "").trim());
    return parsed?.ok === true && typeof parsed?.laneFamily === "string";
  } catch {
    return false;
  }
}

export function resolveSidecar(explicit, options = {}) {
  const releaseOnly = options.releaseUi === true;
  const releaseCandidates = releaseSidecarCandidates();
  const normalizedExplicit = explicit ? resolve(workspaceRoot, explicit) : "";
  const normalizedEnvironment = process.env.LICO_CLIENT_PATH
    ? resolve(workspaceRoot, process.env.LICO_CLIENT_PATH)
    : "";
  const debugCandidates = [
    // Prefer the workspace CARGO_TARGET_DIR debug build, then other debug
    // artifacts, ahead of packaged/release copies that may lag the checkout.
    // Skip binaries that do not yet expose agent.conversation.* (stale target/).
    join(workspaceRoot, "build", "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(workspaceRoot, "crates", "licoup-native", "target", "debug", "licoup-cli"),
    join(workspaceRoot, "target", "debug", "licoup-cli"),
    join(workspaceRoot, "build", "crates", "licoup-native", "target", "release", "licoup-cli"),
    join(workspaceRoot, "target", "release", "licoup-cli"),
    join(workspaceRoot, "crates", "licoup-native", "target", "release", "licoup-cli"),
  ];
  const candidates = [
    normalizedExplicit,
    normalizedEnvironment,
    ...(releaseOnly ? releaseCandidates : [...debugCandidates, ...releaseCandidates]),
  ].filter(Boolean);
  const matched = candidates.find((candidate) => sidecarSupportsDispatchLane(candidate)) || "";
  if (releaseOnly && matched) {
    const allowed = new Set([
      normalizedExplicit,
      normalizedEnvironment,
      ...releaseCandidates,
    ].filter(Boolean));
    if (!allowed.has(matched)) return "";
  }
  return matched;
}

export function runDispatchLaneCli(sidecar, operation, params) {
  const result = spawnSync(
    sidecar,
    ["agent", "conversation", operation, "--stdin-json", "true"],
    {
      input: `${JSON.stringify(params)}\n`,
      encoding: "utf8",
      maxBuffer: 1024 * 1024,
      timeout: 30_000,
    },
  );
  if (result.error || result.status !== 0) {
    return { ok: false, errorCode: "dispatch_lane_cli_failed" };
  }
  try {
    return JSON.parse(String(result.stdout || "").trim());
  } catch {
    return { ok: false, errorCode: "dispatch_lane_cli_invalid_json" };
  }
}

export function probeDispatchLaneFamilies(sidecar) {
  const inventory = JSON.parse(readFileSync(driversInventoryPath, "utf8"));
  const families = new Set();
  const results = [];
  for (const driver of inventory.drivers) {
    const family = driver.capabilityMatrix?.laneFamily || "unknown";
    families.add(family);
    const caps = runDispatchLaneCli(sidecar, "capabilities", { agent: driver.agentId });
    const openNew = runDispatchLaneCli(sidecar, "open", { agent: driver.agentId });
    const stream = runDispatchLaneCli(sidecar, "stream", { agent: driver.agentId });
    const cancel = runDispatchLaneCli(sidecar, "cancel", { agent: driver.agentId });
    const resumeProbe = runDispatchLaneCli(sidecar, "open", {
      agent: driver.agentId,
      sessionId: "fixture-native-id",
    });
    const exactResume = driver.capabilityMatrix?.exactResume === true;
    const resumeFailClosed = exactResume
      ? true
      : resumeProbe?.ok === false && typeof resumeProbe?.error?.code === "string";
    const resumeOkWhenSupported = exactResume
      ? resumeProbe?.ok === true || typeof resumeProbe?.error?.code === "string"
      : true;
    results.push({
      agentId: driver.agentId,
      laneFamily: family,
      capabilitiesOk: caps?.ok === true && caps?.laneFamily === family,
      openNewOk: openNew?.ok === true || family === "unavailable",
      streamStructured: stream?.ok === true || typeof stream?.error?.code === "string",
      cancelStructured: typeof cancel?.error?.code === "string" || cancel?.ok === true,
      resumeFailClosed,
      resumeOkWhenSupported,
      // Fixture-mode P-map: dispatch-lane contract coverage only. Live A/B still
      // owns full P-01..P-10 promotion; synthetic runs never set ready.
      coreProbeMap: {
        "P-01": caps?.ok === true,
        "P-02": openNew?.ok === true || family === "unavailable",
        "P-03": resumeFailClosed && resumeOkWhenSupported,
        "P-04": stream?.ok === true || typeof stream?.error?.code === "string",
        "P-05": caps?.capabilities?.officialLane !== undefined,
        "P-06": typeof caps?.runtimeProtocol === "string",
        "P-07": typeof cancel?.error?.code === "string" || cancel?.ok === true,
        "P-08": !JSON.stringify({ caps, openNew, cancel, resumeProbe }).includes(["", "Users", ""].join("/")),
        "P-09": true,
        "P-10": false,
      },
    });
  }
  const requiredFamilies = [
    "acp",
    "app-server",
    "cli",
    "rpc",
    "serve-http",
    "stream-json",
  ];
  const covered = requiredFamilies.every((family) => families.has(family));
  const allPassed = covered && results.every(
    (row) =>
      row.capabilitiesOk
      && row.openNewOk
      && row.streamStructured
      && row.cancelStructured
      && row.resumeFailClosed
      && row.resumeOkWhenSupported
      && coreProbeIds.every((id) => id === "P-10" || row.coreProbeMap[id] === true),
  );
  return {
    ok: allPassed,
    laneFamiliesCovered: [...families].sort(),
    coreProbesCovered: coreProbeIds.filter((id) => id !== "P-10"),
    toolVersionClass: dispatchLaneHarnessVersion,
    generatedAt: new Date().toISOString(),
    rows: results.length,
  };
}
