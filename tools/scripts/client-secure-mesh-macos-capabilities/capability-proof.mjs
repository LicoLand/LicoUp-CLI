import path from "node:path";
import process from "node:process";
import {
  resolveContainedExistingPath,
  sha256Buffer,
  sha256File,
  stableReadFileSnapshot,
} from "../lib/client-release-artifact-digest.mjs";
import { removeContainedReportIfExists } from "../lib/safe-report-io.mjs";
import { capabilityProofRef, repoRoot, sha256Pattern } from "./constants.mjs";
import { requireSuccess, requireValue, run, text } from "./util.mjs";

export function materializeCapabilityProof() {
  removeContainedReportIfExists(repoRoot, capabilityProofRef);
  const result = run(process.execPath, [
    "tools/scripts/client-secure-mesh-macos-keychain-user-presence-proof.mjs",
  ], { timeout: 90_000 });
  requireSuccess(result, "macos_exact_capability_proof_failed");
  const proofPath = resolveContainedExistingPath(
    repoRoot,
    path.join(repoRoot, capabilityProofRef),
    { expectedKind: "file" },
  );
  const snapshot = stableReadFileSnapshot(proofPath, {
    maxBytes: 16 * 1024 * 1024,
  });
  const report = JSON.parse(snapshot.bytes.toString("utf8"));
  requireValue(report?.ok === true && report?.redacted === true,
    "macos_exact_capability_proof_not_ready");
  return {
    report,
    digest: sha256Buffer(snapshot.bytes),
    dependency: {
      id: "macos-user-presence-proof",
      ref: capabilityProofRef,
      digest: sha256Buffer(snapshot.bytes),
    },
  };
}

export function capabilityProofDependencyReady(dependency) {
  return dependency?.id === "macos-user-presence-proof" &&
    dependency?.ref === capabilityProofRef &&
    sha256Pattern.test(text(dependency?.digest));
}

export function capabilityProofDependencyStable(dependency) {
  return capabilityProofDependencyStableAtRoot(repoRoot, dependency);
}

export function capabilityProofDependencyStableAtRoot(root, dependency) {
  if (!capabilityProofDependencyReady(dependency)) return false;
  try {
    const proofPath = resolveContainedExistingPath(
      path.join(root, "build"),
      path.join(root, dependency.ref),
      { expectedKind: "file" },
    );
    return sha256File(proofPath, { maxBytes: 16 * 1024 * 1024 }) ===
      dependency.digest;
  } catch {
    return false;
  }
}
