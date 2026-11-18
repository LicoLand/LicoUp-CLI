import {
  readdirSync,
  readlinkSync,
  realpathSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import {
  CLIENT_RELEASE_ARTIFACT_TREE_LIMITS,
  sha256Buffer,
} from "./constants.mjs";
import {
  canonicalJson,
  isWithin,
  normalizedDeadline,
  requireBeforeDeadline,
  requireValue,
  sameStableStat,
  stableLstat,
} from "./helpers.mjs";
import { stableHashFileSnapshot } from "./read.mjs";

function normalizeTreeLimits(limits = {}) {
  const merged = { ...CLIENT_RELEASE_ARTIFACT_TREE_LIMITS, ...limits };
  for (const [name, value] of Object.entries(merged)) {
    requireValue(Number.isSafeInteger(value) && value > 0,
      `release artifact tree limit is invalid: ${name}`);
  }
  requireValue(merged.maxFiles <= merged.maxEntries &&
    merged.maxDirectories <= merged.maxEntries &&
    merged.maxFileBytes <= merged.maxTotalFileBytes,
  "release artifact tree limits are inconsistent");
  return Object.freeze(merged);
}

function treeEntryMetadata(info, expectedUid, relative, limits) {
  const pathBytes = Buffer.byteLength(relative || ".", "utf8");
  const depth = relative ? relative.split("/").length : 0;
  const mode = Number(info.mode & 0o7777n);
  requireValue(pathBytes <= limits.maxPathBytes,
    "release artifact entry path exceeds its byte bound");
  requireValue(depth <= limits.maxDepth,
    "release artifact entry exceeds its depth bound");
  requireValue(Number(info.uid) === expectedUid,
    "release artifact entry is not owned by the release user");
  requireValue((mode & 0o7000) === 0,
    "release artifact entry uses privileged permission bits");
  if (!info.isSymbolicLink()) {
    requireValue((mode & 0o022) === 0,
      "release artifact entry is group- or world-writable");
  }
  return {
    mode: mode.toString(8).padStart(4, "0"),
    owner: Number(info.uid),
    group: Number(info.gid),
    depth,
  };
}

export function artifactTreeDigest(artifactPath, {
  onDirectoryRead,
  limits: requestedLimits,
  expectedUid: requestedExpectedUid,
  deadlineMs: requestedDeadlineMs,
} = {}) {
  return artifactTreeSnapshot(artifactPath, {
    onDirectoryRead,
    limits: requestedLimits,
    expectedUid: requestedExpectedUid,
    deadlineMs: requestedDeadlineMs,
  }).digest;
}

export function artifactTreeContentDigest(artifactPath, {
  onDirectoryRead,
  limits: requestedLimits,
  expectedUid: requestedExpectedUid,
  deadlineMs: requestedDeadlineMs,
  allowExternalHardlinks = false,
} = {}) {
  const snapshot = artifactTreeSnapshot(artifactPath, {
    onDirectoryRead,
    limits: requestedLimits,
    expectedUid: requestedExpectedUid,
    deadlineMs: requestedDeadlineMs,
    allowExternalHardlinks,
  });
  const contentRecords = snapshot.entries.map((entry) => {
    if (entry.kind === "file") {
      return {
        kind: entry.kind,
        path: entry.path,
        size: entry.size,
        digest: entry.digest,
      };
    }
    if (entry.kind === "symlink") {
      return { kind: entry.kind, path: entry.path, target: entry.target };
    }
    return { kind: entry.kind, path: entry.path };
  });
  return sha256Buffer(Buffer.from(canonicalJson(contentRecords), "utf8"));
}

export function artifactTreeSnapshot(artifactPath, {
  onDirectoryRead,
  limits: requestedLimits,
  expectedUid: requestedExpectedUid,
  deadlineMs: requestedDeadlineMs,
  allowExternalHardlinks = false,
} = {}) {
  const deadlineMs = normalizedDeadline(requestedDeadlineMs);
  requireBeforeDeadline(deadlineMs);
  const top = stableLstat(artifactPath);
  requireValue(top !== undefined && top.isDirectory(), "release artifact root is not a directory");
  requireValue(top.isSymbolicLink() === false, "release artifact root must not be a symbolic link");
  const root = realpathSync(artifactPath);
  const records = [];
  const limits = normalizeTreeLimits(requestedLimits);
  const expectedUid = requestedExpectedUid === undefined
    ? (typeof process.geteuid === "function" ? process.geteuid() : Number(top.uid))
    : Number(requestedExpectedUid);
  requireValue(Number.isSafeInteger(expectedUid) && expectedUid >= 0,
    "release artifact expected owner is invalid");
  let entryCount = 0;
  let fileCount = 0;
  let directoryCount = 0;
  let totalFileBytes = 0n;
  const hardlinkGroups = new Map();

  function visit(current, relative) {
    requireBeforeDeadline(deadlineMs);
    const infoBefore = stableLstat(current);
    requireValue(infoBefore !== undefined, "release artifact entry disappeared");
    entryCount += 1;
    requireValue(entryCount <= limits.maxEntries,
      "release artifact tree exceeds its entry-count bound");
    const metadata = treeEntryMetadata(infoBefore, expectedUid, relative, limits);
    if (infoBefore.isSymbolicLink()) {
      requireValue(relative !== "", "release artifact root must not be a symbolic link");
      const targetBefore = readlinkSync(current);
      requireValue(Buffer.byteLength(targetBefore, "utf8") <=
        limits.maxSymlinkTargetBytes,
      "release artifact symlink target exceeds its byte bound");
      requireValue(infoBefore.nlink === 1n,
        "release artifact symlink has unsupported hard links");
      requireValue(!path.isAbsolute(targetBefore), "release artifact contains an absolute symlink");
      const resolvedTargetBefore = realpathSync(current);
      requireValue(isWithin(root, resolvedTargetBefore),
        "release artifact symlink resolves outside the artifact root");
      const targetAfter = readlinkSync(current);
      const infoAfter = stableLstat(current);
      const resolvedTargetAfter = realpathSync(current);
      requireValue(infoAfter?.isSymbolicLink() === true && targetBefore === targetAfter &&
        sameStableStat(infoBefore, infoAfter) && resolvedTargetBefore === resolvedTargetAfter,
      "release artifact symlink changed while reading");
      records.push({
        kind: "symlink",
        path: relative,
        target: targetBefore,
        ...metadata,
      });
      return;
    }
    if (infoBefore.isDirectory()) {
      directoryCount += 1;
      requireValue(directoryCount <= limits.maxDirectories,
        "release artifact tree exceeds its directory-count bound");
      const namesBefore = readdirSync(current).sort();
      if (typeof onDirectoryRead === "function") onDirectoryRead(current, relative);
      for (const name of namesBefore) {
        visit(path.join(current, name), relative ? `${relative}/${name}` : name);
      }
      const namesAfter = readdirSync(current).sort();
      const infoAfter = stableLstat(current);
      requireValue(infoAfter?.isDirectory() === true &&
        sameStableStat(infoBefore, infoAfter) &&
        JSON.stringify(namesBefore) === JSON.stringify(namesAfter),
      "release artifact directory changed while reading");
      records.push({
        kind: "directory",
        path: relative,
        childCount: namesBefore.length,
        ...metadata,
      });
      return;
    }
    requireValue(infoBefore.isFile(), "release artifact contains an unsupported filesystem entry");
    fileCount += 1;
    requireValue(fileCount <= limits.maxFiles,
      "release artifact tree exceeds its file-count bound");
    requireValue(infoBefore.size <= BigInt(limits.maxFileBytes),
      "release artifact file exceeds its byte bound");
    totalFileBytes += infoBefore.size;
    requireValue(totalFileBytes <= BigInt(limits.maxTotalFileBytes),
      "release artifact tree exceeds its total file-byte bound");
    const snapshot = stableHashFileSnapshot(current, {
      maxBytes: limits.maxFileBytes,
      deadlineMs,
    });
    const infoAfter = stableLstat(current);
    requireValue(infoAfter?.isFile() === true && sameStableStat(infoBefore, infoAfter),
      "release artifact file changed during tree hashing");
    const inodeKey = `${infoBefore.dev}:${infoBefore.ino}`;
    const hardlinkGroup = hardlinkGroups.get(inodeKey) || {
      firstPath: relative,
      expectedLinks: Number(infoBefore.nlink),
      observedLinks: 0,
    };
    requireValue(hardlinkGroup.expectedLinks === Number(infoBefore.nlink),
      "release artifact hard-link metadata changed");
    hardlinkGroup.observedLinks += 1;
    hardlinkGroups.set(inodeKey, hardlinkGroup);
    records.push({
      kind: "file",
      path: relative,
      size: snapshot.size,
      digest: snapshot.digest,
      hardlinkGroup: hardlinkGroup.firstPath,
      linkCount: Number(infoBefore.nlink),
      ...metadata,
    });
  }

  visit(root, "");
  const topAfter = stableLstat(artifactPath);
  requireValue(topAfter?.isDirectory() === true && topAfter.isSymbolicLink() === false &&
    sameStableStat(top, topAfter), "release artifact root changed while hashing");
  if (!allowExternalHardlinks) {
    for (const group of hardlinkGroups.values()) {
      requireValue(group.expectedLinks === group.observedLinks,
        "release artifact file has a hard link outside the artifact tree");
    }
  }
  return Object.freeze({
    digest: sha256Buffer(Buffer.from(canonicalJson(records), "utf8")),
    root,
    limits,
    deadlineMs,
    metrics: Object.freeze({
      entryCount,
      fileCount,
      directoryCount,
      totalFileBytes: Number(totalFileBytes),
    }),
    entries: Object.freeze(records.map((record) => Object.freeze({ ...record }))),
  });
}
