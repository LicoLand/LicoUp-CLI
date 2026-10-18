import { createHash, randomUUID } from "node:crypto";
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { CONDITIONAL_CHECK_IDS, CONTRACT_VERSION, CORE_CHECK_IDS, EVIDENCE_SCHEMA_VERSION, adapterManifestDigestFor, adapterEvidenceDigestFor, driverInventoryDigestFor, packagedAgentIds, registryDigestFor } from "../reducer-facade.mjs";
import { productContinuityBindingDigest } from "../../../../../../tools/scripts/lib/agent-conversation-release-binding.mjs";
import { dispatchLaneHarnessVersion, driversInventoryPath, evidenceManifestPath, packagingRegistryPath, strictRoundCount } from "./constants.mjs";
import { digest, requireFact } from "./errors.mjs";

export function assertEvidenceHygiene(evidence) {
  const text = JSON.stringify(evidence);
  const forbidden = [
    ["", "Users", ""].join("/"),
    ["", "home", ""].join("/"),
    "BEGIN PRIVATE",
    "password",
    "Authorization:",
  ];
  for (const needle of forbidden) {
    requireFact(!text.includes(needle), "evidence_hygiene_failed");
  }
}

export function createSanitizedSelfTestEvidenceReceipt(probeStamp) {
  // Fixture/self-test evidence is diagnostic only. It must never mutate the
  // checked-in live-evidence authority or make the readiness reducer stale.
  let existingAdapters = [];
  try {
    const existing = JSON.parse(readFileSync(evidenceManifestPath, "utf8"));
    if (Array.isArray(existing?.adapters)) {
      existingAdapters = existing.adapters;
    }
  } catch {
    existingAdapters = [];
  }
  const evidence = {
    schemaVersion: "v0.0.1:client-agent-conversation-parity-evidence-1",
    contractVersion: "CL-06",
    harnessVersion: dispatchLaneHarnessVersion,
    toolVersionClass: probeStamp.toolVersionClass || dispatchLaneHarnessVersion,
    generatedAt: probeStamp.generatedAt || new Date().toISOString(),
    adapters: existingAdapters,
  };
  assertEvidenceHygiene(evidence);
  assertEvidenceHygiene(probeStamp);
  return {
    persisted: false,
    harnessVersion: evidence.harnessVersion,
    generatedAt: evidence.generatedAt,
    toolVersionClass: evidence.toolVersionClass,
    laneFamiliesCovered: probeStamp.laneFamiliesCovered,
    coreProbesCovered: probeStamp.coreProbesCovered,
    adapterCount: existingAdapters.length,
  };
}

export function binaryDigest(binaryPath) {
  if (!binaryPath || !existsSync(binaryPath)) {
    return `sha256:${"0".repeat(64)}`;
  }
  const bytes = readFileSync(binaryPath);
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

export function conditionalResultForSupported(proven) {
  if (proven === true) return "pass";
  if (proven === false) return "fail";
  return "unverified";
}

export function conditionalChecksFromMatrix(matrix, probes = {}) {
  const supportedStreaming = matrix?.streaming === true;
  const supportedApprovals = matrix?.approvals === true;
  const supportedMultimodal = matrix?.multimodal === true;
  const supportedInterruptSteer = matrix?.interruptSteer === true;
  const supportedUsage = matrix?.usageStatus === true;
  const supportedStructured = matrix?.structuredEvents === true;
  // Supported conditionals stay unverified until this harness records a direct
  // release-UI stream probe — never inherit external CLI-only streamingSeen.
  return {
    "C-01": supportedStreaming
      ? {
        nativeSupport: "supported",
        result: conditionalResultForSupported(probes.streaming),
      }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
    "C-02": supportedStructured
      ? {
        nativeSupport: "supported",
        result: conditionalResultForSupported(probes.structured),
      }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
    "C-03": supportedApprovals
      ? { nativeSupport: "supported", result: "unverified" }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
    "C-04": supportedMultimodal
      ? { nativeSupport: "supported", result: "unverified" }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
    "C-05": supportedInterruptSteer
      ? {
        nativeSupport: "supported",
        result: conditionalResultForSupported(probes.interruptSteer),
      }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
    "C-06": supportedUsage
      ? { nativeSupport: "supported", result: "unverified" }
      : { nativeSupport: "unsupported", result: "unsupported-by-native" },
  };
}

export function coreChecksFromAggregate(aggregate) {
  const passOrFail = (ready) => (ready ? "pass" : "fail");
  const processLocal = aggregate.continuityScope === "process-local";
  return {
    "P-01": passOrFail(aggregate.officialNativeLane === true),
    "P-02": passOrFail(aggregate.realSessionIds === true),
    "P-03": passOrFail(processLocal
      ? aggregate.processLocalFactsEvidenceComplete === true
        && aggregate.processLocalFactsPassed === true
        && aggregate.processLocalContinuation === true
        && aggregate.hostShutdownEvidenceComplete === true
        && aggregate.hostShutdownPassed === true
      : aggregate.openNew === true && aggregate.exactResume === true),
    "P-04": passOrFail(
      aggregate.rawResponses === true
        && (processLocal
          ? aggregate.processLocalFactsEvidenceComplete === true
            && aggregate.processLocalFactsPassed === true
            && aggregate.processLocalOraclePassed === true
            && aggregate.hostShutdownEvidenceComplete === true
            && aggregate.hostShutdownPassed === true
          : aggregate.quiescenceOraclePassed === true
            && aggregate.publicStreamChunkOraclePassed === true)
        && aggregate.streamingEvidenceComplete === true
        && aggregate.streamingProven === true,
    ),
    "P-05": passOrFail(aggregate.settingsParity === true && aggregate.cwdParity === true),
    "P-06": passOrFail(aggregate.historyReadback === true),
    "P-07": passOrFail(
      aggregate.errorFailClosed === true && aggregate.permissionFailClosed === true,
    ),
    "P-08": passOrFail(aggregate.privacyPassed === true),
    "P-09": passOrFail(aggregate.cleanupPassed === true),
    "P-10": passOrFail(aggregate.conversationGatePassed === true),
  };
}

export function writeReleaseUiAdapterEvidence(aggregate, context) {
  // Core-only CLI receipts (exactContinue/streamingSeen) never write here.
  // Only a full release-UI paired aggregate may upsert an adapter evidence row.
  // Live agents may emit any valid chunk count in either protocol-valid order.
  // The deterministic fixture oracle, not a live ordering coincidence, proves
  // that the harness drains response-first notifications before publication.
  const processLocal = aggregate?.continuityScope === "process-local";
  if (processLocal && aggregate?.processLocalOracleEvidenceComplete !== true) {
    return { written: false, reason: "process_local_oracle_unproven" };
  }
  if (processLocal && aggregate?.processLocalOraclePassed !== true) {
    return { written: false, reason: "process_local_oracle_unproven" };
  }
  if (processLocal && (aggregate?.processLocalFactsEvidenceComplete !== true
    || aggregate?.processLocalFactsPassed !== true)) {
    return { written: false, reason: "process_local_facts_unproven" };
  }
  if (processLocal && (aggregate?.hostShutdownEvidenceComplete !== true
    || aggregate?.hostShutdownPassed !== true)) {
    return { written: false, reason: "process_local_host_shutdown_unproven" };
  }
  if (!processLocal && aggregate?.quiescenceOraclePassed !== true) {
    return { written: false, reason: "quiescence_oracle_unproven" };
  }
  if (!processLocal && aggregate?.publicStreamChunkOracleEvidenceComplete !== true) {
    return { written: false, reason: "stream_chunk_oracle_unproven" };
  }
  if (!processLocal && aggregate?.publicStreamChunkOraclePassed !== true) {
    return { written: false, reason: "stream_chunk_oracle_unproven" };
  }
  if (aggregate?.streamingEvidenceComplete !== true
    || aggregate?.streamingProven !== true) {
    return { written: false, reason: "streaming_unproven" };
  }
  if (aggregate?.status !== "release-ui-passed" || aggregate?.conversationGatePassed !== true) {
    return { written: false, reason: "release_ui_not_passed" };
  }
  if (aggregate.consecutivePasses < strictRoundCount) {
    return { written: false, reason: "consecutive_passes_incomplete" };
  }
  const packagingRegistry = JSON.parse(readFileSync(packagingRegistryPath, "utf8"));
  const inventory = JSON.parse(readFileSync(driversInventoryPath, "utf8"));
  const driver = inventory.drivers.find((row) => row.agentId === aggregate.agent);
  requireFact(Boolean(driver), "driver_inventory_missing_agent");
  const agentIds = packagedAgentIds(packagingRegistry);
  const registryDigest = registryDigestFor(agentIds);
  const inventoryDigest = driverInventoryDigestFor(inventory);
  const capabilitySnapshotDigest = `sha256:${digest(driver.capabilityMatrix)}`;
  const runtimeVersionDigest = binaryDigest(context.binary);
  const sidecarDigest = binaryDigest(context.sidecar);
  const conditionalRaw = conditionalChecksFromMatrix(driver.capabilityMatrix, {
    streaming: aggregate.streamingProven === true
      ? true
      : (aggregate.streamingProven === false ? false : undefined),
    structured: aggregate.structuredProven === true
      ? true
      : (aggregate.structuredProven === false ? false : undefined),
    interruptSteer: aggregate.interruptSteerProven === true
      ? true
      : (aggregate.interruptSteerProven === false ? false : undefined),
  });
  const conditionalChecks = Object.fromEntries(
    CONDITIONAL_CHECK_IDS.map((id) => [id, conditionalRaw[id]]),
  );
  const adapter = {
    agentId: aggregate.agent,
    driverId: driver.driverId,
    runtimeProtocol: driver.runtimeProtocol,
    harnessVersion: dispatchLaneHarnessVersion,
    runtimeVersionClass: "verified-release",
    runtimeVersionDigest,
    capabilitySnapshotDigest,
    adapterManifestDigest: adapterManifestDigestFor(aggregate.agent),
    releaseArtifactDigest: aggregate.productArtifactDigest,
    releaseSidecarDigest: sidecarDigest,
    productContinuityBindingDigest: aggregate.productContinuityBindingDigest,
    runtimeSourceClass: "discovered-binary",
    registryDigest,
    driverInventoryDigest: inventoryDigest,
    evidenceDigest: "",
    officialNativeLane: aggregate.officialNativeLane === true,
    consecutivePasses: aggregate.consecutivePasses,
    conversationGatePassed: true,
    cleanupPassed: aggregate.cleanupPassed === true,
    privacyPassed: aggregate.privacyPassed === true,
    coreChecks: coreChecksFromAggregate(aggregate),
    conditionalChecks,
  };
  adapter.evidenceDigest = adapterEvidenceDigestFor(adapter);
  assertEvidenceHygiene(adapter);
  requireFact(
    CORE_CHECK_IDS.every((id) => Object.hasOwn(adapter.coreChecks, id)),
    "evidence_core_checks_incomplete",
  );
  requireFact(
    CONDITIONAL_CHECK_IDS.every((id) => Object.hasOwn(adapter.conditionalChecks, id)),
    "evidence_conditional_checks_incomplete",
  );

  const targetEvidenceManifestPath = context.evidenceManifestPath || evidenceManifestPath;
  let evidence;
  try {
    evidence = JSON.parse(readFileSync(targetEvidenceManifestPath, "utf8"));
  } catch {
    evidence = {
      schemaVersion: EVIDENCE_SCHEMA_VERSION,
      contractVersion: CONTRACT_VERSION,
      harnessVersion: dispatchLaneHarnessVersion,
      toolVersionClass: dispatchLaneHarnessVersion,
      generatedAt: new Date().toISOString(),
      adapters: [],
    };
  }
  if (!Array.isArray(evidence.adapters)) evidence.adapters = [];
  evidence.schemaVersion = EVIDENCE_SCHEMA_VERSION;
  evidence.contractVersion = CONTRACT_VERSION;
  evidence.harnessVersion = dispatchLaneHarnessVersion;
  evidence.toolVersionClass = dispatchLaneHarnessVersion;
  evidence.generatedAt = new Date().toISOString();
  evidence.adapters = [
    ...evidence.adapters.filter((row) => row?.agentId !== aggregate.agent),
    adapter,
  ].sort((left, right) => String(left.agentId).localeCompare(String(right.agentId)));
  assertEvidenceHygiene(evidence);
  const temporaryEvidencePath = `${targetEvidenceManifestPath}.tmp-${process.pid}-${randomUUID()}`;
  writeFileSync(temporaryEvidencePath, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporaryEvidencePath, targetEvidenceManifestPath);
  return {
    written: true,
    agentId: aggregate.agent,
    consecutivePasses: aggregate.consecutivePasses,
    evidenceDigest: adapter.evidenceDigest,
  };
}
