import {
  classifyLinuxEvidenceValidationFailure,
  classifyLinuxVmProducerFailure,
  createLinuxVmPackageFailureRecord,
  LinuxEvidenceValidationError,
  linuxEvidencePrivacyRecord,
  linuxVmReceiptWriteFailure,
  linuxNodeMatrixSchema,
  validateLinuxNodeMatrixReport,
  validateLinuxVmPackageReceipt,
} from "../lib/secure-mesh-linux-evidence.mjs";
import {
  loadCapabilityCatalog,
  reduceCapabilityFacts,
} from "../lib/secure-mesh-capability-report.mjs";
import { SAFE_REPORT_WRITE_STAGES } from "../lib/safe-report-io.mjs";
import { assert } from "./assert.mjs";

export function runSelfTest() {
  const digest = `sha256:${"a".repeat(64)}`;
  const capabilityReport = fixtureCapabilityReport();
  const sourceBinding = {
    sourceStateDigest: digest,
    sourceStateDigestProvenance: "vm-orchestrator-verified",
    archiveDigest: `sha256:${"b".repeat(64)}`,
    bundleManifestDigest: `sha256:${"c".repeat(64)}`,
    nativeClientDigest: `sha256:${"f".repeat(64)}`,
    stale: false
  };
  const vm = {
    schema: "licomesh.secure-mesh.linux-vm-package-receipt",
    schemaVersion: 2,
    ok: true,
    producer: "linux-vm-package-receipt",
    generatedAt: "2030-01-01T00:00:00.000Z",
    closureChallengeDigest: `sha256:${"d".repeat(64)}`,
    invocationNonceDigest: `sha256:${"e".repeat(64)}`,
    productVersion: "1.2.3",
    buildNumber: 7,
    artifactKind: "linux-vm-installed-client",
    target: "ubuntu-linux-arm64",
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    sourceBinding,
    package: {
      format: "tar.gz",
      layoutClasses: [
        "desktop_executable",
        "native_sidecar",
        "flutter_assets",
        "package_metadata"
      ],
      executableCount: 2,
      signaturePresent: true,
      validationSignature: true,
      signatureVerified: true,
      archiveDigestVerified: true,
      bundleManifestDigestVerified: true,
      installedFromArchive: true
    },
    session: {
      kind: "x11_virtual_display",
      clientStarted: true,
      visibleWindow: true,
      interactionSmoke: true,
      boundedShutdown: true
    },
    smoke: { cliTargetScan: true, guiSession: true, exactCapabilitySchema: true },
    capabilityReport,
    privacy: linuxEvidencePrivacyRecord(),
    nonBlockingDistributionGuidance: {
      blocking: false,
      storeListingStatus: "not-configured",
      platformSigningStatus: "not-configured",
      publicDownloadStatus: "not-configured",
      updateChannelStatus: "not-configured",
      rollbackChannelStatus: "not-configured",
    },
    summary: {
      currentSourceArchive: true,
      installReceiptReady: true,
      sessionLaunchReady: true,
      smokeReady: true,
      privacyReady: true
    }
  };
  validateLinuxVmPackageReceipt(vm, digest);
  const matrix = {
    schema: linuxNodeMatrixSchema,
    schemaVersion: 1,
    ok: true,
    producer: "linux-node-matrix",
    artifactKind: "linux-current-client-node-matrix",
    target: "ubuntu-linux-arm64",
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    sourceBinding,
    runtime: {
      kind: "isolated_linux_containers",
      nodeCount: 3,
      currentClientArchive: true,
      publicOperationsOnly: true,
      eventDrivenReadiness: true
    },
    isolation: {
      participantLabels: ["linux-a", "linux-b", "linux-c"],
      distinctStateRoots: true,
      noSharedSecretVolume: true,
      uniquePublicIdentityCount: 3,
      crossNodeStateReadRejected: true,
      containerIsolation: true
    },
    pairwise: {
      exchangeCount: 3,
      allNodesParticipated: true,
      secureSessionsEstablished: true,
      opaqueRelay: true,
      relayPlaintextObserved: false,
      relayCiphertextIncludedInReport: false
    },
    restart: {
      restartedParticipant: "linux-a",
      restartedProcessCount: 1,
      restartRequiresRePairRekey: true,
      unaffectedParticipantCount: 2,
      postRestartExchangeReady: true,
      stateContaminationDetected: false
    },
    teardown: {
      bounded: true,
      nodeCount: 3,
      allProcessesStopped: true,
      allContainersRemoved: true,
      ephemeralStateRemoved: true
    },
    capabilityReport,
    privacy: linuxEvidencePrivacyRecord(),
    summary: {
      currentSourceNodes: true,
      isolationReady: true,
      pairwiseReady: true,
      restartIsolationReady: true,
      teardownReady: true,
      privacyReady: true
    }
  };
  validateLinuxNodeMatrixReport(matrix, digest);
  let privacyRejected = false;
  try {
    validateLinuxNodeMatrixReport({ ...matrix, runtimeId: "forbidden" }, digest);
  } catch {
    privacyRejected = true;
  }
  assert(privacyRejected, "Linux node matrix exact schema accepted a runtime identifier");
  let staleRejected = false;
  try {
    validateLinuxVmPackageReceipt(vm, `sha256:${"d".repeat(64)}`);
  } catch (error) {
    staleRejected = classifyLinuxEvidenceValidationFailure(error).ruleId ===
      "linux_vm_expected_source_digest_match";
  }
  assert(staleRejected, "Linux VM receipt accepted stale source binding");
  for (const [name, ruleId, candidate] of [
    ["missing_challenge", "linux_vm_closure_challenge_digest_valid",
      { ...vm, closureChallengeDigest: "" }],
    ["missing_invocation_nonce", "linux_vm_invocation_nonce_digest_valid",
      { ...vm, invocationNonceDigest: "" }],
    ["wrong_product_version", "linux_vm_product_version_match",
      { ...vm, productVersion: "9.9.9" }],
    ["wrong_build_number", "linux_vm_build_number_match", { ...vm, buildNumber: 8 }],
    ["blocking_distribution_guidance", "linux_vm_distribution_guidance_non_blocking",
      { ...vm, nonBlockingDistributionGuidance: { blocking: true } }],
    ["unbounded_shutdown", "linux_vm_session_bounded_shutdown_ready", {
      ...vm,
      session: { ...vm.session, boundedShutdown: false },
    }],
    ["privacy_value", "linux_vm_privacy_value_scan_clean", {
      ...vm,
      target: ["", "tmp", "private-fixture"].join("/"),
    }],
  ]) {
    let rejected = false;
    try {
      validateLinuxVmPackageReceipt(candidate, digest, "1.2.3", 7);
    } catch (error) {
      const failure = classifyLinuxEvidenceValidationFailure(error);
      rejected = failure.ruleId === ruleId &&
        ["artifact", "binding", "capability", "privacy", "readiness", "schema", "session"]
          .includes(failure.category);
    }
    assert(rejected, `Linux VM receipt accepted ${name}`);
  }
  const fallback = classifyLinuxEvidenceValidationFailure(
    new Error(["private", "dynamic", "value"].join("-")),
    "linux_vm_receipt_validation_unclassified",
  );
  assert(fallback.ruleId === "linux_vm_receipt_validation_unclassified" &&
    fallback.category === "schema" &&
    JSON.stringify(fallback).includes("dynamic") === false,
  "Linux VM validation fallback exposed an unsafe dynamic failure");
  let internalOperationFailure;
  try {
    validateLinuxVmPackageReceipt(new Proxy({}, {
      ownKeys() {
        throw new Error(["private", "dynamic", "validator", "value"].join("-"));
      },
    }), digest);
  } catch (error) {
    internalOperationFailure = classifyLinuxEvidenceValidationFailure(error);
  }
  assert(internalOperationFailure?.ruleId ===
    "linux_vm_validator_internal_operation_failed" &&
    internalOperationFailure.category === "schema" &&
    JSON.stringify(internalOperationFailure).includes("dynamic") === false,
  "Linux VM validator leaked an untagged internal operation failure");
  const failureRecord = createLinuxVmPackageFailureRecord("receipt_validation", fallback);
  const failureText = JSON.stringify(failureRecord);
  assert(failureRecord.validationRuleId === "linux_vm_receipt_validation_unclassified" &&
    failureRecord.failureCategory === "schema" &&
    failureRecord.redacted === true && failureRecord.rawPrivateMaterialIncluded === false &&
    failureRecord.rawPlaintextIncluded === false &&
    !failureText.includes("dynamic") && !failureText.includes("nonce") &&
    !failureText.includes("challenge") && !failureText.includes(["", "tmp", "private"].join("/")),
  "Linux VM failure receipt exposed dynamic validation data");
  const plainWriteFailure = classifyLinuxVmProducerFailure(
    "receipt_write",
    new Error(["private", "write", "value"].join("-")),
  );
  const taggedWriteFailure = classifyLinuxVmProducerFailure(
    "receipt_write",
    new LinuxEvidenceValidationError(
      "linux_vm_receipt_write_atomic_publish_failed",
      "producer",
    ),
  );
  const writeFailureRecord = createLinuxVmPackageFailureRecord(
    "receipt_write",
    taggedWriteFailure,
  );
  assert(plainWriteFailure.ruleId === "linux_vm_producer_receipt_write_failed" &&
    plainWriteFailure.category === "producer" &&
    taggedWriteFailure.ruleId === "linux_vm_receipt_write_atomic_publish_failed" &&
    taggedWriteFailure.category === "producer" &&
    writeFailureRecord.phase === "receipt_write" &&
    JSON.stringify(writeFailureRecord).includes("private") === false,
  "Linux VM receipt write failure masqueraded as validator failure");
  for (const stage of SAFE_REPORT_WRITE_STAGES) {
    const stageFailure = classifyLinuxVmProducerFailure(
      "receipt_write",
      linuxVmReceiptWriteFailure(stage),
    );
    assert(stageFailure.ruleId === `linux_vm_receipt_write_${stage}_failed` &&
      stageFailure.category === "producer",
    `Linux VM receipt write stage mapping failed: ${stage}`);
  }
  return {
    ok: true,
    exactCapabilitySchemaReady: true,
    exactEvidenceSchemaReady: true,
    staleSourceRejected: staleRejected,
    runtimeIdentityRejected: true,
    boundedTeardownRequired: true
    ,stableValidationRuleIdsReady: true
    ,dynamicFailureValuesIncluded: false
    ,safeFailureReceiptReady: true
    ,internalOperationFailureTagged: true
    ,receiptWritePhaseIsolated: true
    ,receiptWriteStageCount: SAFE_REPORT_WRITE_STAGES.length
  };
}

function fixtureCapabilityReport() {
  const catalog = loadCapabilityCatalog();
  const facts = catalog.order
    .map((id) => catalog.byId.get(id))
    .filter((definition) => definition.mandatory && !definition.derived)
    .map((definition) => ({ capability: definition.id, state: "supported" }));
  return reduceCapabilityFacts(facts, catalog);
}
