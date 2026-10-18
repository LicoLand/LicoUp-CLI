import path from "node:path";
import { installedApp } from "./constants.mjs";
import { fail, requireSuccess, requireValue, run, wait } from "./util.mjs";

export function plistValue(appPath, key) {
  const result = run("/usr/libexec/PlistBuddy", [
    "-c",
    `Print :${key}`,
    path.join(appPath, "Contents/Info.plist"),
  ]);
  requireSuccess(result, "macos_bundle_plist_value_missing");
  return String(result.stdout || "").trim();
}

export function parseProcessRecords(executablePath) {
  const escapedExecutable = executablePath.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  const lookup = run("/usr/bin/pgrep", ["-f", `^${escapedExecutable}( |$)`], {
    timeout: 5_000,
  });
  if (lookup.status === 1) return [];
  requireSuccess(lookup, "macos_process_lookup_unavailable");
  const pids = [...new Set(String(lookup.stdout || "")
    .split(/\r?\n/u)
    .map((value) => Number(value.trim()))
    .filter((value) => Number.isInteger(value) && value > 0))]
    .slice(0, 64);
  const records = [];
  for (const pid of pids) {
    const result = run("/bin/ps", [
      "-ww",
      "-p",
      String(pid),
      "-o",
      "lstart=",
      "-o",
      "command=",
    ], { timeout: 5_000 });
    if (result.status !== 0) continue;
    const line = String(result.stdout || "").trim();
    const match = line.match(
      /^([A-Za-z]{3}\s+[A-Za-z]{3}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(.+)$/u,
    );
    if (!match) continue;
    const command = match[2];
    if (command !== executablePath && !command.startsWith(`${executablePath} `)) continue;
    const startedAtMs = Date.parse(match[1]);
    if (!Number.isFinite(startedAtMs)) continue;
    records.push({
      pid,
      startedAtMs,
      command,
    });
  }
  return records;
}

export function terminateExistingInstalledApp(executablePath) {
  const before = parseProcessRecords(executablePath);
  if (before.length === 0) return new Set();
  run("/usr/bin/osascript", [
    "-e",
    'tell application id "land.lico.licoup" to quit',
  ], { timeout: 5_000 });
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    if (parseProcessRecords(executablePath).length === 0) {
      return new Set(before.map((record) => record.pid));
    }
    wait(250);
  }
  fail("macos_previous_app_instance_did_not_terminate");
}

export function launchInstalledApp({
  executablePath,
  challenge,
  invocationNonce,
  closureStartedAtMs,
}) {
  const oldPids = terminateExistingInstalledApp(executablePath);
  const invocationStartedAtMs = Date.now();
  const open = run("/usr/bin/open", [
    "-n",
    "-g",
    installedApp,
    "--args",
    "--lico-release-closure-challenge",
    challenge,
    "--lico-release-invocation-nonce",
    invocationNonce,
  ]);
  requireSuccess(open, "macos_installed_app_launch_failed");
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const record = parseProcessRecords(executablePath).find((candidate) =>
      !oldPids.has(candidate.pid) &&
      candidate.startedAtMs >= invocationStartedAtMs - 5_000 &&
      candidate.startedAtMs >= closureStartedAtMs - 5_000 &&
      candidate.command.includes(`--lico-release-closure-challenge ${challenge}`) &&
      candidate.command.includes(`--lico-release-invocation-nonce ${invocationNonce}`)
    );
    if (record) {
      const stableUntil = Date.now() + 2_000;
      while (Date.now() < stableUntil) {
        wait(250);
        const stillRunning = parseProcessRecords(executablePath).find((candidate) =>
          candidate.pid === record.pid &&
          candidate.startedAtMs === record.startedAtMs &&
          candidate.command === record.command
        );
        requireValue(stillRunning, "macos_launched_process_not_stable");
      }
      return Object.freeze({
        newProcessReady: true,
        startedAfterInvocation: true,
        executableWithinInstalledBundle: true,
        closureChallengeBound: true,
        invocationNonceBound: true,
        stableProcessWindowReady: true,
      });
    }
    wait(250);
  }
  fail("macos_launched_process_binding_not_observed");
}
