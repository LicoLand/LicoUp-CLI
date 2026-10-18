import { createHash } from "node:crypto";

export const DEFAULT_STABLE_READ_MAX_BYTES = 8 * 1024 * 1024;
export const DEFAULT_STREAM_CHUNK_BYTES = 1024 * 1024;
export const CLIENT_RELEASE_ARTIFACT_TREE_LIMITS = Object.freeze({
  maxEntries: 200_000,
  maxFiles: 150_000,
  maxDirectories: 50_000,
  maxTotalFileBytes: 8 * 1024 * 1024 * 1024,
  maxFileBytes: 2 * 1024 * 1024 * 1024,
  maxPathBytes: 4 * 1024,
  maxSymlinkTargetBytes: 4 * 1024,
  maxDepth: 128,
});

export function sha256Buffer(value) {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}
