#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import {
  linuxSecretServiceProbeFixture,
  reduceLinuxSecretServiceProbe,
  validateLinuxSecretServiceProbe,
  validateLinuxSecretServiceProjection
} from "./lib/secure-mesh-linux-secret-service-capability.mjs";
import { atomicWriteReportJson, resolveSafeReportPath } from "./lib/safe-report-io.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const defaultReportPath = physicalReportRefs.ubuntuLinuxAdaptiveCustodyProof;
const privateFieldNames = Object.freeze([
  "privateKeyBase64url",
  "signingKeyBase64url",
  "signedPrekeyPrivateKeyBase64url",
  "oneTimePrekeyPrivateKeyBase64url",
  "pairingSecretBase64url",
  "sessionKey",
  "rootKey",
  "chainKey",
  "messageKey"
]);
const leakPatterns = Object.freeze([
  ["local_path", /\/(?:Users|home|private|tmp|var\/folders)\/|[A-Za-z]:\\/u],
  ["dbus_address", /(?:^|["'\s])unix:(?:path|abstract)=/iu],
  ["secret_service_object_path", /\/org\/freedesktop\/(?:DBus|secrets)(?:\/|$)/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", new RegExp(
    `"(?:${privateFieldNames.join("|")})"\\s*:\\s*"[^"]{8,}"`,
    "u"
  )]
]);
const options = parseArgs(process.argv.slice(2));

if (options.selfTest) {
  try {
    console.log(JSON.stringify(runSelfTest(), null, 2));
  } catch (error) {
    console.error(JSON.stringify({ ok: false, error: sanitizeError(error) }, null, 2));
    process.exitCode = 1;
  }
} else if (options.inputReport) {
  try {
    const result = validateInputReport(options.inputReport, options.expectStrategy || "os_secure_store");
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    console.error(JSON.stringify({ ok: false, error: sanitizeError(error) }, null, 2));
    process.exitCode = 1;
  }
} else {
  const tempDir = mkdtempSync(path.join(os.tmpdir(), "lico-adaptive-custody-proof-"));
  try {
    const report = runProof(tempDir);
    writeReport(report);
    console.log(JSON.stringify({
      ok: report.ok,
      report: report.report,
      platform: report.platform,
      custodyStrategy: report.capability.custodyStrategy,
      restartSemantics: report.capability.restartSemantics,
      adaptiveFallbackReady: report.summary.adaptiveFallbackReady,
      ordinaryFilePersistenceAbsent: report.summary.ordinaryFilePersistenceAbsent
    }, null, 2));
    if (!report.ok) process.exitCode = 1;
  } catch (error) {
    const report = failureReport(error);
    writeReport(report);
    console.error(JSON.stringify({
      ok: false,
      report: report.report,
      error: report.failure.code
    }, null, 2));
    process.exitCode = 1;
  } finally {
    rmSync(tempDir, { recursive: true, force: true });
  }
}

function runProof(tempDir) {
  const cli = options.cli || process.env.LICO_RELEASE_CLI || "";
  assert(cli.trim(), "release CLI path is required");
  const platform = options.platform || process.env.LICO_RELEASE_CLI_PLATFORM || hostPlatform();
  assert(platform.includes("linux"), "adaptive Secret Service fallback proof requires a Linux target");
  const result = spawnSync(cli, ["mobile", "relay", "e2ee", "secret-store-self-test"], {
    cwd: repoRoot,
    env: unavailableSecretServiceEnv(tempDir),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024
  });
  const output = `${result.stdout || ""}\n${result.stderr || ""}`;
  const payload = parseJsonOutput(result.stdout);
  const probe = payload?.secretServiceProbe || payload?.secretStore?.secretServiceProbe;
  const capabilityReport = payload?.secretStore?.capabilityReport || payload?.capabilityReport;
  const projection = validateLinuxSecretServiceProjection(probe, capabilityReport);
  const selectedBackend = String(
    payload?.selectedBackend || payload?.secretStore?.selectedBackend || payload?.backend || ""
  );
  const restartProof = payload?.restartProof || {};
  const observedOrdinaryArtifacts = countOrdinaryFileSecretArtifacts(tempDir);
  const outputHasLeak = containsLeak(output);
  const summary = {
    commandReturnedJson: result.status === 0 && Boolean(payload),
    independentProbeFactsComplete: Boolean(validateLinuxSecretServiceProbe(probe)),
    exactCapabilityProjection: projection.ok === true,
    memoryOnlySelected: projection.custodyStrategy === "memory_only_ephemeral" &&
      selectedBackend === "memory-only-ephemeral",
    restartRequiresRePairRekey:
      projection.restartSemantics === "re_pair_rekey_after_restart" &&
      restartProof.staleSessionRestorationRejected === true &&
      restartProof.rePairRekeyRequired === true,
    noninteractivePromptSuppressed:
      probe.interaction === "noninteractive" && probe.prompt !== "required",
    ordinaryFilePersistenceAbsent:
      observedOrdinaryArtifacts === 0 &&
      Number(payload?.ordinaryFileSecretArtifactCount ?? -1) === 0 &&
      payload?.secretStore?.unsafePersistenceDetected !== true,
    portableFallbackRejected: selectedBackend !== "portable" && selectedBackend !== "ordinary-file",
    rawPrivateMaterialAbsent: !rawPrivateMaterialPattern().test(output),
    outputRedacted: !output.trim() || !outputHasLeak
  };
  summary.adaptiveFallbackReady = Object.values(summary).every((value) => value === true);

  return {
    schemaVersion: "licomesh.secure-mesh.linux-adaptive-custody-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-linux-adaptive-custody-proof.mjs",
    generatedAt: new Date().toISOString(),
    report: reportReference(),
    reportLeakScan: true,
    ok: summary.adaptiveFallbackReady,
    platform,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "linux-secret-service-unavailable-adaptive-custody",
    probe,
    capability: {
      catalogDigest: projection.catalogDigest,
      custodyStrategy: projection.custodyStrategy,
      restartSemantics: projection.restartSemantics
    },
    observed: {
      commandCompleted: result.status === 0,
      backend: selectedBackend,
      ordinaryFileSecretArtifactCount: observedOrdinaryArtifacts,
      staleSessionRestorationRejected: restartProof.staleSessionRestorationRejected === true,
      rePairRekeyRequired: restartProof.rePairRekeyRequired === true
    },
    summary
  };
}

function validateInputReport(inputReport, expectedStrategy) {
  const payload = JSON.parse(readFileSync(path.resolve(inputReport), "utf8"));
  return validateInputPayload(payload, expectedStrategy);
}

function validateInputPayload(payload, expectedStrategy) {
  assert(expectedStrategy === "os_secure_store" || expectedStrategy === "memory_only_ephemeral",
    "input report expected strategy is invalid");
  const probe = payload?.secretServiceProbe || payload?.secretStore?.secretServiceProbe;
  const capabilityReport = payload?.secretStore?.capabilityReport || payload?.capabilityReport;
  const projection = validateLinuxSecretServiceProjection(probe, capabilityReport);
  const backend = String(
    payload?.selectedBackend || payload?.secretStore?.selectedBackend || payload?.backend || ""
  );
  const persistent = expectedStrategy === "os_secure_store";
  assert(payload?.ok === true && payload?.selfTestPassed === true,
    "adaptive custody input self-test did not pass");
  assert(projection.custodyStrategy === expectedStrategy,
    "adaptive custody input strategy did not match expectation");
  assert(backend === (persistent ? "linux-secret-service-keyring" : "memory-only-ephemeral"),
    "adaptive custody input backend did not match exact strategy");
  assert(payload?.portableConfigPrivateMaterialRedacted === true,
    "adaptive custody input persisted private material in portable config");
  assert(payload?.secretStore?.unsafePersistenceDetected !== true,
    "adaptive custody input reported unsafe persistence");
  if (persistent) {
    assert(payload?.secretStore?.allPrivateKeysInSelectedCustody === true,
      "OS custody input did not bind every Mobile Relay private key to selected platform custody");
    assert(payload?.secretStore?.pairingSecretInSelectedCustody === true,
      "OS custody input did not bind the pairing secret to selected platform custody");
    assert(Number(payload?.ordinaryFileSecretArtifactCount ?? -1) === 0,
      "OS custody input found an ordinary-file secret artifact");
    assert(payload?.sharedSecretClassPersistenceReady === true,
      "OS custody input omitted shared secret-class persistence proof");
    assert(payload?.sharedSecretClassRoundTrip?.allClassesStored === true &&
      payload?.sharedSecretClassRoundTrip?.allClassesDeleted === true &&
      payload?.sharedSecretClassRoundTrip?.rawSecretMaterialIncluded !== true,
    "OS custody input shared secret-class proof was incomplete");
  } else {
    assert(projection.restartSemantics === "re_pair_rekey_after_restart",
      "memory-only custody input omitted restart re-pair/rekey semantics");
  }
  return {
    ok: true,
    schemaVersion: "licomesh.secure-mesh.linux-adaptive-custody-input-validation.v1",
    custodyStrategy: projection.custodyStrategy,
    restartSemantics: projection.restartSemantics,
    exactCapabilityProjection: true,
    portablePrivateMaterialAbsent: true,
    sharedSecretClassPersistenceVerified: persistent,
    allPrivateKeysBoundToPlatform: persistent,
    pairingSecretBoundToPlatform: persistent
  };
}

function unavailableSecretServiceEnv(tempDir) {
  return {
    ...process.env,
    LICOUP_HOME: path.join(tempDir, "portable"),
    LICO_MOBILE_RELAY_NATIVE_SECRET_STORE: "auto",
    LICO_SECURE_MESH_SECRET_STORE_INTERACTION: "noninteractive",
    DBUS_SESSION_BUS_ADDRESS: `unix:path=${path.join(tempDir, "unavailable-session-bus")}`,
    GNOME_KEYRING_CONTROL: "",
    SSH_AUTH_SOCK: ""
  };
}

function runSelfTest() {
  const fallbackScenarios = [
    "absent",
    "session_failure",
    "no_default_collection",
    "locked",
    "prompt_required",
    "service_disappeared"
  ];
  const unlocked = reduceLinuxSecretServiceProbe(linuxSecretServiceProbeFixture("unlocked"));
  assert(unlocked.capabilityReport.custody.strategy === "os_secure_store",
    "unlocked Secret Service fixture did not select OS custody");
  assert(unlocked.capabilityReport.enabled.includes("custody.linux_secret_service"),
    "unlocked Secret Service fixture omitted the exact Linux capability");
  assert(!unlocked.capabilityReport.enabled.includes("custody.software_backed") &&
    unlocked.capabilityReport.unverified.includes("custody.software_backed"),
  "Secret Service availability incorrectly inferred software-backed custody");
  const unlockedInput = {
    ok: true,
    selfTestPassed: true,
    backend: "linux-secret-service-keyring",
    secretServiceProbe: linuxSecretServiceProbeFixture("unlocked"),
    portableConfigPrivateMaterialRedacted: true,
    secretStore: {
      selectedBackend: "linux-secret-service-keyring",
      capabilityReport: unlocked.capabilityReport,
      unsafePersistenceDetected: false,
      allPrivateKeysInSelectedCustody: true,
      pairingSecretInSelectedCustody: true
    },
    ordinaryFileSecretArtifactCount: 0,
    sharedSecretClassPersistenceReady: true,
    sharedSecretClassRoundTrip: {
      allClassesStored: true,
      allClassesDeleted: true,
      rawSecretMaterialIncluded: false
    }
  };
  assert(validateInputPayload(unlockedInput, "os_secure_store").ok === true,
    "unlocked OS-store input report contract was rejected");

  for (const scenario of fallbackScenarios) {
    const projection = reduceLinuxSecretServiceProbe(linuxSecretServiceProbeFixture(scenario));
    assert(projection.capabilityReport.custody.strategy === "memory_only_ephemeral",
      `${scenario} fixture did not select memory-only custody`);
    assert(projection.capabilityReport.custody.restartSemantics === "re_pair_rekey_after_restart",
      `${scenario} fixture omitted restart re-pair/rekey semantics`);
    assert(!projection.capabilityReport.enabled.includes("custody.os_secure_store"),
      `${scenario} fixture retained unavailable OS-store authority`);
    const expectedReason = {
      absent: "linux_secret_service_api_absent",
      session_failure: "linux_secret_service_session_failed",
      no_default_collection: "linux_secret_service_default_collection_absent",
      locked: "linux_secret_service_collection_locked",
      prompt_required: "linux_secret_service_prompt_required",
      service_disappeared: "linux_secret_service_disappeared"
    }[scenario];
    assert(projection.capabilityReport.reasons["custody.os_secure_store"] === expectedReason,
      `${scenario} fixture emitted an unstable platform reason code`);
  }

  const promptRequired = linuxSecretServiceProbeFixture("prompt_required");
  assert(promptRequired.interaction === "noninteractive" && promptRequired.prompt === "required",
    "prompt-required fixture did not preserve independent prompt facts");

  assertThrows(() => validateLinuxSecretServiceProbe({
    ...linuxSecretServiceProbeFixture("unlocked"),
    dbusAddress: "[redacted]"
  }), "Linux probe accepted a forbidden DBus field");
  assertThrows(() => validateLinuxSecretServiceProbe({
    ...linuxSecretServiceProbeFixture("unlocked"),
    ordinaryFilePersistence: "detected"
  }), "Linux probe accepted ordinary-file secret persistence");
  assertThrows(() => validateLinuxSecretServiceProbe({
    ...linuxSecretServiceProbeFixture("unlocked"),
    api: "/org/freedesktop/secrets"
  }), "Linux probe accepted a Secret Service object path");

  return {
    ok: true,
    schemaVersion: "licomesh.secure-mesh.linux-adaptive-custody-self-test.v1",
    scenarioCount: fallbackScenarios.length + 1,
    independentFactModelReady: true,
    exactSharedReducerProjectionReady: true,
    noninteractivePromptSuppressionReady: true,
    memoryOnlyRestartSemanticsReady: true,
    ordinaryFileFallbackRejected: true,
    runtimeIdentityRedactionReady: true
  };
}

function countOrdinaryFileSecretArtifacts(root) {
  const stack = [root];
  let count = 0;
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const child = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(child);
        continue;
      }
      if (!entry.isFile() || statSync(child).size > 1024 * 1024) continue;
      const content = readFileSync(child);
      if (privateFieldNames.some((field) => content.includes(Buffer.from(`"${field}"`, "utf8")))) {
        count += 1;
      }
    }
  }
  return count;
}

function failureReport(error) {
  return {
    schemaVersion: "licomesh.secure-mesh.linux-adaptive-custody-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-linux-adaptive-custody-proof.mjs",
    generatedAt: new Date().toISOString(),
    report: reportReference(),
    reportLeakScan: true,
    ok: false,
    platform: options.platform || hostPlatform(),
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "linux-secret-service-unavailable-adaptive-custody",
    failure: {
      code: "linux_adaptive_custody_proof_failed",
      sanitized: sanitizeError(error)
    },
    summary: {
      adaptiveFallbackReady: false,
      commandReturnedJson: false,
      independentProbeFactsComplete: false,
      exactCapabilityProjection: false,
      memoryOnlySelected: false,
      restartRequiresRePairRekey: false,
      noninteractivePromptSuppressed: false,
      ordinaryFilePersistenceAbsent: false,
      portableFallbackRejected: false,
      rawPrivateMaterialAbsent: false,
      outputRedacted: false
    }
  };
}

function parseJsonOutput(output) {
  const text = String(output || "");
  const start = text.indexOf("{");
  if (start < 0) return null;
  return JSON.parse(text.slice(start));
}

function writeReport(report) {
  assertNoLeak(report, "Linux adaptive custody proof report");
  const reportRef = reportReference();
  const target = resolveSafeReportPath(repoRoot, reportRef);
  mkdirSync(path.dirname(target), { recursive: true });
  atomicWriteReportJson(repoRoot, reportRef, report);
}

function outputReportPath() {
  return path.resolve(repoRoot, options.report || defaultReportPath);
}

function reportReference() {
  const configured = options.report || defaultReportPath;
  const resolved = path.resolve(repoRoot, configured);
  const relative = path.relative(repoRoot, resolved);
  if (relative && !relative.startsWith("..") && !path.isAbsolute(relative)) return relative;
  return path.basename(resolved);
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) throw new Error(`${label} contains sensitive data: ${kind}`);
  }
}

function containsLeak(value) {
  return leakPatterns.some(([, pattern]) => pattern.test(String(value || "")));
}

function rawPrivateMaterialPattern() {
  return new RegExp(`"(?:${privateFieldNames.join("|")})"\\s*:\\s*"[^"]{8,}"`, "u");
}

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/(?:Users|home)\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/(?:private\/var\/folders|tmp)\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/(?:^|["'\s])unix:(?:path|abstract)=[^\s"]+/giu, " [runtime-address-redacted]")
    .replace(/\/org\/freedesktop\/(?:DBus|secrets)\S*/gu, "[runtime-object-redacted]")
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

function parseArgs(args) {
  const parsed = { selfTest: false };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--self-test") {
      parsed.selfTest = true;
      continue;
    }
    if (!arg.startsWith("--")) continue;
    const [rawKey, inlineValue] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    parsed[key] = inlineValue ?? args[index + 1] ?? "";
    if (inlineValue === undefined) index += 1;
  }
  return parsed;
}

function assertThrows(operation, message) {
  let threw = false;
  try {
    operation();
  } catch {
    threw = true;
  }
  assert(threw, message);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
