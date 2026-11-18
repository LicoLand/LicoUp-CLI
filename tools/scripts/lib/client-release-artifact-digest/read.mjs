import { createHash } from "node:crypto";
import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  fsyncSync,
  openSync,
  readSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import path from "node:path";
import {
  DEFAULT_STABLE_READ_MAX_BYTES,
  DEFAULT_STREAM_CHUNK_BYTES,
} from "./constants.mjs";
import {
  requireBeforeDeadline,
  requireValue,
  sameStableStat,
  stableLstat,
  stableOpenRead,
  validateChunkBytes,
  validateStableOpenedPath,
  normalizedDeadline,
} from "./helpers.mjs";

export function stableReadFileSnapshot(filePath, {
  afterOpen,
  maxBytes = DEFAULT_STABLE_READ_MAX_BYTES,
} = {}) {
  requireValue(Number.isInteger(maxBytes) && maxBytes >= 0,
    "stable file read maximum size is invalid");
  const descriptor = openSync(
    filePath,
    constants.O_RDONLY | (constants.O_NOFOLLOW || 0),
  );
  try {
    const before = fstatSync(descriptor, { bigint: true });
    requireValue(before.isFile(), "release artifact path is not a regular file");
    requireValue(before.size <= BigInt(maxBytes),
      "stable file read exceeds its maximum size");
    if (typeof afterOpen === "function") afterOpen(filePath);
    const expectedSize = Number(before.size);
    const bytes = Buffer.allocUnsafe(expectedSize);
    let offset = 0;
    while (offset < expectedSize) {
      const count = readSync(descriptor, bytes, offset, expectedSize - offset, null);
      if (count === 0) break;
      offset += count;
    }
    const after = fstatSync(descriptor, { bigint: true });
    requireValue(sameStableStat(before, after), "release artifact file changed while reading");
    requireValue(offset === expectedSize, "release artifact file read was incomplete");
    validateStableOpenedPath(filePath, after);
    return Object.freeze({
      bytes,
      mtimeMs: Number(after.mtimeMs),
      size: Number(after.size),
      device: String(after.dev),
      inode: String(after.ino),
    });
  } finally {
    closeSync(descriptor);
  }
}

export function stableReadFile(filePath, options = {}) {
  return stableReadFileSnapshot(filePath, options).bytes;
}

export function stableHashFileSnapshot(filePath, {
  afterOpen,
  chunkBytes: requestedChunkBytes,
  maxBytes: requestedMaxBytes,
  deadlineMs: requestedDeadlineMs,
} = {}) {
  const chunkBytes = validateChunkBytes(requestedChunkBytes);
  const maxBytes = requestedMaxBytes === undefined
    ? Number.MAX_SAFE_INTEGER
    : Number(requestedMaxBytes);
  requireValue(Number.isSafeInteger(maxBytes) && maxBytes >= 0,
    "release artifact hash byte bound is invalid");
  const deadlineMs = normalizedDeadline(requestedDeadlineMs);
  requireBeforeDeadline(deadlineMs);
  const descriptor = stableOpenRead(filePath);
  try {
    const before = fstatSync(descriptor, { bigint: true });
    requireValue(before.isFile(), "release artifact path is not a regular file");
    requireValue(before.size <= BigInt(maxBytes),
      "release artifact file exceeds its hash byte bound");
    if (typeof afterOpen === "function") afterOpen(filePath);
    const hash = createHash("sha256");
    const buffer = Buffer.allocUnsafe(chunkBytes);
    let remaining = before.size;
    let bytesRead = 0n;
    while (remaining > 0n) {
      requireBeforeDeadline(deadlineMs);
      const requested = Number(remaining > BigInt(chunkBytes)
        ? BigInt(chunkBytes)
        : remaining);
      const count = readSync(descriptor, buffer, 0, requested, null);
      requireValue(count > 0, "release artifact file hash was incomplete");
      hash.update(buffer.subarray(0, count));
      bytesRead += BigInt(count);
      remaining -= BigInt(count);
    }
    const after = fstatSync(descriptor, { bigint: true });
    requireValue(bytesRead === before.size && sameStableStat(before, after),
      "release artifact file changed while hashing");
    validateStableOpenedPath(filePath, after);
    return Object.freeze({
      digest: `sha256:${hash.digest("hex")}`,
      size: Number(after.size),
      mtimeMs: Number(after.mtimeMs),
      device: String(after.dev),
      inode: String(after.ino),
    });
  } finally {
    closeSync(descriptor);
  }
}

export function sha256File(filePath, options = {}) {
  return stableHashFileSnapshot(filePath, options).digest;
}

export function stableSnapshotFile(
  sourcePath,
  snapshotDirectory,
  snapshotName,
  { maxBytes: requestedMaxBytes } = {},
) {
  const maxBytes = requestedMaxBytes === undefined
    ? Number.MAX_SAFE_INTEGER
    : Number(requestedMaxBytes);
  requireValue(Number.isSafeInteger(maxBytes) && maxBytes >= 0,
    "stable snapshot byte bound is invalid");
  const directory = path.resolve(snapshotDirectory);
  const directoryInfo = stableLstat(directory);
  requireValue(directoryInfo?.isDirectory() === true &&
    directoryInfo.isSymbolicLink() === false,
  "stable snapshot directory is not safe");
  const target = path.resolve(directory, String(snapshotName || ""));
  requireValue(path.dirname(target) === directory && path.basename(target) === snapshotName,
    "stable snapshot name is invalid");
  requireValue(!existsSync(target), "stable snapshot target already exists");
  const sourceDescriptor = stableOpenRead(sourcePath);
  let targetDescriptor;
  try {
    const sourceBefore = fstatSync(sourceDescriptor, { bigint: true });
    requireValue(sourceBefore.isFile() &&
      sourceBefore.size <= BigInt(maxBytes),
    "stable snapshot source is not a supported regular file");
    targetDescriptor = openSync(
      target,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL |
        (constants.O_NOFOLLOW || 0),
      0o600,
    );
    const buffer = Buffer.allocUnsafe(DEFAULT_STREAM_CHUNK_BYTES);
    const sourceHash = createHash("sha256");
    let remaining = sourceBefore.size;
    while (remaining > 0n) {
      const requested = Number(remaining > BigInt(buffer.length)
        ? BigInt(buffer.length)
        : remaining);
      const count = readSync(sourceDescriptor, buffer, 0, requested, null);
      requireValue(count > 0, "stable snapshot source read was incomplete");
      sourceHash.update(buffer.subarray(0, count));
      let written = 0;
      while (written < count) {
        const writeCount = writeSync(
          targetDescriptor,
          buffer,
          written,
          count - written,
          null,
        );
        requireValue(writeCount > 0, "stable snapshot target write was incomplete");
        written += writeCount;
      }
      remaining -= BigInt(count);
    }
    fsyncSync(targetDescriptor);
    const targetBeforeClose = fstatSync(targetDescriptor, { bigint: true });
    requireValue(targetBeforeClose.isFile() && targetBeforeClose.size === sourceBefore.size,
      "stable snapshot target size is invalid");
    closeSync(targetDescriptor);
    targetDescriptor = undefined;
    const sourceAfter = fstatSync(sourceDescriptor, { bigint: true });
    requireValue(sameStableStat(sourceBefore, sourceAfter),
      "stable snapshot source changed while copying");
    validateStableOpenedPath(sourcePath, sourceAfter);
    const sourceDigest = `sha256:${sourceHash.digest("hex")}`;
    requireValue(sha256File(target, { maxBytes }) === sourceDigest,
      "stable snapshot does not match its source");
    const directoryAfter = stableLstat(directory);
    requireValue(directoryAfter?.isDirectory() === true &&
      directoryAfter.isSymbolicLink() === false &&
      directoryAfter.dev === directoryInfo.dev && directoryAfter.ino === directoryInfo.ino,
    "stable snapshot directory changed while copying");
    return target;
  } catch (error) {
    if (targetDescriptor !== undefined) closeSync(targetDescriptor);
    if (existsSync(target)) {
      const targetInfo = stableLstat(target);
      if (targetInfo?.isFile() === true && targetInfo.isSymbolicLink() === false) {
        unlinkSync(target);
      }
    }
    throw error;
  } finally {
    closeSync(sourceDescriptor);
  }
}
