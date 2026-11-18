import { randomBytes } from "node:crypto";
import {
  closeSync,
  constants,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  writeSync,
} from "node:fs";
import path from "node:path";

import { MigrationStateError } from "./errors.mjs";

export const MAX_MIGRATION_JSON_BYTES = 4 * 1024 * 1024;

export function requireValue(condition, code) {
  if (!condition) throw new MigrationStateError(code);
}

export function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isNonEmptyText(value) {
  return typeof value === "string" && value.length > 0;
}

/**
 * Serde's `as_u64`: a float, a string, or a negative number is not a version.
 * One input class cannot be mirrored — `2.0` and `1e2` are F64 to serde and
 * plain integers here, so a document that spells its version that way reads as
 * current instead of unreadable. No LicoUp writer emits one.
 */
export function asU64(value) {
  return Number.isInteger(value) && value >= 0 ? value : undefined;
}

export function asText(value) {
  return typeof value === "string" ? value : undefined;
}

/**
 * The admission reads its own migration metadata through
 * `read_existing_private_text_bounded`, which on unix requires the parent
 * directory to be exactly 0700 and the file exactly 0600, both owned by the
 * effective user. A root copied without its modes is rejected there, so this
 * tool has to reject it too: reporting it healthy would be a false green.
 */
function requirePrivateMetadata(metadata, kind, code) {
  const owned =
    typeof process.getuid !== "function" || metadata.uid === process.getuid();
  const mode = metadata.mode & 0o777;
  requireValue(owned, code);
  if (process.platform !== "win32") {
    requireValue(mode === (kind === "directory" ? 0o700 : 0o600), code);
  }
}

/**
 * A user-controlled symlink anywhere above private state is how a foreign path
 * would be substituted beneath it, so the admission refuses one. System-owned
 * links are allowed, which keeps `/tmp` and `/var` usable on macOS.
 */
function requireNoUserOwnedSymlinkAncestors(pathname, code) {
  if (process.platform === "win32") return;
  const absolute = path.resolve(pathname);
  const root = path.parse(absolute).root;
  let current = root;
  for (const segment of absolute.slice(root.length).split(path.sep)) {
    if (segment === "") continue;
    current = path.join(current, segment);
    let metadata;
    try {
      metadata = lstatSync(current);
    } catch (error) {
      // A missing component ends the walk, exactly as the admission stops there.
      if (error.code === "ENOENT") return;
      throw new MigrationStateError(code);
    }
    if (metadata.isSymbolicLink() && metadata.uid !== 0) {
      throw new MigrationStateError(code);
    }
  }
}

/** Returns whether the path exists, refusing anything but its private shape. */
export function requirePrivateStatePath(pathname, kind, code) {
  const metadata = metadataOrNull(pathname);
  if (metadata === null) {
    // Ancestors are only validated once the tree exists; otherwise this is
    // simply absent state rather than a broken one.
    if (metadataOrNull(path.dirname(pathname)) === null) return false;
    requireNoUserOwnedSymlinkAncestors(path.dirname(pathname), code);
    return false;
  }
  requireNoUserOwnedSymlinkAncestors(pathname, code);
  if (kind === "directory") {
    requireValue(metadata.isDirectory() && !metadata.isSymbolicLink(), code);
  } else {
    requireValue(metadata.isFile() && !metadata.isSymbolicLink(), code);
  }
  requirePrivateMetadata(metadata, kind, code);
  return true;
}

function metadataOrNull(pathname) {
  try {
    return lstatSync(pathname);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw new MigrationStateError("unsupported_state_shape");
  }
}

/**
 * Mirrors the admission's view of a durable artifact: a missing path is absent
 * state, and anything that is neither absent nor a plain regular file (symlink,
 * directory, device) is a shape this tool refuses to interpret.
 */
export function regularFileExists(pathname) {
  const metadata = metadataOrNull(pathname);
  if (metadata === null) return false;
  requireValue(metadata.isFile() && !metadata.isSymbolicLink(), "unsupported_state_shape");
  return true;
}

export function readBoundedText(pathname, maxBytes = MAX_MIGRATION_JSON_BYTES) {
  if (!regularFileExists(pathname)) return null;
  const metadata = lstatSync(pathname);
  requireValue(metadata.size > 0 && metadata.size <= maxBytes, "unsupported_state_shape");
  return readFileSync(pathname, "utf8");
}

export function readJsonArtifact(pathname, maxBytes = MAX_MIGRATION_JSON_BYTES) {
  const raw = readBoundedText(pathname, maxBytes);
  if (raw === null) return null;
  try {
    return JSON.parse(raw);
  } catch {
    throw new MigrationStateError("unsupported_state_shape");
  }
}

export function ensurePrivateDirectory(pathname) {
  mkdirSync(pathname, { recursive: true, mode: 0o700 });
  const metadata = lstatSync(pathname);
  requireValue(metadata.isDirectory() && !metadata.isSymbolicLink(), "unsupported_state_shape");
}

/**
 * Commit `value` as compact JSON, the byte shape the admission writes. The
 * temporary file is created exclusively (so a concurrent writer cannot be
 * followed) and the commit is a rename, so no reader ever observes a partial
 * document.
 */
export function writePrivateJsonAtomic(pathname, value, { expectedBytes = undefined } = {}) {
  const serialized = `${JSON.stringify(value)}\n`;
  requireValue(
    Buffer.byteLength(serialized, "utf8") <= MAX_MIGRATION_JSON_BYTES,
    "migration_step_failed",
  );
  const directory = path.dirname(pathname);
  const directoryMetadata = metadataOrNull(directory);
  requireValue(
    directoryMetadata !== null &&
      directoryMetadata.isDirectory() &&
      !directoryMetadata.isSymbolicLink(),
    "unsupported_state_shape",
  );
  const existing = metadataOrNull(pathname);
  requireValue(
    existing === null || (existing.isFile() && !existing.isSymbolicLink()),
    "unsupported_state_shape",
  );
  // Compare-and-swap: a caller that read the document first can require it to
  // be byte-identical immediately before the commit. Another writer wins the
  // race instead of being overwritten with a stale document.
  if (expectedBytes !== undefined) {
    const current = existing === null ? null : readFileSync(pathname, "utf8");
    requireValue(current === expectedBytes, "repair_conflict");
  }
  const temporary = path.join(
    directory,
    `.${path.basename(pathname)}.${randomBytes(8).toString("hex")}.tmp`,
  );
  const handle = openSync(
    temporary,
    constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY | constants.O_NOFOLLOW,
    0o600,
  );
  try {
    writeSync(handle, serialized);
    fsyncSync(handle);
  } finally {
    closeSync(handle);
  }
  try {
    renameSync(temporary, pathname);
  } catch {
    rmSync(temporary, { force: true });
    throw new MigrationStateError("migration_step_failed");
  }
  const directoryHandle = openSync(directory, constants.O_RDONLY);
  try {
    fsyncSync(directoryHandle);
  } finally {
    closeSync(directoryHandle);
  }
}

/**
 * Product versions are compared the way the admission compares them with
 * `semver`: numeric core first, and a prerelease ranks below its release.
 */
export function compareProductVersion(left, right) {
  const a = parseProductVersion(left);
  const b = parseProductVersion(right);
  for (let index = 0; index < 3; index += 1) {
    if (a.core[index] !== b.core[index]) return Math.sign(a.core[index] - b.core[index]);
  }
  if (a.pre.length === 0 || b.pre.length === 0) {
    if (a.pre.length === b.pre.length) return 0;
    return a.pre.length === 0 ? 1 : -1;
  }
  for (let index = 0; index < Math.max(a.pre.length, b.pre.length); index += 1) {
    const x = a.pre[index];
    const y = b.pre[index];
    if (x === undefined || y === undefined) return x === undefined ? -1 : 1;
    if (x === y) continue;
    const xn = /^\d+$/u.test(x);
    const yn = /^\d+$/u.test(y);
    if (xn && yn) return Math.sign(Number(x) - Number(y));
    if (xn !== yn) return xn ? -1 : 1;
    return x < y ? -1 : 1;
  }
  return 0;
}

export function parseProductVersion(value) {
  const match = String(value).match(
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:[0-9A-Za-z-]+\.)*[0-9A-Za-z-]+))?(?:\+[0-9A-Za-z.-]+)?$/u,
  );
  if (!match) throw new MigrationStateError("migration_ledger_invalid");
  return { core: match.slice(1, 4).map(Number), pre: match[4]?.split(".") ?? [] };
}
