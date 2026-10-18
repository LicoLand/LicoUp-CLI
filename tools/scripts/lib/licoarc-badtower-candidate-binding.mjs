import {
  accessSync,
  chmodSync,
  constants as fsConstants,
  mkdirSync,
  mkdtempSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import {
  sha256Buffer,
  stableHashFileSnapshot,
  stableReadFileSnapshot,
} from "./client-release-artifact-digest.mjs";
import {
  validateLicoArcV1BundleBytes,
} from "./licoarc-v1-bundle-validator.mjs";

export const LICOARC_BADTOWER_CANDIDATE_BINDING_KEY =
  "licoArcBadTowerCandidates";

const DIGEST_PATTERN = /^sha256:[a-f0-9]{64}$/u;

export function captureLicoArcBadTowerCandidateBinding({
  clientCandidateDigest,
  environment = process.env,
  platform = process.platform,
} = {}) {
  requireValue(DIGEST_PATTERN.test(String(clientCandidateDigest || "")),
    "current client candidate digest is invalid");
  const stationPath = requiredCandidatePath(
    environment,
    "LICOUP_ACCEPTANCE_BADTOWER_BINARY",
    { executable: true, platform },
  );
  const protocolPath = requiredCandidatePath(
    environment,
    "LICOUP_ACCEPTANCE_LICOARC_BUNDLE",
    { platform },
  );
  const protocolRead = stableReadFileSnapshot(protocolPath, {
    maxBytes: 128 * 1024 * 1024,
  });
  validateLicoArcV1BundleBytes(protocolRead.bytes);
  const station = stableHashFileSnapshot(stationPath, {
    maxBytes: 512 * 1024 * 1024,
  });
  const protocol = Object.freeze({
    digest: sha256Buffer(protocolRead.bytes),
    size: protocolRead.size,
    mtimeMs: protocolRead.mtimeMs,
    device: protocolRead.device,
    inode: protocolRead.inode,
  });
  return Object.freeze({
    bindings: Object.freeze({
      clientCandidateDigest,
      protocolCandidateDigest: protocol.digest,
      stationCandidateDigest: station.digest,
    }),
    station,
    protocol,
  });
}

export function licoArcBadTowerCandidateSnapshotsMatch(before, after) {
  return candidateSnapshotMatches(before?.station, after?.station) &&
    candidateSnapshotMatches(before?.protocol, after?.protocol) &&
    JSON.stringify(before?.bindings) === JSON.stringify(after?.bindings);
}

export function createBadTowerStationEnvironment(
  sourceEnvironment = process.env,
  platform = process.platform,
) {
  if (platform !== "win32") return Object.freeze({});
  const sourceByUppercaseName = new Map(
    Object.entries(sourceEnvironment).map(([name, value]) =>
      [name.toUpperCase(), String(value)]),
  );
  const environment = {};
  for (const name of ["SYSTEMROOT", "WINDIR"]) {
    const value = sourceByUppercaseName.get(name);
    if (value) environment[name] = value;
  }
  return Object.freeze(environment);
}

export function runBadTowerStationEnvironmentSelfTest() {
  const ambient = {
    PATH: "ambient-path",
    HOME: "ambient-home",
    UNRELATED_PRIVATE_INPUT: "fixture-value",
    LICOUP_ACCEPTANCE_PRIVATE_CANARY: "must-not-pass",
    UNRELATED_CHANNEL_INPUT: "fixture-value",
    SystemRoot: "system-root",
    WINDIR: "windows-root",
  };
  const posix = createBadTowerStationEnvironment(ambient, "linux");
  const windows = createBadTowerStationEnvironment(ambient, "win32");
  const posixMinimal = Object.keys(posix).length === 0;
  const windowsMinimal =
    JSON.stringify(Object.keys(windows).sort()) ===
      JSON.stringify(["SYSTEMROOT", "WINDIR"]) &&
    windows.SYSTEMROOT === "system-root" &&
    windows.WINDIR === "windows-root";
  const ambientSecretsRejected = [
    "PATH",
    "HOME",
    "BADTOWER_SECRET",
    "LICOUP_ACCEPTANCE_PRIVATE_CANARY",
    "TOKEN",
  ].every((name) =>
    !Object.hasOwn(posix, name) && !Object.hasOwn(windows, name));
  return Object.freeze({
    ok: posixMinimal && windowsMinimal && ambientSecretsRejected,
    posixMinimal,
    windowsMinimal,
    ambientSecretsRejected,
  });
}

export function runLicoArcBadTowerCandidatePathSelfTest() {
  const fixtureRoot = mkdtempSync(path.join(
    os.tmpdir(),
    "licoup-candidate-path-self-test-",
  ));
  try {
    const candidatePath = path.join(fixtureRoot, "candidate");
    const directoryPath = path.join(fixtureRoot, "directory");
    writeFileSync(candidatePath, "candidate", { mode: 0o700 });
    mkdirSync(directoryPath, { mode: 0o700 });
    const environment = { CANDIDATE: candidatePath };
    const regularFileAccepted =
      requiredCandidatePath(environment, "CANDIDATE", {
        executable: true,
        platform: process.platform,
      }) === candidatePath;
    const directoryRejected = rejectsCandidatePath(
      { CANDIDATE: directoryPath },
      { executable: false, platform: process.platform },
    );
    let nonExecutableRejected = true;
    if (process.platform !== "win32") {
      chmodSync(candidatePath, 0o600);
      nonExecutableRejected = rejectsCandidatePath(
        environment,
        { executable: true, platform: process.platform },
      );
    }
    const crossPlatformAccessModes =
      candidateAccessMode(true, "linux") === fsConstants.X_OK &&
      candidateAccessMode(true, "darwin") === fsConstants.X_OK &&
      candidateAccessMode(true, "win32") === fsConstants.F_OK &&
      candidateAccessMode(false, "linux") === fsConstants.F_OK;
    return Object.freeze({
      ok: regularFileAccepted &&
        directoryRejected &&
        nonExecutableRejected &&
        crossPlatformAccessModes,
      regularFileAccepted,
      directoryRejected,
      nonExecutableRejected,
      crossPlatformAccessModes,
    });
  } finally {
    rmSync(fixtureRoot, { recursive: true, force: true });
  }
}

function requiredCandidatePath(
  environment,
  name,
  { executable = false, platform = process.platform } = {},
) {
  const value = String(environment?.[name] || "").trim();
  requireValue(value && path.isAbsolute(value),
    `${name} must be an absolute path`);
  const resolved = path.resolve(value);
  requireValue(statSync(resolved).isFile(),
    `${name} must be a regular file`);
  const mode = candidateAccessMode(executable, platform);
  try {
    accessSync(resolved, mode);
  } catch {
    throw new Error(`${name} is not an accessible candidate file`);
  }
  return resolved;
}

function candidateAccessMode(executable, platform) {
  return executable && platform !== "win32"
    ? fsConstants.X_OK
    : fsConstants.F_OK;
}

function rejectsCandidatePath(environment, options) {
  try {
    requiredCandidatePath(environment, "CANDIDATE", options);
    return false;
  } catch {
    return true;
  }
}

function candidateSnapshotMatches(left, right) {
  return left?.digest === right?.digest &&
    left?.size === right?.size &&
    left?.device === right?.device &&
    left?.inode === right?.inode;
}

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}
