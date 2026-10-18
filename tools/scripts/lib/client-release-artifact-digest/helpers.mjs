import { constants, lstatSync, openSync } from "node:fs";
import path from "node:path";
import { DEFAULT_STREAM_CHUNK_BYTES } from "./constants.mjs";

export function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

export function normalizedDeadline(deadlineMs) {
  if (deadlineMs === undefined) return Number.POSITIVE_INFINITY;
  const value = Number(deadlineMs);
  requireValue((Number.isFinite(value) || value === Number.POSITIVE_INFINITY) && value > 0,
    "release artifact deadline is invalid");
  return value;
}

export function requireBeforeDeadline(deadlineMs) {
  requireValue(Date.now() <= deadlineMs,
    "release artifact inspection deadline exceeded");
}

export function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function statIdentity(info) {
  return [
    info.dev,
    info.ino,
    info.mode,
    info.nlink,
    info.uid,
    info.gid,
    info.rdev,
    info.size,
    info.blksize,
    info.blocks,
    info.mtimeNs,
    info.ctimeNs,
    info.birthtimeNs,
  ].map(String).join(":");
}

export function sameStableStat(before, after) {
  return statIdentity(before) === statIdentity(after);
}

export function isWithin(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

export function stableLstat(filePath) {
  return lstatSync(filePath, { bigint: true, throwIfNoEntry: false });
}

export function stableOpenRead(filePath) {
  return openSync(filePath, constants.O_RDONLY | (constants.O_NOFOLLOW || 0));
}

export function validateStableOpenedPath(filePath, descriptorInfo) {
  const pathAfter = stableLstat(filePath);
  requireValue(pathAfter?.isFile() === true &&
    pathAfter.isSymbolicLink() === false &&
    pathAfter.dev === descriptorInfo.dev && pathAfter.ino === descriptorInfo.ino,
  "release artifact file path changed while reading");
}

export function validateChunkBytes(value) {
  const chunkBytes = Number(value || DEFAULT_STREAM_CHUNK_BYTES);
  requireValue(Number.isInteger(chunkBytes) && chunkBytes >= 4096 &&
    chunkBytes <= 16 * 1024 * 1024,
  "release artifact stream chunk size is invalid");
  return chunkBytes;
}
