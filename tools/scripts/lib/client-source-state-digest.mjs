import { createHash } from "node:crypto";
import { lstatSync, readdirSync } from "node:fs";
import path from "node:path";
import {
  stableHashFileSnapshot,
  stableReadFile,
} from "./client-release-artifact-digest.mjs";

export const CANONICAL_CLIENT_SOURCE_ROOTS = Object.freeze([
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "package-lock.json",
  "crates",
  "apps/desktop",
  "packages/protocols",
  "tools",
]);

const DEFAULT_MAX_UNTRACKED_FILE_BYTES = 256 * 1024 * 1024;
const DEFAULT_MAX_UNTRACKED_TOTAL_BYTES = 1024 * 1024 * 1024;
const DEFAULT_MAX_UNTRACKED_FILES = 100_000;
const CLIENT_SOURCE_MANIFEST_SCHEMA = "licomesh.client-source-manifest";
const CLIENT_SOURCE_MANIFEST_SCHEMA_VERSION = 1;
const DEFAULT_MAX_MANIFEST_BYTES = 64 * 1024 * 1024;

// This list is deliberately code-owned. Git ignore rules are a developer
// convenience and are not a release-input authority: an ignored source file
// can still affect a build. Only reproducible caches, generated outputs, local
// tool discovery files, and protected signing inputs are excluded here.
const EXCLUDED_SOURCE_COMPONENTS = Object.freeze(new Set([
  ".cache",
  ".dart_tool",
  ".gradle",
  ".idea",
  ".playwright-cli",
  ".pub",
  ".pub-cache",
  ".vscode",
  "DerivedData",
  "Pods",
  "build",
  "coverage",
  "ephemeral",
  "node_modules",
  "outputs",
  "reports",
  "target",
  "test-results",
  "tmp",
  "xcuserdata",
]));
const EXCLUDED_SOURCE_BASENAMES = Object.freeze(new Set([
  ".DS_Store",
  ".flutter-plugins",
  ".flutter-plugins-dependencies",
  "Generated.xcconfig",
  "GeneratedPluginRegistrant.h",
  "GeneratedPluginRegistrant.java",
  "GeneratedPluginRegistrant.m",
  "flutter_export_environment.sh",
  "key.properties",
  "local.properties",
]));

function sha256(value) {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

function canonicalJson(value) {
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

function exactKeys(value, keys, label) {
  if (!value || typeof value !== "object" || Array.isArray(value) ||
    JSON.stringify(Object.keys(value).sort()) !== JSON.stringify([...keys].sort())) {
    throw new Error(`${label} keys are invalid`);
  }
}

export function validateClientSourceRoots(sourceRoots) {
  if (!Array.isArray(sourceRoots) || sourceRoots.length === 0) {
    throw new Error("client source digest requires at least one source root");
  }
  const normalizedRoots = sourceRoots.map((value) => {
    if (typeof value !== "string" || value !== value.trim() || !value ||
      value.includes("\\") || value.includes("\0") || value.startsWith(":")) {
      throw new Error("client source digest root is not a literal repository path");
    }
    const components = value.split("/");
    if (path.isAbsolute(value) || components.some((part) =>
      !part || part === "." || part === ".." || /[*?[\]{}]/u.test(part)) ||
      path.posix.normalize(value) !== value) {
      throw new Error("client source digest roots must be literal repository-relative paths");
    }
    return value;
  });
  if (new Set(normalizedRoots).size !== normalizedRoots.length) {
    throw new Error("client source digest roots must be unique");
  }
  return [...normalizedRoots].sort();
}

export function canonicalClientSourceRootsMatch(sourceRoots) {
  return Array.isArray(sourceRoots) &&
    JSON.stringify(sourceRoots) === JSON.stringify(CANONICAL_CLIENT_SOURCE_ROOTS);
}

function positiveSafeInteger(value, fallback, label) {
  const resolved = value === undefined ? fallback : Number(value);
  if (!Number.isSafeInteger(resolved) || resolved <= 0) {
    throw new Error(`${label} is invalid`);
  }
  return resolved;
}

function assertContainedSourcePath(repoRoot, relative) {
  const resolvedRoot = path.resolve(repoRoot);
  const resolved = path.resolve(resolvedRoot, relative);
  const fromRoot = path.relative(resolvedRoot, resolved);
  if (!fromRoot || fromRoot.startsWith("..") || path.isAbsolute(fromRoot)) {
    throw new Error("client source path escapes the repository");
  }
  return resolved;
}

export function clientSourcePathExcluded(relative) {
  const normalized = String(relative || "").replaceAll("\\", "/");
  const components = normalized.split("/");
  const basename = components.at(-1) || "";
  return components.some((component) => EXCLUDED_SOURCE_COMPONENTS.has(component)) ||
    EXCLUDED_SOURCE_BASENAMES.has(basename) ||
    /^\.env(?:\.|$)/u.test(basename) ||
    /\.(?:iml|ipr|iws|jks|keystore|log|swp|swo)$/iu.test(basename);
}

function snapshotClientSourceEntries(repoRoot, sourceRoots, options = {}) {
  const normalizedRoots = validateClientSourceRoots(sourceRoots);
  const maxFileBytes = positiveSafeInteger(
    options.maxFileBytes,
    DEFAULT_MAX_UNTRACKED_FILE_BYTES,
    "client source manifest file byte bound",
  );
  const maxTotalBytes = positiveSafeInteger(
    options.maxTotalBytes,
    DEFAULT_MAX_UNTRACKED_TOTAL_BYTES,
    "client source manifest total byte bound",
  );
  const maxFiles = positiveSafeInteger(
    options.maxFiles,
    DEFAULT_MAX_UNTRACKED_FILES,
    "client source manifest file count bound",
  );
  const entries = [];
  let totalBytes = 0;

  function visit(relative) {
    if (clientSourcePathExcluded(relative)) return;
    const filePath = assertContainedSourcePath(repoRoot, relative);
    const info = lstatSync(filePath, { bigint: true, throwIfNoEntry: false });
    if (!info) throw new Error("client source manifest root or entry is missing");
    if (info.isSymbolicLink()) {
      throw new Error("client source manifest contains a symbolic link");
    }
    if (info.isDirectory()) {
      for (const name of readdirSync(filePath).sort()) {
        visit(path.posix.join(relative, name));
      }
      return;
    }
    if (!info.isFile()) {
      throw new Error("client source manifest contains an unsupported entry");
    }
    if (entries.length >= maxFiles) {
      throw new Error("client source manifest file count exceeds its bound");
    }
    const remaining = maxTotalBytes - totalBytes;
    if (remaining <= 0) {
      throw new Error("client source manifest bytes exceed their total bound");
    }
    const snapshot = stableHashFileSnapshot(filePath, {
      maxBytes: Math.min(maxFileBytes, remaining),
      afterOpen: typeof options.afterSourceOpen === "function"
        ? () => options.afterSourceOpen(relative, filePath)
        : undefined,
    });
    totalBytes += snapshot.size;
    entries.push({
      path: relative,
      type: "file",
      mode: sourceEntryMode(repoRoot, relative, snapshot),
      size: snapshot.size,
      digest: snapshot.digest,
    });
  }

  for (const sourceRoot of normalizedRoots) visit(sourceRoot);
  return entries.sort((left, right) => left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
}

function digestClientSourceEntries(sourceRoots, entries) {
  return sha256(Buffer.from(canonicalJson({
    sourceRoots: validateClientSourceRoots(sourceRoots),
    entries,
  }), "utf8"));
}

function clientSourceManifestBody(manifest) {
  return {
    schema: manifest.schema,
    schemaVersion: manifest.schemaVersion,
    sourceRoots: manifest.sourceRoots,
    sourceStateDigest: manifest.sourceStateDigest,
    entries: manifest.entries,
  };
}

function digestClientSourceManifestBody(manifest) {
  return sha256(Buffer.from(canonicalJson(clientSourceManifestBody(manifest)), "utf8"));
}

export function createClientSourceManifest(
  repoRoot,
  sourceRoots,
  sourceStateDigest,
  options = {},
) {
  if (!/^sha256:[a-f0-9]{64}$/u.test(String(sourceStateDigest || ""))) {
    throw new Error("client source manifest source-state digest is invalid");
  }
  const sourceRootsNormalized = validateClientSourceRoots(sourceRoots);
  const entries = snapshotClientSourceEntries(repoRoot, sourceRootsNormalized, options);
  if (digestClientSourceEntries(sourceRootsNormalized, entries) !== sourceStateDigest) {
    throw new Error("client source manifest does not match its source-state digest");
  }
  const manifest = {
    schema: CLIENT_SOURCE_MANIFEST_SCHEMA,
    schemaVersion: CLIENT_SOURCE_MANIFEST_SCHEMA_VERSION,
    sourceRoots: sourceRootsNormalized,
    sourceStateDigest,
    entries,
  };
  return Object.freeze({
    ...manifest,
    manifestDigest: digestClientSourceManifestBody(manifest),
  });
}

export function verifyClientSourceManifest(
  repoRoot,
  manifest,
  expectedSourceDigest,
  options = {},
) {
  exactKeys(manifest, [
    "schema",
    "schemaVersion",
    "sourceRoots",
    "sourceStateDigest",
    "entries",
    "manifestDigest",
  ], "client source manifest");
  if (manifest.schema !== CLIENT_SOURCE_MANIFEST_SCHEMA ||
    manifest.schemaVersion !== CLIENT_SOURCE_MANIFEST_SCHEMA_VERSION ||
    !/^sha256:[a-f0-9]{64}$/u.test(String(expectedSourceDigest || "")) ||
    manifest.sourceStateDigest !== expectedSourceDigest) {
    throw new Error("client source manifest binding is invalid");
  }
  const normalizedRoots = validateClientSourceRoots(manifest.sourceRoots);
  if (JSON.stringify(manifest.sourceRoots) !== JSON.stringify(normalizedRoots)) {
    throw new Error("client source manifest roots are not canonical");
  }
  if (options.expectedSourceRoots &&
    JSON.stringify(normalizedRoots) !==
      JSON.stringify(validateClientSourceRoots(options.expectedSourceRoots))) {
    throw new Error("client source manifest roots are not canonical");
  }
  if (!Array.isArray(manifest.entries) || manifest.entries.length === 0) {
    throw new Error("client source manifest entries are invalid");
  }
  let previousPath = "";
  for (const entry of manifest.entries) {
    exactKeys(entry, ["path", "type", "mode", "size", "digest"],
      "client source manifest entry");
    if (typeof entry.path !== "string" || entry.path <= previousPath ||
      entry.type !== "file" || !Number.isInteger(entry.mode) || entry.mode < 0 ||
      entry.mode > 0o7777 || !Number.isSafeInteger(entry.size) || entry.size < 0 ||
      !/^sha256:[a-f0-9]{64}$/u.test(String(entry.digest || ""))) {
      throw new Error("client source manifest entry is invalid");
    }
    previousPath = entry.path;
  }
  if (manifest.manifestDigest !== digestClientSourceManifestBody(manifest)) {
    throw new Error("client source manifest digest is invalid");
  }
  if (digestClientSourceEntries(normalizedRoots, manifest.entries) !== expectedSourceDigest) {
    throw new Error("client source manifest source-state digest is not independently derived");
  }
  const actualEntries = snapshotClientSourceEntries(repoRoot, normalizedRoots, options);
  if (canonicalJson(actualEntries) !== canonicalJson(manifest.entries)) {
    throw new Error("client source files do not match the source manifest");
  }
  return Object.freeze({
    ok: true,
    sourceStateDigest: manifest.sourceStateDigest,
    manifestDigest: manifest.manifestDigest,
    entryCount: manifest.entries.length,
  });
}

export function readAndVerifyClientSourceManifest(
  repoRoot,
  manifestPath,
  expectedSourceDigest,
  options = {},
) {
  const info = lstatSync(manifestPath, { throwIfNoEntry: false });
  if (!info?.isFile() || info.isSymbolicLink()) {
    throw new Error("client source manifest is missing or unsafe");
  }
  const manifest = JSON.parse(stableReadFile(manifestPath, {
    maxBytes: options.maxManifestBytes || DEFAULT_MAX_MANIFEST_BYTES,
  }).toString("utf8"));
  return verifyClientSourceManifest(repoRoot, manifest, expectedSourceDigest, options);
}

function sourceEntryMode(repoRoot, relative, snapshot) {
  const filePath = assertContainedSourcePath(repoRoot, relative);
  const info = lstatSync(filePath, { bigint: true, throwIfNoEntry: false });
  if (!info?.isFile() || info.isSymbolicLink() ||
    String(info.dev) !== snapshot.device || String(info.ino) !== snapshot.inode ||
    Number(info.size) !== snapshot.size) {
    throw new Error("client source entry changed while hashing");
  }
  return Number(info.mode & 0o7777n);
}

export function clientSourceStateDigest(repoRoot, sourceRoots, options = {}) {
  const normalizedRoots = validateClientSourceRoots(sourceRoots);
  const maxFileBytes = positiveSafeInteger(
    options.maxUntrackedFileBytes,
    DEFAULT_MAX_UNTRACKED_FILE_BYTES,
    "client source untracked-file byte bound",
  );
  const maxTotalBytes = positiveSafeInteger(
    options.maxUntrackedTotalBytes,
    DEFAULT_MAX_UNTRACKED_TOTAL_BYTES,
    "client source untracked-total byte bound",
  );
  const maxFiles = positiveSafeInteger(
    options.maxUntrackedFiles,
    DEFAULT_MAX_UNTRACKED_FILES,
    "client source untracked-file count bound",
  );
  const entries = snapshotClientSourceEntries(repoRoot, normalizedRoots, {
    maxFileBytes,
    maxTotalBytes,
    maxFiles,
    afterSourceOpen: options.afterUntrackedOpen,
  });
  return digestClientSourceEntries(normalizedRoots, entries);
}
