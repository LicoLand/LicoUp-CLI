import { validateCapabilityReport } from "../secure-mesh-capability-report.mjs";
import {
  NODE_KEYS,
  linuxEvidenceSchemaVersion,
  linuxNodeMatrixSchema,
} from "./constants.mjs";
import { assert, assertExactKeys, scanPrivacy, validatePrivacyRecord, validateRootRedaction, validateSourceBinding } from "./shared.mjs";

export function validateLinuxNodeMatrixReport(report, expectedSourceDigest = "") {
  assertExactKeys(report, NODE_KEYS, "Linux node matrix report");
  assert(report.schema === linuxNodeMatrixSchema, "Linux node matrix schema is invalid");
  assert(report.schemaVersion === linuxEvidenceSchemaVersion,
    "Linux node matrix schema version is invalid");
  validateRootRedaction(report);
  validateSourceBinding(report.sourceBinding, expectedSourceDigest);
  assertExactKeys(report.runtime, [
    "kind",
    "nodeCount",
    "currentClientArchive",
    "publicOperationsOnly",
    "eventDrivenReadiness"
  ], "Linux node runtime record");
  assert(report.runtime.kind === "isolated_linux_containers" && report.runtime.nodeCount === 3,
    "Linux node runtime shape is invalid");
  assert(report.runtime.currentClientArchive === true && report.runtime.publicOperationsOnly === true &&
    report.runtime.eventDrivenReadiness === true, "Linux node runtime proof is incomplete");
  assertExactKeys(report.isolation, [
    "participantLabels",
    "distinctStateRoots",
    "noSharedSecretVolume",
    "uniquePublicIdentityCount",
    "crossNodeStateReadRejected",
    "containerIsolation"
  ], "Linux node isolation record");
  assert(
    JSON.stringify(report.isolation.participantLabels) ===
      JSON.stringify(["linux-a", "linux-b", "linux-c"]),
    "Linux node participant labels are invalid"
  );
  assert(report.isolation.uniquePublicIdentityCount === 3,
    "Linux nodes did not prove three unique public identities");
  for (const key of [
    "distinctStateRoots",
    "noSharedSecretVolume",
    "crossNodeStateReadRejected",
    "containerIsolation"
  ]) {
    assert(report.isolation[key] === true, `Linux node isolation field ${key} is incomplete`);
  }
  assertExactKeys(report.pairwise, [
    "exchangeCount",
    "allNodesParticipated",
    "secureSessionsEstablished",
    "opaqueRelay",
    "relayPlaintextObserved",
    "relayCiphertextIncludedInReport"
  ], "Linux node pairwise record");
  assert(report.pairwise.exchangeCount >= 2 && report.pairwise.allNodesParticipated === true &&
    report.pairwise.secureSessionsEstablished === true && report.pairwise.opaqueRelay === true &&
    report.pairwise.relayPlaintextObserved === false &&
    report.pairwise.relayCiphertextIncludedInReport === false,
  "Linux node pairwise proof is incomplete");
  assertExactKeys(report.restart, [
    "restartedParticipant",
    "restartedProcessCount",
    "restartRequiresRePairRekey",
    "unaffectedParticipantCount",
    "postRestartExchangeReady",
    "stateContaminationDetected"
  ], "Linux node restart record");
  assert(report.restart.restartedParticipant === "linux-a" &&
    report.restart.restartedProcessCount === 1 &&
    report.restart.restartRequiresRePairRekey === true &&
    report.restart.unaffectedParticipantCount === 2 &&
    report.restart.postRestartExchangeReady === true &&
    report.restart.stateContaminationDetected === false,
  "Linux node restart isolation proof is incomplete");
  assertExactKeys(report.teardown, [
    "bounded",
    "nodeCount",
    "allProcessesStopped",
    "allContainersRemoved",
    "ephemeralStateRemoved"
  ], "Linux node teardown record");
  assert(report.teardown.bounded === true && report.teardown.nodeCount === 3 &&
    report.teardown.allProcessesStopped === true &&
    report.teardown.allContainersRemoved === true &&
    report.teardown.ephemeralStateRemoved === true,
  "Linux node teardown proof is incomplete");
  validateCapabilityReport(report.capabilityReport);
  validatePrivacyRecord(report.privacy);
  assertExactKeys(report.summary, [
    "currentSourceNodes",
    "isolationReady",
    "pairwiseReady",
    "restartIsolationReady",
    "teardownReady",
    "privacyReady"
  ], "Linux node summary");
  assert(Object.values(report.summary).every((value) => value === true),
    "Linux node matrix summary is incomplete");
  assert(report.ok === true, "Linux node matrix report is not ready");
  scanPrivacy(report);
  return Object.freeze({ ok: true, sourceStateDigest: report.sourceBinding.sourceStateDigest });
}
