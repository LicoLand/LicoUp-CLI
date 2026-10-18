import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { loadSecureClientContract } from "../lib/secure-client-contract.mjs";
import { createSecureClientMeshE2eeRefReportScope } from "../lib/secure-client-mesh-e2ee-ref-report.mjs";
import { loadSecureMeshPairwiseContentAuditConfig } from "../lib/secure-mesh-pairwise-content-audit-config.mjs";
import { optionalReleaseInvocationBinding } from "../lib/release-closure-challenge.mjs";
import {
  CANONICAL_CLIENT_SOURCE_ROOTS,
  clientSourceStateDigest,
} from "../lib/client-source-state-digest.mjs";
import {
  stableHashFileSnapshot,
  stableReadFile,
} from "../lib/client-release-artifact-digest.mjs";
import { atomicWriteReportJson } from "../lib/safe-report-io.mjs";
import {
  PRODUCER_AUTHORITY_REF,
  repoRoot,
  VERIFIER_REF,
} from "./constants.mjs";
import { buildVectorCorpus } from "./corpus.mjs";
import { runNativeTest } from "./native-test.mjs";
import { assertNoLeak } from "./privacy.mjs";
import {
  assertStableAuditSource,
  reviewSignoffTemplateForCorpus,
  signoffBindingForCorpus,
  vectorCorpusSnapshotRefForCorpus,
} from "./signoff.mjs";
import { createSignoffLoaders } from "./signoff-load.mjs";
import { createReadText, evaluateSourceCheck } from "./source-check.mjs";

export async function main(argv = process.argv.slice(2)) {
  const pairwiseContentAuditConfig = await loadSecureMeshPairwiseContentAuditConfig();
  const reportPath = pairwiseContentAuditConfig.reportOutput;
  const vectorCorpusPath = pairwiseContentAuditConfig.vectorCorpusOutput;
  const reviewSignoffPath = pairwiseContentAuditConfig.reviewSignoffRef;
  const reviewAuthoritiesPath = path.join(
    repoRoot,
    "tools/scripts/config/secure-mesh-pairwise-review-authorities.json",
  );
  const producerPath = path.join(repoRoot, PRODUCER_AUTHORITY_REF);
  const sourceStateDigest = clientSourceStateDigest(
    repoRoot,
    CANONICAL_CLIENT_SOURCE_ROOTS,
  );
  const producerSourceBefore = stableHashFileSnapshot(producerPath, {
    maxBytes: 4 * 1024 * 1024,
  });
  const reviewAuthorities = JSON.parse(stableReadFile(reviewAuthoritiesPath, {
    maxBytes: 1024 * 1024,
  }).toString("utf8"));
  if (reviewAuthorities?.schemaVersion !==
      "licomesh.secure-mesh.pairwise-review-authorities.v1" ||
    !Array.isArray(reviewAuthorities.reviewerKeys) ||
    !Array.isArray(reviewAuthorities.releaseOwnerKeys)) {
    throw new Error("Pairwise review authority config is invalid");
  }
  const args = new Set(argv);
  const strict = args.has("--strict");
  const generateSignoffTemplate = args.has("--generate-signoff-template");
  const sourceChecks = Object.freeze(pairwiseContentAuditConfig.sourceChecks);
  const nativeTestFilters = Object.freeze(pairwiseContentAuditConfig.nativeTestFilters);
  const readText = createReadText(repoRoot);

  const contract = await loadSecureClientContract();
  const {
    SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
    SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
  } = contract;
  const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) => item === "pairwise/content crypto audit");
  if (!blocker) {
    throw new Error("Client-pinned Secure Client Mesh contract does not define pairwise/content crypto audit blocker");
  }

  const { loadReviewSignoffArtifact } = createSignoffLoaders({
    readText,
    reviewAuthorities,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  });

  const sourceResults = [];
  for (const check of sourceChecks) {
    sourceResults.push(await evaluateSourceCheck(check, readText));
  }

  const nativeResults = nativeTestFilters.map(runNativeTest);
  const vectorCorpus = await buildVectorCorpus({
    readText,
    nativeTestFilters,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    signoffBindingForCorpus: (corpus) => signoffBindingForCorpus(corpus, {
      sourceChecks,
      nativeTestFilters,
      sourceStateDigest,
      producerSourceBefore,
    }),
  });
  const checkedAt = new Date().toISOString();
  if (generateSignoffTemplate) {
    const vectorCorpusSnapshotPath = vectorCorpusSnapshotRefForCorpus(vectorCorpus, vectorCorpusPath);
    const template = reviewSignoffTemplateForCorpus(
      vectorCorpus,
      checkedAt,
      vectorCorpusSnapshotPath,
      {
        sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
        vectorCorpusPath,
      },
    );
    assertNoLeak(template, "secure mesh pairwise/content review signoff template");
    assertNoLeak(vectorCorpus, "secure mesh pairwise/content vector corpus report");
    assertStableAuditSource({
      producerPath,
      producerSourceBefore,
      repoRoot,
      sourceStateDigest,
    });
    atomicWriteReportJson(
      path.join(repoRoot, "build"),
      vectorCorpusPath.replace(/^build\//u, ""),
      vectorCorpus,
    );
    atomicWriteReportJson(
      path.join(repoRoot, "build"),
      vectorCorpusSnapshotPath.replace(/^build\//u, ""),
      vectorCorpus,
    );
    atomicWriteReportJson(
      path.join(repoRoot, "build"),
      reviewSignoffPath.replace(/^build\//u, ""),
      template,
    );
    console.log(JSON.stringify({
      ok: true,
      report: reviewSignoffPath,
      vectorCorpusReport: vectorCorpusPath,
      vectorCorpusSnapshotReport: vectorCorpusSnapshotPath,
      templateWritten: true,
      corpusDigest: template.corpusDigest,
      sourceCheckCount: template.sourceCheckCount,
      nativeTestFilterCount: template.nativeTestFilterCount,
      productionReadyClaimed: false
    }, null, 2));
    return;
  }
  const reviewSignoff = await loadReviewSignoffArtifact(reviewSignoffPath, vectorCorpus);
  const reviewSignoffReady = reviewSignoff.ok === true &&
    reviewSignoff.independentCryptographicReviewComplete === true &&
    reviewSignoff.releaseOwnerSignoffComplete === true &&
    reviewSignoff.reviewerSignatureVerified === true &&
    reviewSignoff.releaseOwnerSignatureVerified === true &&
    reviewSignoff.authoritiesDistinct === true &&
    reviewSignoff.sourceStateDigestBound === true &&
    reviewSignoff.producerSourceDigestBound === true &&
    reviewSignoff.productionReadyClaimed !== true;
  vectorCorpus.externalCryptographicReviewComplete = reviewSignoffReady;
  vectorCorpus.releaseOwnerSignoffComplete = reviewSignoffReady;
  const metadataResistanceSourceIds = new Set([
    "content-payload-bucket-padding-is-bounded-and-authenticated",
    "pairwise-relay-header-uses-double-ratchet-header-encryption"
  ]);
  const metadataResistanceTestFilters = new Set([
    "secure_mesh_content_crypto_bucket_padding_hides_length_and_round_trips_boundaries",
    "secure_mesh_content_crypto_rejects_invalid_padding_and_oversized_bucket",
    "secure_mesh_pairwise_encrypted_relay_header_hides_ratchet_structure_and_rejects_tamper",
    "secure_mesh_pairwise_encrypted_headers_preserve_old_chain_envelope_out_of_order"
  ]);
  const metadataResistanceReady = sourceResults
    .filter((result) => metadataResistanceSourceIds.has(result.id))
    .length === metadataResistanceSourceIds.size &&
    sourceResults
      .filter((result) => metadataResistanceSourceIds.has(result.id))
      .every((result) => result.ok === true) &&
    nativeResults
      .filter((result) => metadataResistanceTestFilters.has(result.id))
      .length === metadataResistanceTestFilters.size &&
    nativeResults
      .filter((result) => metadataResistanceTestFilters.has(result.id))
      .every((result) => result.ok === true);
  const ok = sourceResults.every((check) => check.ok) &&
    nativeResults.every((check) => check.ok) &&
    vectorCorpus.ok;
  const productionReady = false;
  const clientRuntimeScopeEvidence = await createSecureClientMeshE2eeRefReportScope({
    contract,
    reportRef: reportPath,
    blocker,
    checkedAt,
  });
  const report = {
    ok,
    schemaVersion: "licomesh.secure-mesh.pairwise-content-audit-report.v1",
    evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    verifier: VERIFIER_REF,
    generatedBy: VERIFIER_REF,
    generatedAt: checkedAt,
    ...optionalReleaseInvocationBinding(),
    checkedAt,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    blocker,
    diagnosticStatus: "incomplete",
    productionReady,
    releaseReady: false,
    evidenceKind: "redacted-static-native-test-and-vector-corpus-evidence",
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    ...clientRuntimeScopeEvidence,
    contractBinding: {
      sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
      canonicalBlocker: blocker,
      canonicalBlockerCount: SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.length
    },
    pairwiseContentAuditConfig: {
      ref: pairwiseContentAuditConfig.configRef,
      schemaVersion: pairwiseContentAuditConfig.schemaVersion,
      sourceCheckCount: sourceChecks.length,
      nativeTestFilterCount: nativeTestFilters.length,
      reviewSignoffRefSource: pairwiseContentAuditConfig.reviewSignoffRefSource,
      reviewSignoffEnvKey: pairwiseContentAuditConfig.reviewSignoffEnvKey ? "[configured]" : ""
    },
    sourceResults,
    nativeResults,
    vectorCorpus: {
      report: vectorCorpusPath,
      schemaVersion: vectorCorpus.schemaVersion,
      ok: vectorCorpus.ok,
      redacted: vectorCorpus.redacted,
      rawPrivateMaterialIncluded: vectorCorpus.rawPrivateMaterialIncluded,
      rawPlaintextIncluded: vectorCorpus.rawPlaintextIncluded,
      rawPublicWireBytesIncluded: vectorCorpus.rawPublicWireBytesIncluded,
      entryCount: vectorCorpus.entries.length,
      corpusDigest: vectorCorpus.corpusDigest,
      signoffBinding: vectorCorpus.signoffBinding,
      externalCryptographicReviewComplete: vectorCorpus.externalCryptographicReviewComplete,
      releaseOwnerSignoffComplete: vectorCorpus.releaseOwnerSignoffComplete
    },
    reviewSignoff,
    summary: {
      verificationPassed: ok,
      metadataResistanceReady,
      sourceCheckCount: sourceResults.length,
      nativeTestCount: nativeResults.length,
      vectorCorpusGenerated: vectorCorpus.ok,
      vectorCorpusReport: vectorCorpusPath,
      vectorCorpusEntryCount: vectorCorpus.entries.length,
      reviewSignoffReport: reviewSignoffPath,
      reviewSignoffPresent: reviewSignoff.present === true,
      reviewSignoffTemplatePresent: reviewSignoff.templatePresent === true,
      reviewSignoffTemplateDigestMatched: reviewSignoff.templateDigestMatched === true,
      reviewSignoffTemplateSnapshotPresent:
        reviewSignoff.templateSnapshotPresent === true,
      reviewSignoffTemplateSnapshotDigestMatched:
        reviewSignoff.templateSnapshotDigestMatched === true,
      reviewSignoffReady,
      reviewerSignatureVerified: reviewSignoff.reviewerSignatureVerified === true,
      releaseOwnerSignatureVerified:
        reviewSignoff.releaseOwnerSignatureVerified === true,
      reviewAuthoritiesDistinct: reviewSignoff.authoritiesDistinct === true,
      reviewSourceStateDigestBound: reviewSignoff.sourceStateDigestBound === true,
      reviewProducerSourceDigestBound:
        reviewSignoff.producerSourceDigestBound === true,
      corpusDigestMatchedBySignoff: reviewSignoff.corpusDigestMatched === true,
      nativeTestFiltersDigestMatchedBySignoff:
        reviewSignoff.nativeTestFiltersDigestMatched === true,
      sourceCheckIdsDigestMatchedBySignoff:
        reviewSignoff.sourceCheckIdsDigestMatched === true,
      externalCryptographicReviewComplete:
        reviewSignoff.independentCryptographicReviewComplete === true,
      releaseOwnerSignoffComplete: reviewSignoff.releaseOwnerSignoffComplete === true,
      productionReady,
      releaseReady: false,
      reportLeakScan: true,
      remainingGates: [
        ...(reviewSignoffReady
          ? []
          : [
              "independent pairwise/content cryptographic review",
              "release-owner signoff of the generated redacted vector corpus"
            ]),
        "production platform secret-store binding",
        "physical multi-device command/result/file matrix"
      ]
    }
  };

  assertNoLeak(report, "secure mesh pairwise/content audit report");
  assertNoLeak(vectorCorpus, "secure mesh pairwise/content vector corpus report");
  assertStableAuditSource({
    producerPath,
    producerSourceBefore,
    repoRoot,
    sourceStateDigest,
  });
  atomicWriteReportJson(
    path.join(repoRoot, "build"),
    vectorCorpusPath.replace(/^build\//u, ""),
    vectorCorpus,
  );
  atomicWriteReportJson(
    path.join(repoRoot, "build"),
    reportPath.replace(/^build\//u, ""),
    report,
  );

  console.log(JSON.stringify({
    ok,
    report: reportPath,
    blocker: report.blocker,
    diagnosticStatus: report.diagnosticStatus,
    productionReady,
    sourceCheckCount: sourceResults.length,
    nativeTestCount: nativeResults.length,
    vectorCorpusGenerated: report.summary.vectorCorpusGenerated,
    metadataResistanceReady: report.summary.metadataResistanceReady,
    remainingGateCount: report.summary.remainingGates.length
  }, null, 2));

  if (!ok || (strict && productionReady !== true)) {
    process.exitCode = 1;
  }
}
