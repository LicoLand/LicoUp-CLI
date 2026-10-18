import assert from "node:assert/strict";
import test from "node:test";
import { invokeScenarioTarget } from "../../product-e2e/cli/continuous-assistant/scenarios/run.mjs";
import { loadCatalog, TARGET } from "../../product-e2e/cli/continuous-assistant/scenarios/ids.mjs";

test("AC-04-001 maps every CA-S and CA-F seed to a production case", () => {
  const catalog = loadCatalog();
  const required = catalog.cases.map((item) => item.id);
  assert.equal(required.filter((id) => id.startsWith("CA-S")).length, 48);
  assert.equal(required.filter((id) => id.startsWith("CA-F")).length, 16);

  const run = invokeScenarioTarget();
  assert.equal(run.target, TARGET);
  assert.equal(run.status, 0, run.output);
  const executed = Object.keys(run.cases);
  assert.ok(executed.length > 0, "zero scenario cases executed");

  const missing = [];
  const failed = [];
  for (const item of catalog.cases) {
    const observed = run.cases[item.id];
    if (!observed || observed.executed !== true) {
      missing.push(item.id);
      continue;
    }
    if (observed.assertionsHeld !== true) {
      failed.push(item.id);
    }
  }
  assert.deepEqual(missing, [], `unexecuted scenario ids: ${missing.join(",")}\n${run.output}`);
  assert.deepEqual(failed, [], `assertion failures: ${failed.join(",")}\n${run.output}`);

  assert.ok(run.oracle, run.output);
  assert.equal(run.oracle.ac04001.zeroTests, false);
  assert.equal(run.oracle.ac04001.copiedReducer, false);
  assert.equal(run.oracle.ac04001.timeoutSuccess, false);
  assert.equal(run.oracle.liveQualification, "unknown");
  assert.equal(run.oracle.drainWakes.postTimeDrainRuns, false);
  assert.equal(run.oracle.drainWakes.foregroundPostReturnsWithoutWakeCognition, true);
  assert.equal(run.oracle.drainWakes.backgroundAttendDueReviewsOnce, true);
});
