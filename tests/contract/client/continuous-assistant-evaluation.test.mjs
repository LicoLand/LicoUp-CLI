import assert from "node:assert/strict";
import test from "node:test";
import {
  runLiveAdmissionAttempt,
  runOfflineEvaluation,
} from "../../product-e2e/cli/continuous-assistant/evaluation/run.mjs";

const REQUIRED_PASSING = [
  "ac_04_002_heldout_family_split_is_disjoint_and_version_isolated",
  "ac_04_002_fpr_and_miss_use_separate_denominators",
  "ac_04_002_all_abstain_rejects_zero_fpr_coverage",
  "ac_04_002_zero_heldout_samples_are_unknown",
  "ac_04_002_wilson_bounds_come_from_rust",
  "ac_04_002_model_runtime_prompt_resource_policy_drift_is_stale",
  "ac_04_002_unknown_cost_is_not_zero",
  "ac_04_002_synthetic_evidence_is_never_live_qualified",
  "ac_04_005_full_chain_and_direct_native_exclude_unmatched",
  "ac_04_005_unpaired_cost_must_not_enter_paired_ratio",
  "ac_04_003_live_admission_requires_authority_facts_and_reload",
];

test("AC-04-002 offline evaluation uses the Rust qualification target", () => {
  const result = runOfflineEvaluation();
  const { rust, admission } = result;
  assert.equal(rust.target, "continuity_evaluation");
  assert.equal(rust.status, 0, rust.output);
  assert.ok(rust.summary.running > 0, rust.output);
  assert.equal(rust.summary.failed, 0, rust.output);
  assert.equal(rust.summary.ignored, 0, rust.output);
  assert.ok(rust.summary.passed > 0, rust.output);
  assert.equal(admission.externalCalls, 0);
  assert.equal(admission.passingCandidateReceipt, false);

  for (const name of REQUIRED_PASSING) {
    assert.equal(rust.tests[name], "ok", `${name} failed\n${rust.output}`);
  }

  assert.equal(rust.oracles["family-split"].heldoutFamilySplitDisjoint, true);
  assert.equal(rust.oracles["family-split"].familySplitLeakAbsentBefore, true);
  assert.equal(rust.oracles["family-split"].familySplitLeakPresentAfter, true);
  assert.equal(rust.oracles["family-split"].versionIsolated, true);
  assert.equal(rust.oracles.denominators.falseTakeoverOpportunities, 4);
  assert.equal(rust.oracles.denominators.falseTakeoverErrors, 1);
  assert.equal(rust.oracles.denominators.missedCommitmentOpportunities, 4);
  assert.equal(rust.oracles.denominators.missedCommitmentErrors, 2);
  assert.equal(rust.oracles["all-abstain"].allAbstainRejected, true);
  assert.equal(rust.oracles["zero-sample"].zeroSamplesUnknown, true);
  assert.equal(rust.oracles.wilson.wilsonN0Unknown, true);
  assert.equal(rust.oracles["identity-drift"].driftedDimensions, 5);
  for (const key of ["model", "runtime", "prompt", "resource", "policy"]) {
    assert.equal(
      rust.oracles["identity-drift"].dimensions[key].baselineBecameStale,
      true,
      `${key} must stale its own baseline`,
    );
    assert.equal(
      rust.oracles["identity-drift"].dimensions[key].automaticAdvancementDenied,
      true,
      `${key} must deny automatic advancement`,
    );
    assert.equal(
      rust.oracles["identity-drift"].dimensions[key].driftedDidNotInheritBaselineEvidence,
      true,
      `${key} must not inherit the baseline evidence`,
    );
  }
  assert.equal(rust.oracles["unknown-cost"].unknownCostNotZero, true);
  assert.equal(rust.oracles["synthetic-class"].syntheticNotLiveQualified, true);
  assert.equal(rust.oracles["native-comparable"].unmatchedExcluded, true);
  assert.equal(rust.oracles["native-comparable"].nativeDirectPairedDifference, 1.0);
  assert.equal(rust.oracles["native-comparable"].unpairedNativeAverageUnused, true);
  assert.equal(rust.oracles["native-comparable"].totalsExcludeNativeDirect, true);
  assert.equal(rust.oracles["live-marker"].liveMarkerRequiresNoAuthorityFacts, false);
  assert.equal(rust.oracles["live-marker"].realAdmissionRequired, true);
  assert.equal(rust.oracles["live-marker"].emptyProvenanceRejected, true);
  assert.equal(rust.oracles["live-marker"].identityMismatchRejected, true);
  assert.equal(rust.oracles["live-marker"].syntheticRemainsUnknown, true);
  assert.equal(rust.oracles["live-marker"].labeledSyntheticTestEvidence, true);
  assert.equal(rust.oracles["live-marker"].testEvidenceDoesNotPromoteQualification, true);
  assert.equal(rust.oracles["live-marker"].realModelQualification, "unknown");
  assert.equal(rust.oracles["pairing-defect"].totalsRetainSpend, true);
  assert.equal(rust.oracles["pairing-defect"].correctPairedRatio, 1.0);
  assert.equal(rust.oracles["pairing-defect"].correctEconomicallyQualified, false);
});

test("AC-04-003 missing live authority refuses before any Rust or paid effect", () => {
  const missingAuthority = runLiveAdmissionAttempt({});
  assert.equal(missingAuthority.rustInvoked, false);
  assert.equal(missingAuthority.admission.admitted, false);
  assert.equal(missingAuthority.admission.code, "approval_required");
  assert.equal(missingAuthority.admission.externalCalls, 0);
  assert.equal(missingAuthority.admission.effectsStarted, false);
  assert.equal(missingAuthority.admission.passingCandidateReceipt, false);
  assert.equal(missingAuthority.admission.candidateReceipt, null);

  const missingBudget = runLiveAdmissionAttempt({
    authority: {
      target: "synthetic-target",
      content: "synthetic-content",
      scope: "synthetic-scope",
    },
  });
  assert.equal(missingBudget.rustInvoked, false);
  assert.equal(missingBudget.admission.admitted, false);
  assert.equal(missingBudget.admission.code, "budget_exhausted");
  assert.equal(missingBudget.admission.externalCalls, 0);
  assert.equal(missingBudget.admission.passingCandidateReceipt, false);
  assert.equal(missingBudget.admission.candidateReceipt, null);
});

test("AC-04-003 live admission attempt cannot be overridden to offline", () => {
  const attempt = runLiveAdmissionAttempt({ mode: "offline" });
  assert.equal(attempt.mode, "live");
  assert.equal(attempt.rustInvoked, false);
  assert.equal(attempt.admission.mode, "live");
  assert.equal(attempt.admission.admitted, false);
  assert.equal(attempt.admission.code, "approval_required");
  assert.equal(attempt.admission.externalCalls, 0);
  assert.equal(attempt.admission.effectsStarted, false);
  assert.equal(attempt.admission.passingCandidateReceipt, false);
  assert.equal(attempt.admission.candidateReceipt, null);
});
