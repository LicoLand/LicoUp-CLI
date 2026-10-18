import { parseArguments } from "./cli.mjs";
import { AcceptanceError, digest } from "./errors.mjs";
import { printLiveGateChecklist } from "./live-gate.mjs";
import { runLive } from "./live.mjs";
import { blockedResult } from "./results.mjs";
import { runSelfTest } from "./self-test/runner.mjs";
import { cleanupProductSession } from "./session-cleanup.mjs";

export async function runAcpConversationParityCli(argv = process.argv.slice(2)) {
  let output;
  try {
    const options = parseArguments(argv);
    if (options.cleanupProductSession) {
      output = await cleanupProductSession(options);
    } else if (options.printLiveGate) {
      output = printLiveGateChecklist();
    } else {
      const selfTest = await runSelfTest();
      if (options.selfTest) {
        output = selfTest;
      } else if (selfTest.status !== "passed") {
        output = blockedResult(options.agent, options.strict, false, "harness_self_test_failed", {
          permissionFailClosed: selfTest.permissionFailClosed,
          errorFailClosed: selfTest.errorFailClosed,
          boundedOutputFailClosed: selfTest.boundedOutputFailClosed,
          quiescenceOraclePassed: selfTest.quiescenceOraclePassed,
          publicStreamChunkOraclePassed: selfTest.publicStreamChunkOraclePassed,
          processLocalOraclePassed: selfTest.processLocalOraclePassed,
          processLocalCleanupSynchronized: selfTest.processLocalCleanupSynchronized,
          processLocalHostShutdownPassed: selfTest.processLocalHostShutdownPassed,
        });
      } else {
        output = await runLive(options, {
          permissionFailClosed: selfTest.permissionFailClosed,
          errorFailClosed: selfTest.errorFailClosed,
          boundedOutputFailClosed: selfTest.boundedOutputFailClosed,
          quiescenceOraclePassed: selfTest.quiescenceOraclePassed,
          publicStreamChunkOraclePassed: selfTest.publicStreamChunkOraclePassed,
          processLocalOraclePassed: selfTest.processLocalOraclePassed,
          processLocalCleanupSynchronized: selfTest.processLocalCleanupSynchronized,
          processLocalHostShutdownPassed: selfTest.processLocalHostShutdownPassed,
        });
      }
    }
  } catch (error) {
    const code = error instanceof AcceptanceError ? error.code : "unexpected_failure";
    output = {
      status: "failed",
      roundsRequired: 0,
      roundsCompleted: 0,
      cleanupCount: 0,
      cleanupVerified: false,
      errorCode: code,
      evidenceDigest: digest({ status: "failed", errorCode: code }),
    };
  }

  console.log(JSON.stringify(output));
  if (!["core-passed", "passed", "release-ui-passed", "live-gate-checklist"].includes(output.status)) {
    process.exitCode = 1;
  }
  return output;
}
