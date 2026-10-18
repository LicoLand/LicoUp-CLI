import { createHash } from "node:crypto";
import { existsSync, lstatSync } from "node:fs";
import path from "node:path";
import {
  ANDROID_NATIVE_CARGO_TARGET,
  FORBIDDEN_TARGET_SEGMENTS,
  FLUTTER_TEST_TARGET,
  NATIVE_CARGO_TEST_TARGET,
  REGISTRY_PATH,
  ROOT_CARGO_TARGET,
  TEST_ARTIFACT_SCHEMA_VERSION,
} from "./constants.mjs";

export function normalizeTestArtifactTarget(repoRoot, targetPath) {
  const root = path.resolve(repoRoot);
  const absoluteTarget = path.resolve(root, targetPath);
  const relative = path.relative(root, absoluteTarget);
  if (
    relative.length === 0 ||
    relative === ".." ||
    relative.startsWith(`..${path.sep}`) ||
    path.isAbsolute(relative)
  ) {
    throw new Error("test artifact target must stay inside the repository");
  }
  const normalized = relative.split(path.sep).join("/");
  const managedCompilerOutput = normalized === ROOT_CARGO_TARGET ||
    normalized === NATIVE_CARGO_TEST_TARGET ||
    normalized === ANDROID_NATIVE_CARGO_TARGET ||
    /^build\/crates\/[^/]+\/(?:target|[a-z0-9._-]+-target)$/u.test(normalized) ||
    normalized === FLUTTER_TEST_TARGET ||
    normalized.startsWith(`${FLUTTER_TEST_TARGET}/`);
  if (!managedCompilerOutput || normalized === REGISTRY_PATH ||
      normalized.startsWith(`${REGISTRY_PATH}/`)) {
    throw new Error("test artifact target must be a managed build output");
  }
  if (normalized.split("/").some((segment) =>
    FORBIDDEN_TARGET_SEGMENTS.has(segment))) {
    throw new Error("download caches and toolchains cannot be test artifacts");
  }
  return Object.freeze({ absoluteTarget, normalized, root });
}

export function rejectSymlinkComponents(repoRoot, relativeTarget) {
  let candidate = path.resolve(repoRoot);
  for (const segment of relativeTarget.split("/")) {
    candidate = path.join(candidate, segment);
    if (existsSync(candidate) && lstatSync(candidate).isSymbolicLink()) {
      throw new Error("test artifact target cannot contain symbolic links");
    }
  }
}

export function testArtifactId(relativeTarget) {
  return createHash("sha256").update(relativeTarget).digest("hex").slice(0, 24);
}

export function validateDescriptor(descriptor, artifactId, relativeTarget) {
  if (
    descriptor?.schemaVersion !== TEST_ARTIFACT_SCHEMA_VERSION ||
    descriptor?.artifactId !== artifactId ||
    descriptor?.relativeTarget !== relativeTarget ||
    descriptor?.artifactClass !== "compiler-output" ||
    descriptor?.retention !== "reclaimable" ||
    descriptor?.containsDownloadedDependencies !== false
  ) {
    throw new Error("test artifact descriptor is invalid");
  }
  if (testArtifactId(relativeTarget) !== artifactId) {
    throw new Error("test artifact descriptor target binding is invalid");
  }
}

export function normalizeArtifactScope(scope) {
  const value = String(scope || "test");
  if (!/^[a-zA-Z0-9._:-]{1,160}$/u.test(value)) {
    throw new Error("test artifact scope is invalid");
  }
  return value;
}
