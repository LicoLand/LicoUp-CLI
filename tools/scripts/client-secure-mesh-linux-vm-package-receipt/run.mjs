import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  classifyLinuxVmProducerFailure,
  createLinuxVmPackageFailureRecord,
  validateLinuxVmPackageReceipt,
} from "../lib/secure-mesh-linux-evidence.mjs";
import {
  releaseClosureChallengeDigest,
  releaseInvocationNonceDigest,
  requiredReleaseClosureChallenge,
  requiredReleaseClosureStartedAt,
  requiredReleaseInvocationNonce,
} from "../lib/release-closure-challenge.mjs";
import { parseArgs } from "./cli.mjs";
import { runReceipt } from "./receipt.mjs";
import { writeFailureReceipt, writeReport } from "./report.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

export async function main(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const workRoot = mkdtempSync(path.join(os.tmpdir(), "lico-linux-vm-package-receipt-"));
  const ctx = {
    options,
    workRoot,
    repoRoot,
    verificationPhase: "input_validation",
    releaseContext: undefined,
  };

  try {
    const challenge = requiredReleaseClosureChallenge();
    const invocationNonce = requiredReleaseInvocationNonce();
    const closureStartedAt = requiredReleaseClosureStartedAt();
    ctx.releaseContext = Object.freeze({
      closureChallengeDigest: releaseClosureChallengeDigest(challenge),
      invocationNonceDigest: releaseInvocationNonceDigest(invocationNonce),
      closureStartedAtMs: closureStartedAt.milliseconds,
    });
    const report = await runReceipt(ctx);
    ctx.verificationPhase = "receipt_validation";
    validateLinuxVmPackageReceipt(
      report,
      options.expectedSourceDigest,
      report.productVersion,
      report.buildNumber,
    );
    ctx.verificationPhase = "receipt_write";
    writeReport(options, report);
    console.log(JSON.stringify({
      ok: true,
      artifactKind: report.artifactKind,
      currentSourceArchive: true,
      installReceiptReady: true,
      sessionLaunchReady: true,
      smokeReady: true,
      privacyReady: true
    }, null, 2));
  } catch (error) {
    const failure = classifyLinuxVmProducerFailure(ctx.verificationPhase, error);
    const failureRecord = createLinuxVmPackageFailureRecord(ctx.verificationPhase, failure);
    try {
      writeFailureReceipt(options, failureRecord);
    } catch {
      // The canonical nonzero exit remains authoritative when even the blocked
      // receipt destination is unsafe or unavailable.
    }
    console.error(JSON.stringify({
      ok: false,
      artifactKind: "linux-vm-installed-client",
      reason: failureRecord.reason,
      phase: failureRecord.phase,
      validationRuleId: failureRecord.validationRuleId,
      failureCategory: failureRecord.failureCategory
    }, null, 2));
    process.exitCode = 1;
  } finally {
    rmSync(workRoot, { recursive: true, force: true });
  }
}
