import { spawnSync } from "node:child_process";
import process from "node:process";
import {
  repoRoot,
  updateReleaseVerifierCommand,
  physicalEvidenceManifestVerifierCommand,
  reportRedactionVerifierCommand
} from "./config.mjs";
import { readText } from "./io.mjs";
import { sanitizeError, summarizeOutput } from "./lists.mjs";
import { readSourceCheckBundle } from "../lib/source-check-bundle.mjs";

export async function evaluateSourceCheck(check) {
  const { files, source } = await readSourceCheckBundle(check, readText);
  const missingTokens = check.tokens.filter((token) => !source.includes(token));
  return {
    id: check.id,
    file: check.file,
    files,
    ok: missingTokens.length === 0,
    missingTokens
  };
}

export function runConfiguredVerifier(verifierCommand, { env = {} } = {}) {
  const started = Date.now();
  const commandArgs = [verifierCommand.script, ...verifierCommand.args];
  const result = spawnSync(process.execPath, commandArgs, {
    cwd: repoRoot,
    env: {
      ...process.env,
      ...env
    },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024
  });
  return {
    id: verifierCommand.id,
    command: verifierCommand.command,
    ok: result.status === 0,
    exitCode: result.status ?? 1,
    durationMs: Date.now() - started,
    outputSummary: result.status === 0 ? summarizeOutput(result.stdout) : sanitizeError(result.stderr || result.stdout)
  };
}

export function runUpdateReleaseVerifier() {
  return runConfiguredVerifier(updateReleaseVerifierCommand);
}

export function runPhysicalEvidenceManifestVerifier() {
  return runConfiguredVerifier(physicalEvidenceManifestVerifierCommand);
}

export function runReportRedactionVerifier(redactionRunId) {
  const result = runConfiguredVerifier(reportRedactionVerifierCommand, {
    env: {
      [reportRedactionVerifierCommand.runIdEnv]: redactionRunId
    }
  });
  return {
    ...result,
    redactionRunId
  };
}
