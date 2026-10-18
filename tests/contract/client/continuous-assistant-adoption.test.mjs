import assert from "node:assert/strict";
import test from "node:test";
import { runAdoptionProofs } from "../../product-e2e/cli/continuous-assistant/adoption/run.mjs";
import { REQUIRED_TESTS, TARGET } from "../../product-e2e/cli/continuous-assistant/adoption/cargo-target.mjs";

test("AC-05 adoption uses the real continuity_adoption Rust target", () => {
  const { rust } = runAdoptionProofs();
  assert.equal(rust.target, TARGET);
  assert.equal(rust.status, 0, rust.output);
  assert.ok(rust.summary.running > 0, rust.output);
  assert.equal(rust.summary.failed, 0, rust.output);
  assert.equal(rust.summary.ignored, 0, rust.output);
  assert.ok(rust.summary.passed > 0, rust.output);

  for (const name of REQUIRED_TESTS) {
    assert.equal(rust.tests[name], "ok", `${name} failed\n${rust.output}`);
  }

  assert.equal(rust.oracles["default-policy"].enabled, true);
  assert.equal(rust.oracles["default-policy"].stage, "offline");
  assert.equal(rust.oracles["default-policy"].automaticDenied, true);
  assert.equal(rust.oracles["default-policy"].explicitRetained, true);
  assert.equal(rust.oracles["default-policy"].realModelQualification, "unknown");
  assert.equal(rust.oracles["disable-persist"].disabledPersisted, true);
  assert.equal(rust.oracles["disable-persist"].goalPreserved, true);
  assert.equal(rust.oracles["disable-persist"].unknownPreserved, true);
  assert.equal(rust.oracles["disable-persist"].pauseDistinctFromCancel, true);
  assert.equal(rust.oracles["disable-persist"].nonOwnerDenied, true);
  assert.equal(rust.oracles["live-admission"].renamedSyntheticRejected, true);
  assert.equal(rust.oracles["live-admission"].identityMismatchRejected, true);
  assert.equal(rust.oracles["live-admission"].stage, "admitted-shadow");
  assert.equal(rust.oracles["live-admission"].reloadKeptLive, true);
  assert.equal(rust.oracles["live-admission"].labeledSyntheticTestEvidence, true);
  assert.equal(rust.oracles["live-admission"].testEvidenceDoesNotPromoteQualification, true);
  assert.equal(rust.oracles["live-admission"].realModelQualification, "unknown");
  assert.equal(rust.oracles["live-admission"].automaticAdmittedByMechanism, false);
  assert.equal(rust.oracles["owner-issued-live"].spoofedSyntheticImportRejected, true);
  assert.equal(rust.oracles["owner-issued-live"].collectWithoutSessionRejected, true);
  assert.equal(
    rust.oracles["owner-issued-live"].productionProducerWithoutObserverUnavailable,
    true,
  );
  assert.equal(rust.oracles["owner-issued-live"].nonOwnerDenied, true);
  assert.equal(rust.oracles["owner-issued-live"].stage, "qualified-low-risk");
  assert.equal(rust.oracles["owner-issued-live"].automaticAdmittedByStoredOwner, true);
  assert.equal(rust.oracles["owner-issued-live"].reloadRevalidated, true);
  assert.equal(rust.oracles["owner-issued-live"].revocationDroppedLive, true);
  assert.equal(rust.oracles["owner-issued-live"].realModelQualification, "unknown");
  assert.equal(rust.oracles["within-process-revoke"].automaticDeniedWithoutReopen, true);
  assert.equal(rust.oracles["within-process-revoke"].explicitRetained, true);
  assert.equal(
    rust.oracles["same-responsibility-multi-identity"].sameResponsibilityDoesNotExpand,
    true,
  );
  assert.equal(rust.oracles["same-responsibility-multi-identity"].stage, "qualified-low-risk");
  assert.equal(
    rust.oracles["distinct-responsibility-expands"].distinctResponsibilitiesExpand,
    true,
  );
  assert.equal(rust.oracles["distinct-responsibility-expands"].stage, "expanded");
  assert.equal(rust.oracles["successful-collection-once"].replayRejected, true);
  assert.equal(rust.oracles["successful-collection-once"].dbUnchanged, true);
  assert.equal(rust.oracles["successful-collection-once"].cacheUnchanged, true);
  assert.equal(rust.oracles["successful-collection-once"].sessionConsumed, true);
  assert.equal(rust.oracles["bound-runtime-collect"].nativeInvoked, true);
  assert.equal(rust.oracles["bound-runtime-collect"].hermeticObserverUnused, true);
  assert.equal(rust.oracles["bound-runtime-collect"].sessionMatched, true);
  assert.equal(rust.oracles["bound-runtime-collect"].candidateMatched, true);
  assert.equal(rust.oracles["bound-runtime-collect"].materialsObserved, true);
  assert.equal(rust.oracles["bound-runtime-collect"].typedObservations, true);
  assert.equal(rust.oracles["bound-runtime-collect"].receiptBoundEvidence, true);
  assert.equal(rust.oracles["bound-runtime-collect"].persistedAndReloaded, true);
  assert.equal(rust.oracles["bound-runtime-collect"].stage, "admitted-shadow");
  assert.equal(rust.oracles["bound-runtime-collect"].notQualifiedFromSmallSet, true);
  assert.equal(rust.oracles["bound-runtime-collect"].expectedLabelsPrivate, true);
  assert.equal(rust.oracles["bound-runtime-collect"].noDuplicateNative, true);
  assert.equal(rust.oracles["bound-runtime-collect"].corpusBound, true);
  assert.equal(rust.oracles["bound-runtime-collect"].datasetVersionMatched, true);
  assert.equal(rust.oracles["bound-runtime-collect"].realModelQualification, "unknown");
  assert.equal(rust.oracles["native-failure-closed"].transportFailureNotQualified, true);
  assert.equal(rust.oracles["native-failure-closed"].malformedNotQualified, true);
  assert.equal(rust.oracles["native-failure-closed"].staleOrMismatchNotQualified, true);
  assert.equal(rust.oracles["native-failure-closed"].sessionNotConsumed, true);
  assert.equal(rust.oracles["native-failure-closed"].realModelQualification, "unknown");
  assert.equal(rust.oracles["corpus-binding"].missingFailedBeforeNative, true);
  assert.equal(rust.oracles["corpus-binding"].emptyRejected, true);
  assert.equal(rust.oracles["corpus-binding"].mismatchFailedBeforeNative, true);
  assert.equal(rust.oracles["corpus-binding"].nativeInvoked, false);
  assert.equal(rust.oracles["corpus-binding"].realModelQualification, "unknown");
  assert.equal(rust.oracles["unknown-claim-retry"].partialFailureLeftUnknown, true);
  assert.equal(rust.oracles["unknown-claim-retry"].retryReconciles, true);
  assert.equal(rust.oracles["unknown-claim-retry"].reopenRetryReconciles, true);
  assert.equal(rust.oracles["unknown-claim-retry"].invocationCountUnchanged, true);
  assert.equal(rust.oracles["unknown-claim-retry"].sessionUnconsumed, true);
  assert.equal(rust.oracles["unknown-claim-retry"].stageUnchanged, true);
  assert.equal(rust.oracles["unknown-claim-retry"].unknownIsNotProofOfNoExecution, true);
  assert.equal(rust.oracles["unknown-claim-retry"].realModelQualification, "unknown");
  assert.equal(rust.oracles["preinvoke-retry"].missingRuntimeReleased, true);
  assert.equal(rust.oracles["preinvoke-retry"].missingRuntimeRetrySucceeded, true);
  assert.equal(rust.oracles["preinvoke-retry"].missingCorpusReleased, true);
  assert.equal(rust.oracles["preinvoke-retry"].missingCorpusRetrySucceeded, true);
  assert.equal(rust.oracles["preinvoke-retry"].realModelQualification, "unknown");
  assert.equal(rust.oracles["receipt-mutation-reload"].mutatedEconomyRejectedOnReload, true);
  assert.equal(rust.oracles["receipt-mutation-reload"].liveRowDropped, true);
  assert.equal(rust.oracles["receipt-mutation-reload"].realModelQualification, "unknown");
  assert.equal(rust.oracles.migration.schemaVersion, "7");
  assert.equal(rust.oracles.migration.idempotent, true);
  assert.equal(rust.oracles.migration.unknownPreserved, true);
  assert.equal(rust.oracles.migration.noHistoricalExecutionCreated, true);
});
