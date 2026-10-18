#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import {
  CANONICAL_CLIENT_SOURCE_ROOTS,
  clientSourceStateDigest,
} from "./lib/client-source-state-digest.mjs";
import { stableHashFileSnapshot } from "./lib/client-release-artifact-digest.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { requireReleaseCliTargetEvidence } from "./lib/client-release-target-evidence.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|\/tmp\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
  ["file_canary", /(?:release-cli-private-file-canary|release-cli-private-relative-canary|release-cli-approved-root-canary)/u],
  ["command_body", /"body"\s*:/u],
]);

const options = parseArgs(process.argv.slice(2));
const releaseInvocationBinding = optionalReleaseInvocationBinding();
const defaultReportPath = defaultReleaseCliReportPath(
  options.platform || process.env.LICO_RELEASE_CLI_PLATFORM || hostPlatform()
);
const tempDir = mkdtempSync(path.join(os.tmpdir(), "lico-release-cli-proof-"));

try {
  const report = runProof();
  writeReport(report);
  console.log(JSON.stringify({
    ok: report.ok,
    report: report.report,
    platform: report.platform,
    releaseCliProofReady: report.summary.releaseCliProofReady,
    commandExecuteReady: report.summary.commandExecuteReady,
    commandReplayRejected: report.summary.commandReplayRejected,
    filePolicyReady: report.summary.filePolicyReady,
    trustPolicyReady: report.summary.trustPolicyReady,
  }, null, 2));
  if (!report.ok) {
    process.exitCode = 1;
  }
} catch (error) {
  const report = failureReport(error);
  writeReport(report);
  console.error(JSON.stringify({
    ok: false,
    report: report.report,
    error: report.failure.code,
  }, null, 2));
  process.exitCode = 1;
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}

function runProof() {
  const cliValue = String(options.cli || process.env.LICO_RELEASE_CLI || "").trim();
  assert(cliValue, "release CLI path is required");
  const cli = path.resolve(cliValue);
  const sourceStateDigest = clientSourceStateDigest(
    repoRoot,
    CANONICAL_CLIENT_SOURCE_ROOTS,
  );
  const cliBefore = stableHashFileSnapshot(cli, { maxBytes: 512 * 1024 * 1024 });
  const platform = options.platform || process.env.LICO_RELEASE_CLI_PLATFORM || hostPlatform();
  const portableDir = path.join(tempDir, "portable");
  const ledgerPath = path.join(tempDir, "secure-command-replay.sqlite");
  const env = {
    ...process.env,
    LICOUP_HOME: portableDir,
    LICO_MOBILE_RELAY_NATIVE_SECRET_STORE:
      process.env.LICO_MOBILE_RELAY_NATIVE_SECRET_STORE || "portable",
  };

  const status = runJson(cli, ["secure-mesh", "status"], env);
  const commandFirst = runJson(cli, [
    "secure-mesh",
    "command",
    "execute",
    "--payload",
    JSON.stringify(commandPayload("cmd_release_cli_execute", "idem_release_cli_execute")),
    "--context",
    JSON.stringify(commandContext()),
    "--ledger-path",
    ledgerPath,
  ], env);
  const commandReplay = runJson(cli, [
    "secure-mesh",
    "command",
    "execute",
    "--payload",
    JSON.stringify(commandPayload("cmd_release_cli_execute", "idem_release_cli_replay")),
    "--context",
    JSON.stringify(commandContext()),
    "--ledger-path",
    ledgerPath,
  ], env);
  const route = runJson(cli, [
    "secure-mesh",
    "file",
    "route",
    "--manifest",
    JSON.stringify(fileManifest()),
  ], env);
  const receiveDestination = runJson(cli, [
    "secure-mesh",
    "file",
    "receive-destination",
    "--manifest",
    JSON.stringify(fileManifest()),
    "--approved-root",
    path.join(tempDir, "release-cli-approved-root-canary"),
  ], env);
  const receiveConfirmation = runJson(cli, [
    "secure-mesh",
    "file",
    "receive-confirmation",
    "--manifest",
    JSON.stringify(fileManifest()),
    "--approved-root",
    path.join(tempDir, "release-cli-approved-root-canary"),
  ], env);
  const trust = runJson(cli, [
    "secure-mesh",
    "device-trust",
    "evaluate",
    "--identity",
    JSON.stringify(identityFixture("release-cli-peer", 0x21, 0x22, 2)),
    "--previous-identity",
    JSON.stringify(identityFixture("release-cli-peer", 0x11, 0x12, 1)),
    "--trust-state",
    "verified",
    "--require-verified-device",
    "true",
  ], env);

  const summary = {
    statusReady: status.protocolVersion === "licomesh.secure-mesh.v1" &&
      Array.isArray(status.supportedTransports) &&
      status.supportedTransports.length >= 5,
    commandExecuteReady: commandFirst.ok === true &&
      commandFirst.evaluation?.accepted === true &&
      commandFirst.evaluation?.shouldExecute === true &&
      commandFirst.execution?.outcome === "result" &&
      commandFirst.bodyRedacted === true,
    commandReplayRejected: commandReplay.ok === true &&
      commandReplay.evaluation?.shouldExecute === false &&
      commandReplay.evaluation?.replayed === true &&
      commandReplay.execution?.outcome === "error" &&
      commandReplay.execution?.errorCode === "command_replay_rejected" &&
      commandReplay.bodyRedacted === true,
    fileRouteReady: route.ok === true &&
      route.route?.uploadOperation === "secure_mesh.file_chunk.upload" &&
      route.route?.metadataEncrypted === true &&
      route.transfer?.chunkCount === 2 &&
      route.resume?.ackRequired === false,
    fileReceiveDestinationReady: receiveDestination.ok === true &&
      receiveDestination.receivePolicy?.destinationApproved === true &&
      receiveDestination.receivePolicy?.destinationPathRedacted === true &&
      receiveDestination.manifest?.metadataEncrypted === true &&
      receiveDestination.manifest?.bodyRedacted === true,
    fileReceiveConfirmationReady: receiveConfirmation.ok === true &&
      receiveConfirmation.receiveConfirmation?.required === true &&
      receiveConfirmation.receiveConfirmation?.userVisibleConfirmationRequired === true &&
      receiveConfirmation.receiveConfirmation?.writeAllowed === false &&
      receiveConfirmation.receiveConfirmation?.autoPreviewEnabled === false &&
      receiveConfirmation.receiveConfirmation?.autoIngestionEnabled === false &&
      receiveConfirmation.receivePolicy?.destinationPathRedacted === true,
    trustPolicyReady: trust.ok === true &&
      trust.keyChangeDetected === true &&
      trust.trustState === "key_changed" &&
      trust.decision?.allowedForHighRiskCommand === false,
  };
  summary.filePolicyReady = summary.fileRouteReady &&
    summary.fileReceiveDestinationReady &&
    summary.fileReceiveConfirmationReady;
  summary.releaseCliProofReady = summary.statusReady &&
    summary.commandExecuteReady &&
    summary.commandReplayRejected &&
    summary.filePolicyReady &&
    summary.trustPolicyReady;

  const cliAfter = stableHashFileSnapshot(cli, { maxBytes: 512 * 1024 * 1024 });
  assert(cliBefore.digest === cliAfter.digest && cliBefore.device === cliAfter.device &&
    cliBefore.inode === cliAfter.inode,
  "release CLI changed during proof generation");
  assert(clientSourceStateDigest(repoRoot, CANONICAL_CLIENT_SOURCE_ROOTS) ===
    sourceStateDigest, "client source changed during release CLI proof generation");
  const report = {
    schemaVersion: "licomesh.secure-mesh.release-cli-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-release-cli-proof.mjs",
    generatedAt: new Date().toISOString(),
    ...releaseInvocationBinding,
    report: reportReference(),
    reportLeakScan: true,
    ok: summary.releaseCliProofReady,
    platform,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "release-cli-binary",
    sourceStateDigest,
    cliArtifactDigest: cliBefore.digest,
    proofSurface: {
      sharedRustSecureMeshCore: true,
      releaseCliExecuted: true,
      commandGateExecuted: summary.commandExecuteReady,
      commandReplayRejected: summary.commandReplayRejected,
      fileRoutePolicyExecuted: summary.fileRouteReady,
      fileReceivePolicyExecuted: summary.fileReceiveDestinationReady,
      fileReceiveConfirmationPolicyExecuted: summary.fileReceiveConfirmationReady,
      trustPolicyExecuted: summary.trustPolicyReady,
    },
    summary,
  };
  requireReleaseCliTargetEvidence(report, {
    platform,
    sourceStateDigest,
    runtimeExecutableDigest: cliBefore.digest,
  });
  return report;
}

function failureReport(error) {
  return {
    schemaVersion: "licomesh.secure-mesh.release-cli-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-release-cli-proof.mjs",
    generatedAt: new Date().toISOString(),
    ...releaseInvocationBinding,
    report: reportReference(),
    reportLeakScan: true,
    ok: false,
    platform: options.platform || hostPlatform(),
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "release-cli-binary",
    sourceStateDigest: "",
    cliArtifactDigest: "",
    failure: {
      code: "release_cli_proof_failed",
      sanitized: sanitizeError(error),
    },
    summary: {
      releaseCliProofReady: false,
      statusReady: false,
      commandExecuteReady: false,
      commandReplayRejected: false,
      filePolicyReady: false,
      fileRouteReady: false,
      fileReceiveDestinationReady: false,
      fileReceiveConfirmationReady: false,
      trustPolicyReady: false,
    },
  };
}

function commandPayload(commandId, idempotencyKey) {
  return {
    schema: "licomesh.secure-mesh.command.v1",
    commandId,
    commandKind: "client.activity.sync",
    senderIdentity: {
      endpointId: "release-cli-sender",
      identityFingerprint: "release-cli-fingerprint",
      trustState: "verified",
      endpointKind: "desktop_sidecar",
    },
    targetBinding: {
      targetEndpointId: "release-cli-recipient",
      targetAgentId: "release-cli-agent",
      workspaceId: "release-cli-workspace",
    },
    riskClass: "read_only",
    requiresUserConfirmation: false,
    idempotencyKey,
    createdAt: "2026-01-01T00:00:00Z",
    expiresAt: "2026-01-01T00:10:00Z",
    body: { limit: 1 },
  };
}

function commandContext() {
  return {
    localEndpointId: "release-cli-recipient",
    senderEndpointId: "release-cli-sender",
    senderIdentityFingerprint: "release-cli-fingerprint",
    senderTrustState: "verified",
    senderEndpointKind: "desktop_sidecar",
    senderRosterActive: true,
    targetRosterActive: true,
    sessionOrEpochValid: true,
    userConfirmed: false,
    allowedWorkspaceIds: ["release-cli-workspace"],
    allowedAgentIds: ["release-cli-agent"],
    now: "2026-01-01T00:01:00Z",
  };
}

function fileManifest() {
  return {
    fileId: "release-cli-file",
    fileName: "release-cli-private-file-canary.txt",
    mimeType: "text/plain",
    relativePath: "release-cli-private-relative-canary",
    totalSize: 16,
    chunkSize: 8,
    chunkCount: 2,
  };
}

function identityFixture(endpointId, identityByte, signingByte, rotationEpoch) {
  return {
    endpointId,
    identityPublicKey: repeatedBase64url(identityByte),
    signingPublicKey: repeatedBase64url(signingByte),
    rotationEpoch,
  };
}

function repeatedBase64url(byte) {
  return Buffer.from(Array.from({ length: 32 }, () => byte & 0xff)).toString("base64url");
}

function runJson(command, args, env) {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    env,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout || `${path.basename(command)} failed`);
  }
  return parseJsonOutput(result.stdout);
}

function parseJsonOutput(output) {
  const start = String(output).indexOf("{");
  assert(start >= 0, "command did not return JSON");
  return JSON.parse(String(output).slice(start));
}

function writeReport(report) {
  assertNoLeak(report, "secure mesh release CLI proof report");
  const target = outputReportPath();
  const buildRoot = path.join(repoRoot, "build");
  const relative = path.relative(buildRoot, target);
  assert(relative && !relative.startsWith("..") && !path.isAbsolute(relative),
    "release CLI proof output escapes the build root");
  atomicWriteReportJson(buildRoot, relative, report);
}

function outputReportPath() {
  return path.resolve(repoRoot, options.report || defaultReportPath);
}

function reportReference() {
  const configured = options.report || defaultReportPath;
  const resolved = path.resolve(repoRoot, configured);
  const relative = path.relative(repoRoot, resolved);
  if (relative && !relative.startsWith("..") && !path.isAbsolute(relative)) {
    return relative;
  }
  return path.basename(resolved);
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
    .replace(/\/tmp\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}

function hostPlatform() {
  if (process.platform === "darwin") return "macos";
  if (process.platform === "win32") return "windows";
  if (process.platform === "linux") return "linux";
  return process.platform;
}

function defaultReleaseCliReportPath(platform) {
  const normalized = String(platform || "").toLowerCase();
  if (normalized === "macos" || normalized.startsWith("macos-")) {
    return physicalReportRefs.macosReleaseCliProof;
  }
  return physicalReportRefs.ubuntuReleaseCliProof;
}

function parseArgs(args) {
  const parsed = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) {
      continue;
    }
    const [rawKey, inlineValue] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    parsed[key] = inlineValue ?? args[index + 1] ?? "";
    if (inlineValue === undefined) {
      index += 1;
    }
  }
  return parsed;
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}
