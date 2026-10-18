import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { loadSecureClientContract } from "../lib/secure-client-contract.mjs";
import {
  loadSecureClientMeshAuthorityTrustRoot,
} from "../lib/secure-client-mesh-authority-proof.mjs";
import { loadSecureClientMeshE2eeEvidenceRoutePlan } from "../lib/secure-client-mesh-e2ee-route-plan.mjs";
import { atomicWriteReportJson, resolveSafeReportPath } from "../lib/safe-report-io.mjs";
import {
  argValue,
  assertNoLeak,
  authorityProofTemplateForRoutes,
  inspectEvidenceRef,
  normalizeSafeRef,
  routeCoverage,
  runAuthorityProofSelfTest,
  runLeakScanSelfTest,
  runReadinessSelfTest,
  verifyBundle,
  bindRepoRoot,
} from "./util.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

export async function main(argv = process.argv.slice(2)) {
  bindRepoRoot(repoRoot);
  const args = new Set(argv);
  const strict = args.has("--strict");
  const contractBindingCheck = args.has("--contract-binding-check");
  const authorityProofSelfTest = args.has("--authority-proof-self-test");
  const readinessSelfTest = args.has("--readiness-self-test");
  const leakScanSelfTest = args.has("--leak-scan-self-test");
  const generateAuthorityProofTemplate = args.has("--generate-authority-proof-template");

const contract = await loadSecureClientContract();

if (leakScanSelfTest) {
  runLeakScanSelfTest();
  process.exit(0);
}
if (authorityProofSelfTest) {
  await runAuthorityProofSelfTest(contract);
  process.exit(0);
}
if (readinessSelfTest) {
  await runReadinessSelfTest(contract);
  process.exit(0);
}

const routeConfig = await loadSecureClientMeshE2eeEvidenceRoutePlan({
  canonicalBlockers: contract.SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS
});
const coverage = routeCoverage(routeConfig.evidenceRoutePlan, contract.SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS);

if (contractBindingCheck) {
  console.log(JSON.stringify({
    ok: true,
    sourceOfTruth: contract.SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    bundlePath: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_PATH,
    evidenceRefReportSchemaVersion: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    readinessField: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_READY_FIELD,
    blockerStatesField: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD,
    evidenceRefDigestsField: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_DIGESTS_FIELD,
    routeConfigRef: routeConfig.configRef,
    routeConfigSchemaVersion: routeConfig.schemaVersion,
    authorityProofTemplateRef: routeConfig.authorityProofTemplate.ref,
    stationAcceptanceReportRef: routeConfig.diagnosticRefs.stationAcceptance,
    canonicalBlockerCount: coverage.canonicalBlockerCount,
    evidenceRouteMissingCount: coverage.missingRouteBlockers.length
  }, null, 2));
  process.exit(0);
}

if (generateAuthorityProofTemplate) {
  const outputRef = normalizeSafeRef(
    argValue(argv, ["--authority-proof-template", "--authority-proof-template-out"]) ||
      process.env.LICO_SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_TEMPLATE ||
      routeConfig.authorityProofTemplate.ref,
    "authority-proof template ref"
  );
  const template = await authorityProofTemplateForRoutes({
    routeConfig,
    routePlan: routeConfig.evidenceRoutePlan,
    contract,
    outputRef
  });
  const target = resolveSafeReportPath(repoRoot, outputRef);
  await fs.mkdir(path.dirname(target), { recursive: true });
  assertNoLeak(template, "secure mesh authority proof template");
  atomicWriteReportJson(repoRoot, outputRef, template);
  console.log(JSON.stringify({
    ok: true,
    authorityProofTemplateWritten: true,
    report: outputRef,
    ...template.summary
  }, null, 2));
  process.exit(0);
}

const trustRootRef = argValue(argv, ["--trust-root", "--authority-trust-root"]) ||
  process.env[contract.SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT_ENV];
const trustRoot = await loadSecureClientMeshAuthorityTrustRoot(trustRootRef, { evidenceRoot: repoRoot });
const template = contract.createSecureClientMeshExternalEvidenceBundleTemplate();
const templateStates = template[contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD];
const productionBlockerStates = [];

for (const templateState of templateStates) {
  const blocker = String(templateState.blocker || "").trim();
  const route = routeConfig.evidenceRoutePlan[blocker];
  const reports = [];
  for (const ref of route.refs) {
    reports.push(await inspectEvidenceRef(ref, blocker, contract, trustRoot));
  }
  const evidenceRefs = reports.filter((report) => report.exists).map((report) => report.ref);
  const readyEvidenceRefs = reports.filter((report) => report.ready).map((report) => report.ref);
  const evidenceRefDigests = Object.fromEntries(reports
    .filter((report) => report.evidenceRefDigest)
    .map((report) => [report.ref, report.evidenceRefDigest]));
  const passed = route.refs.length > 0 && readyEvidenceRefs.length === route.refs.length;
  productionBlockerStates.push({
    ...templateState,
    blocker,
    status: passed ? "passed" : evidenceRefs.length > 0 ? "incomplete" : "missing",
    passed,
    evidenceRefs,
    [contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_DIGESTS_FIELD]: evidenceRefDigests,
    readyEvidenceRefs,
    evidenceRefReports: reports,
    commands: route.commands
  });
}

const readiness = contract.createSecureClientMeshProductionReadiness({
  [contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD]: productionBlockerStates
});
const checkedAt = new Date().toISOString();
const bundle = {
  schemaVersion: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_SCHEMA_VERSION,
  sourceOfTruth: contract.SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  generatedBy: "tools/scripts/client-secure-mesh-e2ee-evidence-bundle.mjs",
  generatedAt: checkedAt,
  checkedAt,
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  [contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_READY_FIELD]: readiness.productionReleaseReady,
  [contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD]: productionBlockerStates,
  contractBinding: {
    sourceOfTruth: contract.SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    routeConfigRef: routeConfig.configRef,
    routeConfigSchemaVersion: routeConfig.schemaVersion,
    canonicalBlockerCount: coverage.canonicalBlockerCount,
    stationAcceptanceReportRef: routeConfig.diagnosticRefs.stationAcceptance
  },
  readinessReduction: readiness,
  diagnostics: {
    stationAcceptanceReportRef: routeConfig.diagnosticRefs.stationAcceptance,
    physicalEvidenceManifestRef: routeConfig.diagnosticRefs.physicalEvidenceManifest,
    authorityTrustRootProvided: trustRoot.provided === true,
    authorityTrustRootAccepted: trustRoot.accepted === true
  }
};
assertNoLeak(bundle, "secure mesh e2ee evidence bundle");
const outputPath = resolveSafeReportPath(
  repoRoot,
  contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_PATH,
);
await fs.mkdir(path.dirname(outputPath), { recursive: true });
atomicWriteReportJson(
  repoRoot,
  contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_PATH,
  bundle,
);

const verification = verifyBundle(bundle, contract);
const summary = {
  ok: verification.accepted,
  report: contract.SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_PATH,
  productionReleaseReady: readiness.productionReleaseReady,
  sourceOfTruth: readiness.sourceOfTruth,
  canonicalBlockerCount: coverage.canonicalBlockerCount,
  blockedBlockerCount: readiness.productionBlockers.length,
  evidenceRouteMissingCount: coverage.missingRouteBlockers.length,
  stationAcceptanceReportRef: routeConfig.diagnosticRefs.stationAcceptance,
  clientVerifierAccepted: verification.accepted
};
console.log(JSON.stringify(summary, null, 2));

if (strict && (readiness.productionReleaseReady !== true || verification.accepted !== true)) {
  process.exitCode = 1;
}

}
