import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function runHost() {
  return spawnSync(
    process.execPath,
    [
      "tools/scripts/cargo-client.mjs",
      "test",
      "-p",
      "licoup-native",
      "--features",
      "test-support",
      "--test",
      "continuity_host",
      "--offline",
      "--",
      "--nocapture",
      "--test-threads=1",
    ],
    {
      cwd: repoRoot,
      encoding: "utf8",
    },
  );
}

test("AC-03-001 and AC-03-002 use the production host path", () => {
  const result = runHost();
  const output = `${result.stdout || ""}\n${result.stderr || ""}`;
  assert.equal(result.status, 0, output);
  const oracleLine = output
    .split("\n")
    .find((line) => line.includes("CONTINUITY_HOST_ORACLE:"));
  assert.ok(oracleLine, output);
  const oracle = JSON.parse(oracleLine.split("CONTINUITY_HOST_ORACLE:")[1]);
  assert.equal(oracle.ac03001.naturalEntry, true);
  assert.equal(oracle.ac03001.oneChildPerGoal, true);
  assert.equal(oracle.ac03001.cardIsEventPart, true);
  assert.equal(oracle.ac03001.writerIsolation, true);
  assert.equal(oracle.ac03001.noticeOnce, true);
  assert.equal(oracle.ac03002.unknownReconciles, true);
  assert.equal(oracle.ac03002.replayed, 0);
  const journeyLine = output
    .split("\n")
    .find((line) => line.includes("CONTINUITY_CA_J001_ORACLE:"));
  assert.ok(journeyLine, output);
  const journey = JSON.parse(journeyLine.split("CONTINUITY_CA_J001_ORACLE:")[1]);
  assert.equal(journey.privateLeak, false);
  assert.equal(journey.outboundGrant, false);
  assert.equal(journey.staleEvidenceUsed, false);
  assert.equal(journey.subjectVersionMatches, true);
  assert.equal(journey.cancelEffects, 0);
  assert.equal(journey.artifactAuthorRewritten, false);
  assert.equal(journey.closureAuthority, "user-acceptance");
  assert.equal(journey.replayed, 0);
  assert.ok(journey.shareGoalPresent === true || journey.shareRelationCount === 1);
  assert.match(output, /ordinary_post_reaches_admission_without_a_script/);
  assert.match(output, /scripted_user_input_admits_one_child_and_reuses_it/);
  assert.match(output, /unknown_effect_restarts_into_reconciliation_not_replay/);
  assert.match(output, /resume_goal_wire_resumes_paused_and_denies_invalid_targets/);
  assert.match(output, /accepted_completion_fresh_notice_targets_consume_once_while_b_selected/);
});
