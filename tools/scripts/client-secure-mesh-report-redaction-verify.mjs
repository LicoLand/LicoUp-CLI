#!/usr/bin/env node
import { existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { loadSecureMeshReportRedactionConfig } from "./lib/secure-mesh-report-redaction-config.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import {
  sha256Buffer,
  stableReadFileSnapshot,
} from "./lib/client-release-artifact-digest.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const args = new Set(process.argv.slice(2));
const releaseProofInputsOnly = args.has("--release-proof-inputs");
const selfTestOnly = args.has("--self-test");
const redactionRunId = String(process.env.LICO_SECURE_MESH_REDACTION_RUN_ID || "").trim();
const redactionConfig = await loadSecureMeshReportRedactionConfig();
const reportPath = releaseProofInputsOnly
  ? redactionConfig.reportOutputs.releaseProofInputs
  : redactionConfig.reportOutputs.default;
const selectedClosureRefs = selectedClosureReportRefs(
  process.env.LICO_CLIENT_RELEASE_CLOSURE_REPORT_REFS_JSON,
  reportPath,
);
const selectedClosureMode = selectedClosureRefs.length > 0;
const selectedModeName = selectedClosureMode
  ? "selectedClosure"
  : (releaseProofInputsOnly ? "releaseProofInputs" : "default");
const selectedMode = selectedClosureMode
  ? { requiredRefs: selectedClosureRefs, optionalRefs: [], deferredGraphRefs: [] }
  : redactionConfig.modes[selectedModeName];
const reportRefPattern = /\b(?:build\/reports|build\/client-cli-vm)\/[A-Za-z0-9._/@-]+\.json\b/gu;
const selectedReportRefs = selectedMode.requiredRefs;
const selectedOptionalReportRefs = selectedMode.optionalRefs;
const selectedDeferredGraphRefs = selectedMode.deferredGraphRefs;

const sensitiveStringFieldName = /^(privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey|sharedSecret|rawSecret|rawProof|secretMaterial|authorization|bearer|password|token)$/iu;
const sensitiveIdentityFieldName = /^(?:signingIdentity|certificate|certificateName|thumbprint|fingerprint|teamId|deviceName|deviceId|deviceIdentifier|udid|ecid|serial|serialNumber|adbSerial|localPath)$|(?:(?:signer|certificate|team).*(?:digest|sha(?:256)?|fingerprint)|(?:digest|sha(?:256)?|fingerprint).*(?:signer|certificate|team))/iu;
const forbiddenStringPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/home\/[^/\s"]+|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["file_url", /file:\/\/\//u],
  ["adb_public_key", /AAAA[0-9A-Za-z+/]{40,}={0,2}/u],
  ["labeled_device_identifier", /\b(?:UDID|ECID|Serial(?:Number)?|DeviceIdentifier)\s*[:=]\s*[A-Za-z0-9-]{8,}\b/u],
  ["adb_device_listing", /\b[A-Za-z0-9_-]{8,}\s+device\b[^\n"]*\b(?:usb:|product:|model:|transport_id:)/u],
  ["jwt", /\b[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\b/u]
]);

function readJsonSnapshotIfPresent(relativePath) {
  const filePath = path.join(repoRoot, relativePath);
  if (!existsSync(filePath)) return null;
  try {
    const snapshot = stableReadFileSnapshot(filePath, { maxBytes: 16 * 1024 * 1024 });
    return {
      payload: JSON.parse(snapshot.bytes.toString("utf8")),
      digest: sha256Buffer(snapshot.bytes),
    };
  } catch (error) {
    throw new Error(`Unable to read redaction input ${relativePath}: ${sanitizeError(error)}`);
  }
}

function safeFieldPath(pathParts) {
  return pathParts
    .map((part) => String(part).replace(/[^A-Za-z0-9_.:[\]-]/gu, "_"))
    .join(".");
}

function isAllowedIdentityKind(key, value) {
  return key === "codeSigningIdentityKind" &&
    (value === "adhoc" || value === "configured" || value === "not_configured");
}

function isRedactedPlaceholder(value) {
  return value === "redacted" ||
    value === "[redacted]" ||
    value === "<redacted>" ||
    value === "Bearer [redacted]" ||
    value === "file:///<redacted>";
}

function isDiagnosticFieldPathString(value, pathParts) {
  const inDiagnosticFieldArray = pathParts.some((part) =>
    /(?:missingFields|weakProofFields|missingTokens|forbiddenTokensPresent)$/iu.test(String(part)),
  );
  if (!inDiagnosticFieldArray) {
    return false;
  }
  // Diagnostic arrays store dotted field-path identifiers, not JWT compact serialization.
  return /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+$/u.test(value);
}

function inspectValue(value, pathParts, hits, file) {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => inspectValue(entry, [...pathParts, `[${index}]`], hits, file));
    return;
  }
  if (value && typeof value === "object") {
    for (const [key, entry] of Object.entries(value)) {
      const nextPath = [...pathParts, key];
      if (typeof entry === "string") {
        inspectStringField(key, entry, nextPath, hits, file);
      }
      inspectValue(entry, nextPath, hits, file);
    }
    return;
  }
  if (typeof value === "string") {
    inspectStringValue(value, pathParts, hits, file);
  }
}

function inspectStringField(key, value, pathParts, hits, file) {
  if (value.length === 0 || isRedactedPlaceholder(value) || isAllowedIdentityKind(key, value)) {
    return;
  }
  if (sensitiveStringFieldName.test(key)) {
    hits.push({
      file,
      path: safeFieldPath(pathParts),
      reason: "sensitive-string-field"
    });
  }
  if (sensitiveIdentityFieldName.test(key)) {
    hits.push({
      file,
      path: safeFieldPath(pathParts),
      reason: "identity-or-local-string-field"
    });
  }
}

function inspectStringValue(value, pathParts, hits, file) {
  if (isRedactedPlaceholder(value)) {
    return;
  }
  for (const [kind, pattern] of forbiddenStringPatterns) {
    if (kind === "jwt" && isDiagnosticFieldPathString(value, pathParts)) {
      continue;
    }
    if (pattern.test(value)) {
      hits.push({
        file,
        path: safeFieldPath(pathParts),
        reason: kind
      });
    }
  }
}

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/home\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/file:\/\/\/[^\s"]+/gu, "file:///<redacted>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 600);
}

function normalizeReportRef(value) {
  const trimmed = String(value || "").trim().replace(/^\.\//u, "");
  if (!trimmed.endsWith(".json")) {
    return "";
  }
  if (trimmed.startsWith("build/reports/") || trimmed.startsWith("build/client-cli-vm/")) {
    return trimmed;
  }
  return "";
}

function selectedClosureReportRefs(rawValue, outputRef) {
  const raw = String(rawValue || "").trim();
  if (!raw) return [];
  if (raw.length > 64 * 1024) {
    throw new Error("Selected client closure redaction refs are oversized");
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("Selected client closure redaction refs are invalid JSON");
  }
  if (!Array.isArray(parsed) || parsed.length === 0 || parsed.length > 64) {
    throw new Error("Selected client closure redaction refs are incomplete");
  }
  const refs = parsed.map((value) => normalizeReportRef(value));
  if (refs.some((ref) => !ref || ref === outputRef) || new Set(refs).size !== refs.length) {
    throw new Error("Selected client closure redaction refs are unsafe or duplicated");
  }
  return refs;
}

function collectLinkedReportRefs(value, refs = new Set()) {
  if (Array.isArray(value)) {
    value.forEach((entry) => collectLinkedReportRefs(entry, refs));
    return refs;
  }
  if (value && typeof value === "object") {
    for (const entry of Object.values(value)) {
      collectLinkedReportRefs(entry, refs);
    }
    return refs;
  }
  if (typeof value !== "string") {
    return refs;
  }
  const matches = value.matchAll(reportRefPattern);
  for (const match of matches) {
    const ref = normalizeReportRef(match[0]);
    if (ref) {
      refs.add(ref);
    }
  }
  return refs;
}

function runSelfTest(config) {
  const releaseProofDeferredGraphRefs = config.modes.releaseProofInputs.deferredGraphRefs;
  const defaultCliVmReportRef = config.modes.default.requiredRefs
    .find((ref) => ref.startsWith("build/client-cli-vm/")) || "";
  const selfTestGraphRef = "build/reports/secure-mesh-self-test-linked-report.json";
  const leakHits = [];
  const selfTestMacHomePath = ["", "Users", "self-test", "secure-mesh-secret.json"].join("/");
  const selfTestLinuxHomePath = ["", "home", "self-test", "secure-mesh-secret.json"].join("/");
  const selfTestPemText = ["-----BEGIN", "SELF", "TEST", "PRIVATE", "KEY-----"].join(" ");
  inspectValue({
    privateKeyBase64url: "self-test-private-key-canary",
    rootKey: "self-test-root-key-canary",
    messageKey: "self-test-message-key-canary",
    rawSecret: "self-test-raw-secret-canary",
    nested: {
      token: ["self", "test", "token", "canary"].join("-"),
      bearer: "self-test-bearer-field-canary",
      password: ["self", "test", "password", "canary"].join("-"),
      deviceIdentifier: "self-test-device-identifier",
      adbSerial: "self-test-adb-serial",
      fingerprint: "self-test-fingerprint",
      [["certificate", "Identity", "Digest"].join("")]: `sha256:${"f".repeat(64)}`,
      localPath: "self-test-local-path"
    },
    bearerText: "Bearer self-test-bearer-canary",
    apiTokenText: ["sk", "selftesttoken000000000000"].join("-"),
    localPathText: selfTestMacHomePath,
    linuxHomePathText: selfTestLinuxHomePath,
    fileUrlText: "file:///self-test/secure-mesh-secret.json",
    pemText: selfTestPemText,
    jwtText: "aaaaaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbb.cccccccccccccccccccc",
    adbPublicKeyText: "AAAA000000000000000000000000000000000000000000000000000000000000",
    labeledDeviceIdentifierText: "UDID: SELFTESTDEVICEID12345",
    adbDeviceListingText: "SELFTEST123456 device usb:1-1 product:self model:self transport_id:1"
  }, [], leakHits, "self-test-leak-fixture");
  const allowedHits = [];
  inspectValue({
    privateKeyBase64url: "redacted",
    authorization: "Bearer [redacted]",
    codeSigningIdentityKind: "adhoc"
  }, [], allowedHits, "self-test-allowed-fixture");
  const hitReasons = new Set(leakHits.map((hit) => hit.reason));
  const linkedRefs = collectLinkedReportRefs({
    reports: [
      ...releaseProofDeferredGraphRefs,
      selfTestGraphRef,
      defaultCliVmReportRef
    ].filter(Boolean)
  });
  const deferredReleaseRefRecognized = releaseProofDeferredGraphRefs.length > 0 &&
    releaseProofDeferredGraphRefs
    .every((ref) => linkedRefs.has(ref));
  const graphDerivedRefRecognized =
    linkedRefs.has(selfTestGraphRef) &&
    !releaseProofDeferredGraphRefs.includes(selfTestGraphRef);
  const requiredReasons = [
    "sensitive-string-field",
    "identity-or-local-string-field",
    "local_path",
    "bearer",
    "token",
    "pem_material",
    "file_url",
    "adb_public_key",
    "labeled_device_identifier",
    "adb_device_listing",
    "jwt"
  ];
  const ok = requiredReasons.every((reason) => hitReasons.has(reason)) &&
    allowedHits.length === 0 &&
    defaultCliVmReportRef &&
    linkedRefs.has(defaultCliVmReportRef) &&
    graphDerivedRefRecognized &&
    deferredReleaseRefRecognized;
  return {
    ok,
    leakFixtureHitCount: leakHits.length,
    allowedFixtureHitCount: allowedHits.length,
    requiredReasons,
    observedReasons: [...hitReasons].sort(),
    graphRefFixtureCount: linkedRefs.size,
    graphDerivedRefRecognized,
    deferredReleaseRefRecognized
  };
}

const checkedAt = new Date().toISOString();
const selfTest = runSelfTest(redactionConfig);
if (selfTestOnly) {
  console.log(JSON.stringify({
    ok: selfTest.ok,
    mode: "self-test",
    verifier: "tools/scripts/client-secure-mesh-report-redaction-verify.mjs",
    leakFixtureHitCount: selfTest.leakFixtureHitCount,
    allowedFixtureHitCount: selfTest.allowedFixtureHitCount,
    observedReasonCount: selfTest.observedReasons.length,
    graphRefFixtureCount: selfTest.graphRefFixtureCount,
    graphDerivedRefRecognized: selfTest.graphDerivedRefRecognized,
    deferredReleaseRefRecognized: selfTest.deferredReleaseRefRecognized
  }, null, 2));
  if (!selfTest.ok) {
    process.exitCode = 1;
  }
  process.exit();
}
const refs = selectedReportRefs;
const seedRefs = new Set(refs);
const optionalRefSet = new Set(selectedOptionalReportRefs);
const selectedDeferredGraphRefSet = new Set(selectedDeferredGraphRefs);
const queuedRefs = refs.map(normalizeReportRef).filter(Boolean);
const queuedRefSet = new Set(queuedRefs);
const graphDerivedRefs = [];
const deferredGraphRefs = [];
const scannedRefs = [];
const scannedOptionalRefs = [];
const scannedRefDigests = [];
const missingRefs = [];
const missingOptionalRefs = [];
const hits = [];

for (let index = 0; index < queuedRefs.length; index += 1) {
  const ref = queuedRefs[index];
  if (ref === reportPath) {
    continue;
  }
  const snapshot = readJsonSnapshotIfPresent(ref);
  if (!snapshot) {
    missingRefs.push(ref);
    continue;
  }
  const report = snapshot.payload;
  scannedRefs.push(ref);
  scannedRefDigests.push({
    ref,
    sha256: snapshot.digest
  });
  inspectValue(report, [], hits, ref);
  if (selectedClosureMode) continue;
  const linkedRefs = collectLinkedReportRefs(report);
  for (const linkedRef of linkedRefs) {
    if (linkedRef === reportPath || queuedRefSet.has(linkedRef)) {
      continue;
    }
    if (selectedDeferredGraphRefSet.has(linkedRef)) {
      if (!deferredGraphRefs.includes(linkedRef)) {
        deferredGraphRefs.push(linkedRef);
      }
      continue;
    }
    if (optionalRefSet.has(linkedRef)) {
      continue;
    }
    queuedRefSet.add(linkedRef);
    queuedRefs.push(linkedRef);
    if (!seedRefs.has(linkedRef) && !optionalRefSet.has(linkedRef)) {
      graphDerivedRefs.push(linkedRef);
    }
  }
}

if (selectedOptionalReportRefs.length > 0) {
  for (const ref of selectedOptionalReportRefs) {
    if (queuedRefSet.has(ref) || ref === reportPath) {
      continue;
    }
    const snapshot = readJsonSnapshotIfPresent(ref);
    if (!snapshot) {
      missingOptionalRefs.push(ref);
      continue;
    }
    const report = snapshot.payload;
    scannedRefs.push(ref);
    scannedOptionalRefs.push(ref);
    scannedRefDigests.push({
      ref,
      sha256: snapshot.digest
    });
    inspectValue(report, [], hits, ref);
  }
}

const reportCoverageComplete = missingRefs.length === 0;
// The default command is the local no-plaintext gate. Physical-host reports
// may not exist yet, but every report that does exist must be scanned and
// clean. A selected release closure remains strict about complete coverage.
const ok = selfTest.ok === true &&
  hits.length === 0 &&
  scannedRefs.length > 0 &&
  (!selectedClosureMode || reportCoverageComplete);
const report = {
  ok,
  schemaVersion: "licomesh.secure-mesh.report-redaction-verifier.v1",
  verifier: "tools/scripts/client-secure-mesh-report-redaction-verify.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-report-redaction-verify.mjs",
  generatedAt: checkedAt,
  ...optionalReleaseInvocationBinding(),
  checkedAt,
  mode: selectedClosureMode
    ? "selected-client-release-closure"
    : (releaseProofInputsOnly ? "release-proof-inputs" : "secure-mesh-e2ee-reports"),
  redactionConfig: {
    ref: redactionConfig.configRef,
    schemaVersion: redactionConfig.schemaVersion,
    mode: selectedModeName,
    requiredRefCount: selectedReportRefs.length,
    optionalRefCount: selectedOptionalReportRefs.length,
    deferredGraphRefCount: selectedDeferredGraphRefs.length
  },
  redactionRunId,
  diagnosticStatus: ok ? "passed" : "failed",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawLocalPathIncluded: false,
  rawIdentityMaterialIncluded: false,
  selfTest,
  scannedRefs,
  scannedOptionalRefs,
  scannedRefDigests,
  graphDerivedRefs,
  deferredGraphRefs,
  missingRefs,
  missingOptionalRefs,
  hits,
  summary: {
    reportRedactionReady: ok,
    reportCoverageComplete,
    selfTestReady: selfTest.ok === true,
    scannedReportCount: scannedRefs.length,
    scannedOptionalReportCount: scannedOptionalRefs.length,
    scannedRefDigestCount: scannedRefDigests.length,
    graphDerivedReportCount: graphDerivedRefs.length,
    deferredGraphRefCount: deferredGraphRefs.length,
    missingReportCount: missingRefs.length,
    missingOptionalReportCount: missingOptionalRefs.length,
    hitCount: hits.length,
    redactionRunIdPresent: redactionRunId.length > 0,
    releaseProofInputsOnly
    ,selectedClosureMode
  }
};

atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  report,
);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  mode: report.mode,
  selfTestReady: selfTest.ok === true,
  redactionRunIdPresent: redactionRunId.length > 0,
  scannedReportCount: scannedRefs.length,
  scannedOptionalReportCount: scannedOptionalRefs.length,
  scannedRefDigestCount: scannedRefDigests.length,
  graphDerivedReportCount: graphDerivedRefs.length,
  deferredGraphRefCount: deferredGraphRefs.length,
  missingReportCount: missingRefs.length,
  missingOptionalReportCount: missingOptionalRefs.length,
  hitCount: hits.length
}, null, 2));

if (!ok) {
  process.exitCode = 1;
}
