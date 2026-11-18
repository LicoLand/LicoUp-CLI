import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const LOCK_NAMES = new Set([".cargo-lock", ".cargo-build-lock"]);
const helperPath = fileURLToPath(new URL("./cargo-lock-fence.py", import.meta.url));
const policy = JSON.parse(readFileSync(
  fileURLToPath(new URL("./cargo-lock-policy.json", import.meta.url)),
  "utf8",
));
const COMPILER_DIRECTORIES = new Set(policy.compilerDirectories);
const COMPILER_FILES = new Set(policy.compilerFiles);
const COMPILER_SUFFIXES = new Set(policy.compilerSuffixes);
let supported;

export function isCargoProfileEntryReclaimable({ name, type, executable = false }) {
  if (type === "directory") return COMPILER_DIRECTORIES.has(name);
  if (type !== "file" || LOCK_NAMES.has(name)) return false;
  return COMPILER_FILES.has(name) || COMPILER_SUFFIXES.has(path.extname(name).toLowerCase()) ||
    (executable === true && path.extname(name) === "");
}

function directories(directory) {
  try {
    return readdirSync(directory, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && !entry.isSymbolicLink())
      .map((entry) => path.join(directory, entry.name));
  } catch {
    return [];
  }
}

function locksIn(directory) {
  try {
    return readdirSync(directory, { withFileTypes: true })
      .filter((entry) => entry.isFile() && LOCK_NAMES.has(entry.name))
      .map((entry) => entry.name)
      .sort();
  } catch {
    return [];
  }
}

export function cargoProfileLocks(targetRoot) {
  try {
    if (!existsSync(targetRoot) || lstatSync(targetRoot).isSymbolicLink()) return [];
  } catch {
    return [];
  }
  const profiles = [];
  for (const first of directories(targetRoot)) {
    const directLocks = locksIn(first);
    if (directLocks.length > 0) {
      profiles.push({ relativeProfile: path.relative(targetRoot, first), lockNames: directLocks });
      continue;
    }
    for (const second of directories(first)) {
      const nestedLocks = locksIn(second);
      if (nestedLocks.length > 0) {
        profiles.push({ relativeProfile: path.relative(targetRoot, second), lockNames: nestedLocks });
      }
    }
  }
  return profiles.sort((left, right) =>
    left.relativeProfile.localeCompare(right.relativeProfile));
}

export function cargoLockFenceSupported() {
  if (supported !== undefined) return supported;
  if (process.platform !== "darwin") {
    supported = false;
    return supported;
  }
  const result = spawnSync("python3", ["-c", "import fcntl"], {
    encoding: "utf8",
    windowsHide: true,
    stdio: ["ignore", "ignore", "ignore"],
  });
  supported = !result.error && result.status === 0;
  return supported;
}

export function clearCargoProfiles({ repoRoot, targetRoot, profiles }) {
  if (process.platform !== "darwin") {
    return { status: "lock-unavailable" };
  }
  const result = spawnSync("python3", [helperPath], {
    encoding: "utf8",
    input: JSON.stringify({ profiles, repoRoot, targetRoot }),
    maxBuffer: 1024 * 1024,
    windowsHide: true,
  });
  if (result.error || result.status !== 0) return { status: "lock-unavailable" };
  try {
    const response = JSON.parse(result.stdout);
    return ["removed", "empty", "active-build", "running-binary", "probe-unavailable", "lock-unavailable"]
      .includes(response.status)
      ? response
      : { status: "lock-unavailable" };
  } catch {
    return { status: "lock-unavailable" };
  }
}
