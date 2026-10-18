import path from "node:path";
import { stableHashFileSnapshot } from "../lib/client-release-artifact-digest.mjs";
import {
  CANONICAL_CLIENT_SOURCE_ROOTS,
  clientSourceStateDigest,
} from "../lib/client-source-state-digest.mjs";
import { VERIFIER_REF } from "./constants.mjs";
import { sha256Json } from "./hash.mjs";

export function assertStableAuditSource({
  producerPath,
  producerSourceBefore,
  repoRoot,
  sourceStateDigest,
}) {
  const producerAfter = stableHashFileSnapshot(producerPath, {
    maxBytes: 4 * 1024 * 1024,
  });
  if (
    producerAfter.digest !== producerSourceBefore.digest ||
    producerAfter.device !== producerSourceBefore.device ||
    producerAfter.inode !== producerSourceBefore.inode ||
    clientSourceStateDigest(repoRoot, CANONICAL_CLIENT_SOURCE_ROOTS) !==
      sourceStateDigest
  ) {
    throw new Error("Pairwise audit source changed during verification");
  }
}

export function signoffBindingForCorpus(corpus, {
  sourceChecks,
  nativeTestFilters,
  sourceStateDigest,
  producerSourceBefore,
}) {
  const entryIds = corpus.entries.map((entry) => String(entry.id || ""));
  return {
    schemaVersion: "licomesh.secure-mesh.pairwise-content-review-signoff-binding.v2",
    corpusSchemaVersion: corpus.schemaVersion,
    corpusDigest: corpus.corpusDigest,
    corpusEntryCount: corpus.entries.length,
    corpusEntryIdsDigest: sha256Json(entryIds),
    sourceCheckCount: sourceChecks.length,
    sourceCheckIdsDigest: sha256Json(sourceChecks.map((check) => check.id)),
    nativeTestFilterCount: nativeTestFilters.length,
    nativeTestFiltersDigest: sha256Json(nativeTestFilters),
    sourceStateDigest,
    producerSourceDigest: producerSourceBefore.digest,
  };
}

export function vectorCorpusSnapshotRefForCorpus(corpus, vectorCorpusPath) {
  const digest = String(corpus.corpusDigest || "").replace(/^sha256:/u, "");
  const suffix = /^[a-f0-9]{64}$/u.test(digest) ? digest.slice(0, 16) : "unknown-digest";
  const parsed = path.posix.parse(vectorCorpusPath.replaceAll("\\", "/"));
  return `${parsed.dir}/${parsed.name}-${suffix}${parsed.ext}`;
}

export function safeReportRef(value) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref ||
    !ref.endsWith(".json") ||
    ref.startsWith("/") ||
    ref.startsWith("file:") ||
    /^https?:\/\//iu.test(ref) ||
    ref.split("/").includes("..") ||
    !(ref.startsWith("build/reports/") || ref.startsWith("build/client-cli-vm/"))) {
    return "";
  }
  return ref;
}

export function reviewSignoffTemplateForCorpus(
  corpus,
  checkedAt,
  vectorCorpusSnapshotRef,
  { sourceOfTruth, vectorCorpusPath },
) {
  const binding = corpus.signoffBinding;
  return {
    schemaVersion: "licomesh.secure-mesh.pairwise-content-review-signoff.v2",
    templateSchemaVersion: "licomesh.secure-mesh.pairwise-content-review-signoff-template.v2",
    sourceOfTruth,
    generatedBy: VERIFIER_REF,
    generatedAt: checkedAt,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    vectorCorpusReport: vectorCorpusPath,
    vectorCorpusSnapshotReport: vectorCorpusSnapshotRef,
    corpusSchemaVersion: binding.corpusSchemaVersion,
    corpusDigest: binding.corpusDigest,
    corpusEntryCount: binding.corpusEntryCount,
    corpusEntryIdsDigest: binding.corpusEntryIdsDigest,
    sourceCheckCount: binding.sourceCheckCount,
    sourceCheckIdsDigest: binding.sourceCheckIdsDigest,
    nativeTestFilterCount: binding.nativeTestFilterCount,
    nativeTestFiltersDigest: binding.nativeTestFiltersDigest,
    sourceStateDigest: binding.sourceStateDigest,
    producerSourceDigest: binding.producerSourceDigest,
    independentCryptographicReviewComplete: null,
    releaseOwnerSignoffComplete: null,
    releaseDecision: null,
    productionReadyClaimed: false,
    reviewer: {
      authority: null,
      keyId: null,
      signedAt: null,
      signatureBase64: null,
    },
    releaseOwner: {
      authority: null,
      keyId: null,
      signedAt: null,
      signatureBase64: null,
    },
    instructions: [
      "Do not change digest, count, schema, source-of-truth, or redaction fields.",
      "The independent reviewer may set independentCryptographicReviewComplete to true only after reviewing the generated vector corpus and source/test coverage represented by these digests.",
      "The release owner may set releaseOwnerSignoffComplete to true and releaseDecision to approved_for_release_gate only after accepting the independent review.",
      "productionReadyClaimed must remain false; platform secret-store and physical-device gates are separate release blockers.",
    ],
  };
}
