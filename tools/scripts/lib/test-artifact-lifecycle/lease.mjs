import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import {
  DEAD_LEASE_GRACE_MS,
  TEST_ARTIFACT_SCHEMA_VERSION,
} from "./constants.mjs";
import {
  normalizeArtifactScope,
  normalizeTestArtifactTarget,
  rejectSymlinkComponents,
  testArtifactId,
  validateDescriptor,
} from "./policy.mjs";
import {
  artifactRecordPaths,
  marker,
  processIsAlive,
  readJson,
  removeStateMarkers,
  withArtifactLock,
  writeJson,
} from "./registry.mjs";

export function leaseFiles(paths) {
  if (!existsSync(paths.leases)) return [];
  return readdirSync(paths.leases, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
    .map((entry) => `${paths.leases}/${entry.name}`);
}

export function classifyLeases(paths, options = {}) {
  const deadLeaseGraceMs = options.deadLeaseGraceMs ?? DEAD_LEASE_GRACE_MS;
  const isAlive = options.isAlive ?? processIsAlive;
  const now = options.now ?? Date.now;
  let active = false;
  let invalid = false;
  const staleFiles = [];
  const files = leaseFiles(paths);
  for (const filePath of files) {
    try {
      const lease = readJson(filePath);
      const startedAt = Date.parse(lease.startedAt);
      const structurallyValid =
        lease.schemaVersion === TEST_ARTIFACT_SCHEMA_VERSION &&
        lease.artifactId === options.artifactId &&
        typeof lease.leaseId === "string" && lease.leaseId.length > 0 &&
        Number.isSafeInteger(lease.pid) && lease.pid > 0 &&
        (lease.processIds === undefined ||
          (Array.isArray(lease.processIds) && lease.processIds.every((pid) =>
            Number.isSafeInteger(pid) && pid > 0))) &&
        typeof lease.scope === "string" && Number.isFinite(startedAt);
      if (!structurallyValid) {
        invalid = true;
        continue;
      }
      const alive = isAlive(lease.pid) ||
        (lease.processIds || []).some((pid) => isAlive(pid));
      if (alive || now() - startedAt < deadLeaseGraceMs) active = true;
      else staleFiles.push(filePath);
    } catch {
      invalid = true;
    }
  }
  return Object.freeze({ active, files, invalid, staleFiles });
}

function ensureDescriptor(paths, descriptor) {
  if (existsSync(paths.descriptor)) {
    validateDescriptor(readJson(paths.descriptor), descriptor.artifactId, descriptor.relativeTarget);
    return;
  }
  writeJson(paths.descriptor, descriptor);
}

export function acquireTestArtifactLease({
  deadLeaseGraceMs,
  isAlive,
  now = Date.now,
  repoRoot,
  scope,
  targetPath,
}) {
  const target = normalizeTestArtifactTarget(repoRoot, targetPath);
  rejectSymlinkComponents(target.root, target.normalized);
  const artifactId = testArtifactId(target.normalized);
  const paths = artifactRecordPaths(target.root, artifactId);
  const leaseId = randomUUID();
  const normalizedScope = normalizeArtifactScope(scope);

  withArtifactLock(paths, () => {
    ensureDescriptor(paths, {
      artifactClass: "compiler-output",
      artifactId,
      containsDownloadedDependencies: false,
      createdAt: new Date(now()).toISOString(),
      relativeTarget: target.normalized,
      retention: "reclaimable",
      schemaVersion: TEST_ARTIFACT_SCHEMA_VERSION,
    });
    removeStateMarkers(paths);
    mkdirSync(target.absoluteTarget, { recursive: true });
    mkdirSync(paths.leases, { recursive: true });
    writeJson(`${paths.leases}/${leaseId}.json`, {
      artifactId,
      leaseId,
      pid: process.pid,
      processIds: [],
      schemaVersion: TEST_ARTIFACT_SCHEMA_VERSION,
      scope: normalizedScope,
      startedAt: new Date(now()).toISOString(),
    });
  }, { deadLeaseGraceMs, isAlive, now });

  let released = false;
  return Object.freeze({
    artifactId,
    targetPath: target.absoluteTarget,
    trackProcess(pid) {
      if (!Number.isSafeInteger(pid) || pid <= 0) {
        throw new Error("test artifact process id is invalid");
      }
      return withArtifactLock(paths, () => {
        const leasePath = `${paths.leases}/${leaseId}.json`;
        const lease = readJson(leasePath);
        const processIds = new Set(lease.processIds || []);
        processIds.add(pid);
        writeJson(leasePath, { ...lease, processIds: [...processIds] });
      }, { deadLeaseGraceMs, isAlive, now });
    },
    release() {
      return withArtifactLock(paths, () => {
        if (!released) rmSync(`${paths.leases}/${leaseId}.json`, { force: true });
        released = true;
        if (leaseFiles(paths).length > 0) return { state: "active" };
        writeJson(paths.reclaimable, marker(artifactId, "reclaimable", now));
        return { state: "reclaimable" };
      }, { deadLeaseGraceMs, isAlive, now });
    },
  });
}
