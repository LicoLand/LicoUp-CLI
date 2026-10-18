import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { parseArgs } from "./cli.mjs";
import { loadPhysicalReportDefaults } from "./constants.mjs";
import { runProof } from "./proof.mjs";
import { failureReport, normalizeReportReference, writeReport } from "./report.mjs";
import { runPolicySelfTest } from "./self-test.mjs";

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const { defaultReportPath } = await loadPhysicalReportDefaults();
  let tempDir = "";
  let configuredReportRef = "";

  try {
    configuredReportRef = normalizeReportReference(
      options.report || defaultReportPath,
    );
    if (options.selfTest === true) {
      console.log(JSON.stringify(runPolicySelfTest()));
      return;
    }
    tempDir = mkdtempSync(path.join(os.tmpdir(), "lico-macos-adaptive-custody-proof-"));
    const report = runProof({ tempDir, configuredReportRef, options });
    writeReport(configuredReportRef, report);
    console.log(JSON.stringify({
      ok: report.ok,
      report: report.report,
      platform: report.platform,
      custodyStrategy: report.capabilityReport.custody?.strategy || "",
      enabledCapabilities: report.capabilityReport.enabled,
      safeOsStoreAvailable: report.summary.safeOsStoreAvailable,
      strongestObservedKeychainConfiguration:
        report.summary.strongestObservedKeychainConfiguration,
      promptBudgetSatisfied: report.summary.promptBudgetSatisfied,
    }, null, 2));
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    if (options.selfTest === true) {
      console.error(JSON.stringify({ ok: false, error: "macos_adaptive_custody_self_test_failed" }));
    } else {
      const report = failureReport(error, configuredReportRef);
      if (configuredReportRef) writeReport(configuredReportRef, report);
      console.error(JSON.stringify({
        ok: false,
        report: configuredReportRef || "",
        error: report.failure.code,
      }, null, 2));
    }
    process.exitCode = 1;
  } finally {
    if (tempDir) rmSync(tempDir, { recursive: true, force: true });
  }
}
