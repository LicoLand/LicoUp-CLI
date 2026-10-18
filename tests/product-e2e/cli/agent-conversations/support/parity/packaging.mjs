import { createHash } from "node:crypto";
import { existsSync, lstatSync, readdirSync, readFileSync, readlinkSync } from "node:fs";
import { join, relative } from "node:path";
import { releaseClosureChallengeDigest, requiredReleaseClosureChallenge } from "../../../../../../tools/scripts/lib/release-closure-challenge.mjs";
import { productContinuityBindingDigest } from "../../../../../../tools/scripts/lib/agent-conversation-release-binding.mjs";
import { parityModelForAgent } from "./agent-ids.mjs";
import { packagedReleaseAppPath, packagingRegistryPath } from "./constants.mjs";
import { AcceptanceError, digest, requireFact } from "./errors.mjs";

export function readPackagedAgents() {
  let registry;
  try {
    registry = JSON.parse(readFileSync(packagingRegistryPath, "utf8"));
  } catch {
    throw new AcceptanceError("packaging_registry_invalid");
  }
  const packaged = registry?.modules?.["target-adapters"]?.targetAdapters;
  requireFact(Array.isArray(packaged), "packaging_registry_invalid");
  return new Set(packaged.filter((value) => typeof value === "string"));
}

export function requireExactFields(value, fields, code) {
  requireFact(value && typeof value === "object" && !Array.isArray(value), code);
  requireFact(Object.keys(value).every((key) => fields.has(key)), code);
}

export function readBoundedJson(path, maxBytes, code) {
  requireFact(typeof path === "string" && path.length > 0 && existsSync(path), code);
  const metadata = lstatSync(path);
  requireFact(metadata.isFile() && !metadata.isSymbolicLink() && metadata.size <= maxBytes, code);
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    throw new AcceptanceError(code);
  }
}

export function releaseBundleDigest(appBundle) {
  requireFact(existsSync(appBundle) && lstatSync(appBundle).isDirectory(), "release_app_missing");
  const hash = createHash("sha256");
  let fileCount = 0;
  const visit = (directory) => {
    const entries = readdirSync(directory, { withFileTypes: true })
      .sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      const name = relative(appBundle, path).replaceAll("\\", "/");
      const metadata = lstatSync(path);
      if (metadata.isDirectory()) {
        hash.update(`d\0${name}\0`);
        visit(path);
      } else if (metadata.isSymbolicLink()) {
        hash.update(`l\0${name}\0${readlinkSync(path)}\0`);
        fileCount += 1;
      } else if (metadata.isFile()) {
        hash.update(`f\0${name}\0`);
        hash.update(readFileSync(path));
        fileCount += 1;
      }
    }
  };
  visit(appBundle);
  return { digest: `sha256:${hash.digest("hex")}`, fileCount };
}

export function validateProductReceipt(path, expectedAgent) {
  const report = readBoundedJson(path, 256 * 1024, "release_ui_product_receipt_invalid");
  requireExactFields(report, new Set([
    "schemaVersion", "status", "receiptKind", "platform", "buildMode",
    "productHarnessKind", "fixtureBackend", "productLivePassed", "releaseUiPassed",
    "cleanupPassed", "coreJoinRequired", "externalRuntimeInvoked", "testedAgents",
    "testedAgentCount", "composerSubmitted", "progressiveTimelineVisible",
    "sameNativeSessionId", "historyReadback", "artifactDigest", "artifactFileCount",
    "artifactName",
    "invocationChallengeDigest",
  ]), "release_ui_product_receipt_unbounded");
  requireFact(
    report.schemaVersion === "lico-agent-conversation-product-e2e-report-v3"
      && report.status === "passed"
      && report.receiptKind === "release-ui-live-product"
      && report.platform === "macos"
      && report.buildMode === "release"
      && report.productHarnessKind === "packaged-release-app-live-runtime"
      && report.fixtureBackend === false
      && report.productLivePassed === true
      && report.releaseUiPassed === false
      && report.cleanupPassed === true
      && report.coreJoinRequired === true
      && report.externalRuntimeInvoked === true
      && report.composerSubmitted === true
      && report.progressiveTimelineVisible === true
      && report.sameNativeSessionId === true
      && report.historyReadback === true
      && report.artifactName === "LicoUp.app"
      && report.invocationChallengeDigest === releaseClosureChallengeDigest(
        requiredReleaseClosureChallenge(process.env),
      )
      && Array.isArray(report.testedAgents)
      && report.testedAgentCount === report.testedAgents.length,
    "release_ui_product_receipt_incomplete",
  );
  const rows = report.testedAgents.filter((row) => row?.agentId === expectedAgent);
  requireFact(rows.length === 1, "release_ui_product_agent_receipt_missing");
  const row = rows[0];
  requireExactFields(row, new Set([
    "agentId", "model", "turnCount", "productLivePassed", "releaseUiPassed",
    "cleanupPassed", "nativeContinuityDigest", "productContinuityBindingDigest",
  ]), "release_ui_product_agent_receipt_unbounded");
  const expectedModel = parityModelForAgent(expectedAgent);
  requireFact(
    row.productLivePassed === true
      && row.releaseUiPassed === false
      && row.cleanupPassed === true
      && row.turnCount === 2
      && (!expectedModel || row.model === expectedModel)
      && /^sha256:[a-f0-9]{64}$/u.test(row.nativeContinuityDigest)
      && /^sha256:[a-f0-9]{64}$/u.test(row.productContinuityBindingDigest),
    "release_ui_product_agent_receipt_incomplete",
  );
  const artifact = releaseBundleDigest(packagedReleaseAppPath);
  requireFact(
    report.artifactDigest === artifact.digest && report.artifactFileCount === artifact.fileCount,
    "release_ui_product_artifact_mismatch",
  );
  const expectedContinuityBinding = productContinuityBindingDigest({
    artifactDigest: artifact.digest,
    invocationChallengeDigest: report.invocationChallengeDigest,
    agentId: row.agentId,
    model: row.model,
    nativeDigest: row.nativeContinuityDigest,
  });
  requireFact(
    row.productContinuityBindingDigest === expectedContinuityBinding,
    "release_ui_product_continuity_binding_mismatch",
  );
  return {
    artifactDigest: artifact.digest,
    continuityBindingDigest: row.productContinuityBindingDigest,
  };
}
