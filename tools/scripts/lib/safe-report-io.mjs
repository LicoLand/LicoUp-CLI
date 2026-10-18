import { randomBytes } from "node:crypto";
import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import {
  resolveContainedExistingPath,
  stableHashFileSnapshot,
  stableReadFile,
  stableSnapshotFile,
} from "./client-release-artifact-digest.mjs";

export const SAFE_REPORT_WRITE_STAGES = Object.freeze([
  "parent_open_validate",
  "temp_create",
  "temp_write",
  "temp_fsync",
  "temp_validate",
  "before_publish",
  "parent_revalidate",
  "target_validate",
  "rename",
  "published_validate",
  "directory_fsync",
  "cleanup",
]);
export const DEFAULT_MAX_REPORT_JSON_BYTES = 8 * 1024 * 1024;
const SAFE_REPORT_WRITE_STAGE_SET = new Set(SAFE_REPORT_WRITE_STAGES);

export class SafeReportWriteError extends Error {
  constructor(stage) {
    super("Safe report write failed");
    this.name = "SafeReportWriteError";
    this.stage = stage;
  }
}

function runWriteStage(stage, faultInjector, operation) {
  if (!SAFE_REPORT_WRITE_STAGE_SET.has(stage)) {
    throw new SafeReportWriteError("parent_open_validate");
  }
  try {
    if (typeof faultInjector === "function") faultInjector(stage);
    return operation();
  } catch (error) {
    if (error instanceof SafeReportWriteError) throw error;
    throw new SafeReportWriteError(stage);
  }
}

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function inodeIdentity(info) {
  return `${String(info.dev)}:${String(info.ino)}`;
}

function stableEntryIdentity(info) {
  return [
    info.dev,
    info.ino,
    info.mode,
    info.nlink,
    info.uid,
    info.gid,
    info.size,
    info.mtimeNs,
    info.ctimeNs,
  ].map(String).join(":");
}

function publishedFileIdentity(info) {
  return [
    info.dev,
    info.ino,
    info.mode,
    info.nlink,
    info.uid,
    info.gid,
    info.size,
  ].map(String).join(":");
}

function ownedWithoutSharedWrite(info, expectedKind) {
  const kindReady = expectedKind === "directory" ? info.isDirectory() : info.isFile();
  if (!kindReady || info.isSymbolicLink()) return false;
  if (process.platform === "win32") return true;
  const ownerReady = typeof process.geteuid !== "function" ||
    info.uid === BigInt(process.geteuid());
  const sharedWriteAbsent = (info.mode & 0o022n) === 0n;
  return ownerReady && sharedWriteAbsent;
}

function ensureSafeDirectoryTree(rootPath, directoryPath) {
  const root = path.resolve(rootPath);
  const directory = path.resolve(directoryPath);
  requireValue(isWithin(root, directory), "report path escapes its allowed root");
  if (!existsSync(root)) mkdirSync(root, { mode: 0o700 });
  const rootInfo = lstatSync(root, { bigint: true });
  requireValue(ownedWithoutSharedWrite(rootInfo, "directory"),
    "report root is not a safe directory");
  let current = root;
  for (const component of path.relative(root, directory).split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    if (!existsSync(current)) mkdirSync(current, { mode: 0o700 });
    const info = lstatSync(current, { bigint: true });
    requireValue(ownedWithoutSharedWrite(info, "directory"),
      "report directory traverses a symbolic link");
  }
  return { root, directory };
}

export function resolveSafeReportPath(allowedRoot, reportRef) {
  const ref = String(reportRef || "").trim();
  requireValue(ref && !path.isAbsolute(ref), "report reference must be relative");
  requireValue(!ref.includes("\\") && !ref.includes("\0") &&
    ref.split("/").every((component) => component && component !== "." && component !== ".."),
  "report reference contains a traversal component");
  const root = path.resolve(allowedRoot);
  const target = path.resolve(root, ref);
  requireValue(isWithin(root, target), "report path escapes its allowed root");
  ensureSafeDirectoryTree(root, path.dirname(target));
  if (existsSync(target)) {
    const info = lstatSync(target, { bigint: true });
    requireValue(ownedWithoutSharedWrite(info, "file"),
      "report output is not a regular file");
  }
  return target;
}

export function atomicWriteReportJson(
  allowedRoot,
  reportRef,
  payload,
  {
    beforePublish,
    faultInjector,
    maxBytes = DEFAULT_MAX_REPORT_JSON_BYTES,
  } = {},
) {
  requireValue(
    Number.isSafeInteger(maxBytes) && maxBytes > 0 &&
      maxBytes <= DEFAULT_MAX_REPORT_JSON_BYTES,
    "report JSON byte bound is invalid",
  );
  const serialized = `${JSON.stringify(payload, null, 2)}\n`;
  requireValue(
    Buffer.byteLength(serialized, "utf8") <= maxBytes,
    "report JSON exceeds the byte bound",
  );
  let target;
  let parent;
  let parentBefore;
  let temporary;
  let descriptor;
  let parentDescriptor;
  let temporaryIdentity;
  let primaryFailure = false;
  try {
    runWriteStage("parent_open_validate", faultInjector, () => {
      target = resolveSafeReportPath(allowedRoot, reportRef);
      parent = path.dirname(target);
      parentBefore = lstatSync(parent, { bigint: true });
      temporary = path.join(
        parent,
        `.${path.basename(target)}.${randomBytes(12).toString("hex")}.tmp`,
      );
      parentDescriptor = openSync(
        parent,
        constants.O_RDONLY | (constants.O_DIRECTORY || 0) |
          (constants.O_NOFOLLOW || 0),
      );
      const openedParent = fstatSync(parentDescriptor, { bigint: true });
      requireValue(inodeIdentity(parentBefore) === inodeIdentity(openedParent) &&
        ownedWithoutSharedWrite(openedParent, "directory"),
      "report directory descriptor is unstable");
    });
    runWriteStage("temp_create", faultInjector, () => {
      descriptor = openSync(
        temporary,
        constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL |
          (constants.O_NOFOLLOW || 0),
        0o600,
      );
    });
    runWriteStage("temp_write", faultInjector, () => {
      writeFileSync(descriptor, serialized, "utf8");
    });
    runWriteStage("temp_fsync", faultInjector, () => fsyncSync(descriptor));
    runWriteStage("temp_validate", faultInjector, () => {
      temporaryIdentity = fstatSync(descriptor, { bigint: true });
      requireValue(ownedWithoutSharedWrite(temporaryIdentity, "file") &&
        temporaryIdentity.nlink === 1n,
      "temporary report file is unstable");
      closeSync(descriptor);
      descriptor = undefined;
    });
    runWriteStage("before_publish", faultInjector, () => {
      if (typeof beforePublish === "function") beforePublish({ parent, target });
    });
    runWriteStage("parent_revalidate", faultInjector, () => {
      const parentBeforeRename = lstatSync(parent, { bigint: true });
      requireValue(inodeIdentity(parentBefore) === inodeIdentity(parentBeforeRename) &&
        parentBeforeRename.isDirectory() && !parentBeforeRename.isSymbolicLink(),
      "report directory changed before atomic publication");
    });
    runWriteStage("target_validate", faultInjector, () => {
      if (existsSync(target)) {
        const targetInfo = lstatSync(target, { bigint: true });
        requireValue(ownedWithoutSharedWrite(targetInfo, "file"),
          "report output changed to an unsafe entry");
      }
    });
    runWriteStage("rename", faultInjector, () => renameSync(temporary, target));
    runWriteStage("published_validate", faultInjector, () => {
      const published = lstatSync(target, { bigint: true });
      const parentAfter = lstatSync(parent, { bigint: true });
      const openedParentAfter = fstatSync(parentDescriptor, { bigint: true });
      requireValue(ownedWithoutSharedWrite(published, "file") &&
        published.nlink === 1n &&
        inodeIdentity(published) === inodeIdentity(temporaryIdentity) &&
        published.size === temporaryIdentity.size &&
        inodeIdentity(parentBefore) === inodeIdentity(parentAfter) &&
        inodeIdentity(parentBefore) === inodeIdentity(openedParentAfter),
      "atomic report publication was not stable");
    });
    runWriteStage("directory_fsync", faultInjector, () => {
      if (process.platform !== "win32") fsyncSync(parentDescriptor);
    });
    return target;
  } catch (error) {
    primaryFailure = true;
    if (error instanceof SafeReportWriteError) throw error;
    throw new SafeReportWriteError("parent_open_validate");
  } finally {
    try {
      if (descriptor !== undefined) closeSync(descriptor);
      if (parentDescriptor !== undefined) closeSync(parentDescriptor);
      if (temporary && existsSync(temporary)) {
        const info = lstatSync(temporary, { bigint: true });
        if (info.isFile() && !info.isSymbolicLink()) unlinkSync(temporary);
      }
      if (typeof faultInjector === "function") faultInjector("cleanup");
    } catch (error) {
      if (!primaryFailure) {
        if (error instanceof SafeReportWriteError) throw error;
        throw new SafeReportWriteError("cleanup");
      }
    }
  }
}

export function atomicReplaceContainedFileSnapshot(
  allowedRoot,
  targetRef,
  sourcePath,
  { maxBytes = Number.MAX_SAFE_INTEGER, beforePublish } = {},
) {
  requireValue(Number.isSafeInteger(maxBytes) && maxBytes >= 0,
    "contained snapshot byte bound is invalid");
  const target = resolveSafeReportPath(allowedRoot, targetRef);
  const parent = path.dirname(target);
  const parentBefore = lstatSync(parent, { bigint: true });
  const temporaryName = `.${path.basename(target)}.${randomBytes(12).toString("hex")}.tmp`;
  const temporary = path.join(parent, temporaryName);
  let parentDescriptor;
  let temporaryIdentity;
  let published = false;
  try {
    parentDescriptor = openSync(
      parent,
      constants.O_RDONLY | (constants.O_DIRECTORY || 0) |
        (constants.O_NOFOLLOW || 0),
    );
    const openedParent = fstatSync(parentDescriptor, { bigint: true });
    requireValue(inodeIdentity(parentBefore) === inodeIdentity(openedParent) &&
      ownedWithoutSharedWrite(openedParent, "directory"),
    "contained snapshot directory descriptor is unstable");

    stableSnapshotFile(sourcePath, parent, temporaryName, { maxBytes });
    temporaryIdentity = lstatSync(temporary, { bigint: true });
    requireValue(ownedWithoutSharedWrite(temporaryIdentity, "file") &&
      temporaryIdentity.nlink === 1n,
    "contained snapshot temporary file is unstable");

    let targetIdentity;
    if (existsSync(target)) {
      const targetBefore = lstatSync(target, { bigint: true });
      requireValue(ownedWithoutSharedWrite(targetBefore, "file") &&
        targetBefore.nlink === 1n,
      "contained snapshot target is not a replaceable regular file");
      stableHashFileSnapshot(target, { maxBytes });
      const targetAfterValidation = lstatSync(target, { bigint: true });
      requireValue(stableEntryIdentity(targetBefore) ===
        stableEntryIdentity(targetAfterValidation),
      "contained snapshot target changed during validation");
      targetIdentity = stableEntryIdentity(targetAfterValidation);
    }

    if (typeof beforePublish === "function") beforePublish({ parent, target, temporary });

    const parentBeforeRename = lstatSync(parent, { bigint: true });
    const openedParentBeforeRename = fstatSync(parentDescriptor, { bigint: true });
    requireValue(inodeIdentity(parentBefore) === inodeIdentity(parentBeforeRename) &&
      inodeIdentity(parentBefore) === inodeIdentity(openedParentBeforeRename) &&
      ownedWithoutSharedWrite(parentBeforeRename, "directory"),
    "contained snapshot directory changed before publication");
    const targetBeforeRename = lstatSync(target, {
      bigint: true,
      throwIfNoEntry: false,
    });
    if (targetIdentity === undefined) {
      requireValue(targetBeforeRename === undefined,
        "contained snapshot target appeared before publication");
    } else {
      requireValue(targetBeforeRename !== undefined &&
        ownedWithoutSharedWrite(targetBeforeRename, "file") &&
        targetBeforeRename.nlink === 1n &&
        stableEntryIdentity(targetBeforeRename) === targetIdentity,
      "contained snapshot target changed before publication");
    }

    renameSync(temporary, target);
    published = true;
    const targetAfter = lstatSync(target, { bigint: true });
    const parentAfter = lstatSync(parent, { bigint: true });
    const openedParentAfter = fstatSync(parentDescriptor, { bigint: true });
    requireValue(ownedWithoutSharedWrite(targetAfter, "file") &&
      targetAfter.nlink === 1n &&
      publishedFileIdentity(targetAfter) === publishedFileIdentity(temporaryIdentity) &&
      inodeIdentity(parentBefore) === inodeIdentity(parentAfter) &&
      inodeIdentity(parentBefore) === inodeIdentity(openedParentAfter),
    "contained snapshot publication was not stable");
    if (process.platform !== "win32") fsyncSync(parentDescriptor);
    return target;
  } finally {
    if (parentDescriptor !== undefined) closeSync(parentDescriptor);
    if (!published && temporaryIdentity !== undefined) {
      const currentParent = lstatSync(parent, { bigint: true, throwIfNoEntry: false });
      const currentTemporary = currentParent &&
        inodeIdentity(currentParent) === inodeIdentity(parentBefore)
        ? lstatSync(temporary, { bigint: true, throwIfNoEntry: false })
        : undefined;
      if (currentTemporary?.isFile() === true &&
          currentTemporary.isSymbolicLink() === false &&
          stableEntryIdentity(currentTemporary) === stableEntryIdentity(temporaryIdentity)) {
        unlinkSync(temporary);
      }
    }
  }
}

export function removeContainedReportIfExists(allowedRoot, reportRef) {
  const target = resolveSafeReportPath(allowedRoot, reportRef);
  if (!existsSync(target)) return false;
  const info = lstatSync(target, { bigint: true });
  requireValue(ownedWithoutSharedWrite(info, "file"),
    "stale report is not a removable regular file");
  unlinkSync(target);
  if (process.platform !== "win32") {
    const parentDescriptor = openSync(
      path.dirname(target),
      constants.O_RDONLY | (constants.O_DIRECTORY || 0) |
        (constants.O_NOFOLLOW || 0),
    );
    try {
      fsyncSync(parentDescriptor);
    } finally {
      closeSync(parentDescriptor);
    }
  }
  return true;
}

export function readContainedJson(allowedRoot, relativeRef) {
  const target = path.resolve(allowedRoot, relativeRef);
  const safe = resolveContainedExistingPath(allowedRoot, target, { expectedKind: "file" });
  return JSON.parse(stableReadFile(safe, {
    maxBytes: 16 * 1024 * 1024,
  }).toString("utf8"));
}

// Shared sanitization patterns for safe error output.
// Used by safeStderr() to redact sensitive data before writing to stderr.
const REDACT_PATTERNS = Object.freeze([
  [/\/Users\/[^\s"']+/g, "[user-path-redacted]"],
  [/\/home\/[^\s"']+/g, "[home-path-redacted]"],
  [/gh[pousr]_[A-Za-z0-9_]+/g, "[github-token-redacted]"],
  [/sk-[A-Za-z0-9_\-]+/g, "[api-key-redacted]"],
  [/Bearer\s+[A-Za-z0-9._~-]+/g, "Bearer [redacted]"],
  [/-----BEGIN [A-Z ]+ PRIVATE KEY-----[A-Za-z0-9+/=\s]+-----END [A-Z ]+ PRIVATE KEY-----/g, "[pem-redacted]"],
  [/file:\/\/\/[^\s"']+/g, "[file-url-redacted]"],
  [/eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g, "[jwt-redacted]"],
]);

/**
 * Write a sanitized, bounded error message to stderr.
 * Strips sensitive patterns and truncates to maxLength characters.
 */
export function safeStderr(error, maxLength = 1200) {
  let text = error instanceof Error ? error.message : String(error);
  for (const [pattern, replacement] of REDACT_PATTERNS) {
    text = text.replace(pattern, replacement);
  }
  if (text.length > maxLength) {
    text = text.slice(0, maxLength);
  }
  process.stderr.write(`${text}\n`);
}
