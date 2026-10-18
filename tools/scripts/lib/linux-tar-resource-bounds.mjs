import { spawnSync } from "node:child_process";
import path from "node:path";
import { stableHashFileSnapshot } from "./client-release-artifact-digest.mjs";

export const LINUX_TAR_RESOURCE_LIMITS = Object.freeze({
  maxCompressedBytes: 1024 * 1024 * 1024,
  maxEntries: 50_000,
  maxSingleEntryBytes: 1024 * 1024 * 1024,
  maxExpandedBytes: 4 * 1024 * 1024 * 1024,
  maxPathBytes: 4096,
  maxListingBytes: 16 * 1024 * 1024,
  listTimeoutMs: 60_000,
  extractTimeoutMs: 180_000,
});

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function positiveSafeInteger(value, label) {
  const number = Number(value);
  requireValue(Number.isSafeInteger(number) && number > 0, `${label} is invalid`);
  return number;
}

function normalizedLimits(overrides = {}) {
  return Object.fromEntries(Object.entries(LINUX_TAR_RESOURCE_LIMITS).map(
    ([key, fallback]) => [key, positiveSafeInteger(overrides[key] ?? fallback, key)],
  ));
}

export function validateLinuxTarListings(entryListing, verboseListing, overrides = {}) {
  const limits = normalizedLimits(overrides);
  const rawEntries = String(entryListing || "").split(/\r?\n/u).filter(Boolean);
  const verboseLines = String(verboseListing || "").split(/\r?\n/u).filter(Boolean);
  requireValue(rawEntries.length > 0 && rawEntries.length <= limits.maxEntries,
    "Linux archive entry count exceeds its bound");
  requireValue(verboseLines.length === rawEntries.length,
    "Linux archive listings are inconsistent");
  const names = new Set();
  const entries = rawEntries.map((rawEntry) => {
    const entry = rawEntry.replace(/^\.\//u, "").replace(/\/$/u, "");
    const components = entry.split("/");
    requireValue(entry && Buffer.byteLength(entry, "utf8") <= limits.maxPathBytes &&
      !path.posix.isAbsolute(entry) && !entry.includes("\\") &&
      !entry.includes("\0") &&
      components.every((component) => component && component !== "." && component !== ".."),
    "Linux archive contains an unsafe entry path");
    requireValue(!names.has(entry), "Linux archive contains a duplicate entry");
    names.add(entry);
    return entry;
  });
  let expandedBytes = 0;
  verboseLines.forEach((line) => {
    requireValue(["-", "d"].includes(line[0]),
      "Linux archive contains a non-regular entry");
    const sizeMatch = line.match(/^[^\s]+\s+\S+\s+(\d+)\s+\d{4}-\d{2}-\d{2}\s/u);
    requireValue(sizeMatch, "Linux archive verbose listing is unsupported");
    const size = Number(sizeMatch[1]);
    requireValue(Number.isSafeInteger(size) && size >= 0 &&
      size <= limits.maxSingleEntryBytes,
    "Linux archive entry size exceeds its bound");
    expandedBytes += size;
    requireValue(Number.isSafeInteger(expandedBytes) &&
      expandedBytes <= limits.maxExpandedBytes,
    "Linux archive expanded size exceeds its bound");
  });
  return Object.freeze({ entries: Object.freeze(entries), expandedBytes, limits });
}

export function inspectLinuxTarGzipArchive(archivePath, overrides = {}) {
  const limits = normalizedLimits(overrides);
  stableHashFileSnapshot(archivePath, { maxBytes: limits.maxCompressedBytes });
  const common = {
    encoding: "utf8",
    maxBuffer: limits.maxListingBytes,
    timeout: limits.listTimeoutMs,
    stdio: ["ignore", "pipe", "pipe"],
  };
  const entries = spawnSync("/usr/bin/tar", ["-tzf", archivePath], common);
  requireValue(entries.status === 0 && entries.error?.code !== "ETIMEDOUT",
    "Linux archive listing failed or timed out");
  const verbose = spawnSync("/usr/bin/tar", [
    "--numeric-owner",
    "--full-time",
    "-tvzf",
    archivePath,
  ], common);
  requireValue(verbose.status === 0 && verbose.error?.code !== "ETIMEDOUT",
    "Linux archive verbose listing failed or timed out");
  return validateLinuxTarListings(entries.stdout, verbose.stdout, limits);
}
