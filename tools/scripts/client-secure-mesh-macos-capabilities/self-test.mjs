import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { sha256File } from "../lib/client-release-artifact-digest.mjs";
import {
  artifactSecurityStateStable,
} from "./artifact-security.mjs";
import {
  capabilityProofDependencyStableAtRoot,
} from "./capability-proof.mjs";
import { capabilityProofRef, schemaVersion, verifier } from "./constants.mjs";
import { requireValue } from "./util.mjs";
import { validateReport } from "./validate.mjs";

export function selfTest() {
  const digest = `sha256:${"a".repeat(64)}`;
  const fixture = {
    schemaVersion,
    verifier,
    generatedAt: new Date().toISOString(),
    platform: "macos",
    redacted: true,
    reportLeakScan: true,
    rawRuntimeOutputIncluded: false,
    rawPrivateMaterialIncluded: false,
    sourceStateDigest: digest,
    closureChallengeDigest: digest,
    invocationNonceDigest: digest,
    buildManifestDigest: digest,
    capabilityProofDigest: digest,
    dependencies: [{
      id: "macos-user-presence-proof",
      ref: capabilityProofRef,
      digest,
    }],
    nonBlockingDistributionGuidance: {
      blocking: false,
      storeListingStatus: "not-configured",
      platformSigningStatus: "not-configured",
      notarizationStatus: "not-configured",
      updateChannelStatus: "not-configured",
    },
    receipts: [{
      targetId: "macos-arm64",
      artifactKind: "macos-app-bundle",
      artifactDigest: digest,
      runtimeExecutableDigest: digest,
      signatureMetadataDigest: digest,
      entitlementsDigest: digest,
      signatureKind: "local-identity-codesign",
      platformLocalSignatureReady: true,
      hardenedRuntime: true,
      nestedCodeMinimalEntitlements: true,
      entitlementsMatch: true,
      installedArtifactMatched: true,
      installReceiptReady: true,
      nonBlockingDistributionGuidance: {
        blocking: false,
        storeListingStatus: "not-configured",
        platformSigningStatus: "not-configured",
        notarizationStatus: "not-configured",
        updateChannelStatus: "not-configured",
      },
      launchReady: true,
      newProcessReady: true,
      startedAfterInvocation: true,
      executableWithinInstalledBundle: true,
      closureChallengeBound: true,
      invocationNonceBound: true,
      stableProcessWindowReady: true,
      postLaunchArtifactStable: true,
      smokeReady: true,
      capabilityProofReady: true,
    }],
  };
  requireValue(validateReport(fixture), "macos_receipt_positive_self_test_failed");
  for (const [name, mutate] of [
    ["ad_hoc_signature", (value) => {
      value.receipts[0].signatureKind = "local-ad-hoc-codesign";
    }],
    ["wrong_entitlements", (value) => {
      value.receipts[0].entitlementsMatch = false;
    }],
    ["missing_hardened_runtime", (value) => {
      value.receipts[0].hardenedRuntime = false;
    }],
    ["nested_entitlements_overgrant", (value) => {
      value.receipts[0].nestedCodeMinimalEntitlements = false;
    }],
    ["old_process", (value) => {
      value.receipts[0].newProcessReady = false;
    }],
    ["unstable_process", (value) => {
      value.receipts[0].stableProcessWindowReady = false;
    }],
    ["post_launch_swap", (value) => {
      value.receipts[0].postLaunchArtifactStable = false;
    }],
    ["wrong_executable", (value) => {
      value.receipts[0].executableWithinInstalledBundle = false;
    }],
    ["missing_challenge", (value) => {
      value.receipts[0].closureChallengeBound = false;
    }],
    ["blocking_distribution_guidance", (value) => {
      value.nonBlockingDistributionGuidance.blocking = true;
    }],
    ["capability_dependency_digest_missing", (value) => {
      value.dependencies[0].digest = "";
    }],
  ]) {
    const candidate = structuredClone(fixture);
    mutate(candidate);
    requireValue(!validateReport(candidate), `macos_self_test_accepted_${name}`);
  }
  const artifactState = {
    artifactDigest: digest,
    signatureKind: "local-identity-codesign",
    signatureVerified: true,
    hardenedRuntime: true,
    entitlementsMatch: true,
    entitlementsDigest: digest,
    nestedCodeMinimalEntitlements: true,
  };
  requireValue(artifactSecurityStateStable(artifactState, { ...artifactState }),
    "macos_stable_artifact_state_self_test_failed");
  requireValue(!artifactSecurityStateStable(artifactState, {
    ...artifactState,
    artifactDigest: `sha256:${"b".repeat(64)}`,
  }), "macos_during_launch_swap_self_test_failed");
  const temporaryRoot = mkdtempSync(path.join(os.tmpdir(), "lico-macos-proof-dependency-"));
  try {
    const dependencyPath = path.join(
      temporaryRoot,
      "build/reports/secure-mesh-macos-keychain-user-presence-proof.json",
    );
    mkdirSync(path.dirname(dependencyPath), { recursive: true, mode: 0o700 });
    writeFileSync(dependencyPath, "before", { mode: 0o600 });
    const dependency = {
      id: "macos-user-presence-proof",
      ref: capabilityProofRef,
      digest: sha256File(dependencyPath),
    };
    requireValue(capabilityProofDependencyStableAtRoot(temporaryRoot, dependency),
      "macos_capability_dependency_positive_self_test_failed");
    writeFileSync(dependencyPath, "after", { mode: 0o600 });
    requireValue(!capabilityProofDependencyStableAtRoot(temporaryRoot, dependency),
      "macos_capability_dependency_tamper_self_test_failed");
  } finally {
    rmSync(temporaryRoot, { recursive: true, force: true });
  }
  console.log(JSON.stringify({ ok: true, mode: "self-test", caseCount: 15 }));
}
