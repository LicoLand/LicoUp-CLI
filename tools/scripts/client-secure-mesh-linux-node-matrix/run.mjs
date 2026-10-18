import process from "node:process";
import {
  classifyLinuxEvidenceValidationFailure,
} from "../lib/secure-mesh-linux-evidence.mjs";
import { parseArgs } from "./cli.mjs";
import { runMatrix } from "./matrix.mjs";
import { writeFailureReport } from "./report.mjs";
import { runSelfTest } from "./self-test.mjs";

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const phase = {
    value: "input_validation",
    get() { return this.value; },
    set(next) { this.value = next; },
  };

  if (options.selfTest) {
    try {
      console.log(JSON.stringify(runSelfTest(), null, 2));
    } catch (error) {
      const failure = classifyLinuxEvidenceValidationFailure(
        error,
        "linux_node_matrix_self_test_assertion_failed",
      );
      console.error(JSON.stringify({
        ok: false,
        reason: "linux_node_matrix_self_test_failed",
        validationRuleId: failure.ruleId,
        failureCategory: failure.category,
      }, null, 2));
      process.exitCode = 1;
    }
    return;
  }

  try {
    await runMatrix(options, phase);
  } catch {
    try {
      writeFailureReport(options, phase.get(), "linux_node_matrix_incomplete");
    } catch {
      // The nonzero exit remains authoritative when the blocked destination is unsafe.
    }
    console.error(JSON.stringify({
      ok: false,
      artifactKind: "linux-current-client-node-matrix",
      reason: "linux_node_matrix_incomplete",
      phase: phase.get(),
    }, null, 2));
    process.exitCode = 1;
  }
}
