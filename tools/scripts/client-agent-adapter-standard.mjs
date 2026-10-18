#!/usr/bin/env node

import { readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const inventory = readJson("crates/licoup-native/resources/agent-conversation-drivers.json");
const packaging = readJson("apps/desktop/packaging.modules.json");
const schema = readJson("packages/contracts/client/agent-conversation-adapter.schema.json");
const template = readJson("packages/contracts/client/fixtures/agent-conversation-adapter/template.json");
const manifestDirectory = resolve(
  root,
  "packages/contracts/client/fixtures/agent-conversation-adapter/manifests",
);
const safeId = /^[a-z0-9][a-z0-9._:+-]{0,127}$/u;
const laneFamilies = new Set(["acp", "app-server", "cli", "rpc", "serve-http", "stream-json", "unavailable"]);
const requiredCapabilities = [
  "laneFamily", "openNew", "exactResume", "streaming", "cancel", "interruptSteer",
  "structuredEvents", "approvals", "multimodal", "usageStatus", "officialLane",
  "hostSurvivesGuiDisconnect", "activeTurnReattach", "orderedCursorReplay",
];
const safetyBlockers = new Set([
  "antigravity_cli_structured_transport_unavailable",
  "canonical_driver_missing",
  "exact_session_resume_unavailable",
  "official_native_lane_missing",
  "safe_cleanup_unavailable",
]);
const capabilityOperations = Object.freeze({
  openNew: "openNew",
  exactResume: "exactResume",
  streaming: "stream",
  cancel: "cancel",
  interruptSteer: "interruptSteer",
  approvals: "approvals",
  multimodal: "multimodal",
  usageStatus: "usage",
});
const checkSemantics = Object.freeze({
  "P-01": "baseline-binding",
  "P-02": "native-session-creation",
  "P-03": "bidirectional-exact-resume",
  "P-04": "deterministic-final-result",
  "P-05": "effective-settings-parity",
  "P-06": "history-readback-and-rendering",
  "P-07": "error-cancel-timeout-parity",
  "P-08": "privacy-and-process-boundary",
  "P-09": "isolation-and-cleanup",
  "P-10": "exact-release-product-ui",
  "C-01": "streaming-delta",
  "C-02": "reasoning-and-tool-trace",
  "C-03": "approval-lifecycle",
  "C-04": "attachments-and-multimodal",
  "C-05": "interrupt-and-steer",
  "C-06": "usage-and-status",
});

function readJson(relativePath) {
  return JSON.parse(readFileSync(resolve(root, relativePath), "utf8"));
}

function parseValidationScope(argv) {
  if (argv.length === 0) {
    return Object.freeze({ agentId: null, sharedOnly: false, scope: "all" });
  }
  if (argv.length === 1 && argv[0] === "--shared") {
    return Object.freeze({ agentId: null, sharedOnly: true, scope: "shared" });
  }
  if (argv.length === 2 && argv[0] === "--agent" && safeId.test(argv[1])) {
    return Object.freeze({ agentId: argv[1], sharedOnly: false, scope: "agent" });
  }
  throw new Error("adapter_standard_argument_invalid");
}

function fail(code) {
  throw new Error(code);
}

function requireFact(value, code) {
  if (!value) fail(code);
}

function sorted(values) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

function sameSet(left, right) {
  return JSON.stringify(sorted(left)) === JSON.stringify(sorted(right));
}

function loadManifests() {
  return readdirSync(manifestDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".json"))
    .map((entry) => ({
      fileName: entry.name,
      manifest: JSON.parse(readFileSync(resolve(manifestDirectory, entry.name), "utf8")),
    }));
}

function validateSchemaAuthority() {
  requireFact(schema?.type === "object" && schema?.additionalProperties === false, "adapter_schema_not_closed");
  for (const field of ["identity", "officialCapabilityAssessment", "transport", "configuration", "operations", "events", "lifecycle", "privacy", "routedContext", "productIntegration", "acceptance"]) {
    requireFact(schema.required?.includes(field) && schema.properties?.[field], "adapter_schema_section_missing");
  }
  requireFact(schema.properties?.privacy?.properties?.promptInArguments?.type === "boolean", "adapter_schema_prompt_privacy_missing");
  requireFact(schema.properties?.privacy?.properties?.continuityIdInArguments?.type === "boolean", "adapter_schema_id_privacy_missing");
  requireFact(schema.properties?.privacy?.properties?.boundedInput?.const === true, "adapter_schema_input_bound_missing");
  requireFact(schema.properties?.privacy?.properties?.structuredEventProjection?.const === true, "adapter_schema_event_projection_missing");
  requireFact(schema.properties?.lifecycle?.properties?.survivesGuiDisconnect?.const === true, "adapter_schema_gui_survival_missing");
  requireFact(schema.properties?.lifecycle?.properties?.reattachScope?.const === "activeTurn", "adapter_schema_reattach_scope_missing");
  requireFact(schema.properties?.lifecycle?.properties?.replayMode?.const === "orderedCursor", "adapter_schema_replay_mode_missing");
  requireFact(schema.properties?.officialCapabilityAssessment?.properties?.completedBeforeImplementation?.const === true, "adapter_schema_official_assessment_order_missing");
  requireFact(schema.properties?.officialCapabilityAssessment?.properties?.sourceAuthority?.const === "official", "adapter_schema_official_authority_missing");
  for (const field of ["assessedVersion", "versionProbe", "capabilityProbe", "newSessionMethod", "exactResumeMethod", "streamingMethod", "historyMethod", "cleanupMethod", "officialReferences", "versionSupport"]) {
    requireFact(schema.properties?.officialCapabilityAssessment?.required?.includes(field), "adapter_schema_official_capability_missing");
  }
  // An assessed version is descriptive text and can never stand in for support:
  // the machine-readable interval is its own required field, it is closed, an
  // unknown bound must name a reason, and a bound is a semantic version exactly.
  const versionSupport = schema.properties?.officialCapabilityAssessment?.properties?.versionSupport;
  requireFact(versionSupport?.type === "object" && versionSupport?.additionalProperties === false, "adapter_schema_version_support_not_closed");
  requireFact(
    sameSet(versionSupport?.properties?.kind?.enum ?? [], ["unknown", "bounded"]),
    "adapter_schema_version_support_kind_missing",
  );
  requireFact(
    sameSet(versionSupport?.properties?.source?.enum ?? [], ["unverified", "probe-observed", "vendor-documented"]),
    "adapter_schema_version_support_source_missing",
  );
  requireFact(
    versionSupport?.properties?.from?.$ref === "#/$defs/safeVersion"
      && versionSupport?.properties?.before?.$ref === "#/$defs/safeVersion",
    "adapter_schema_version_support_bound_not_a_version",
  );
  requireFact(
    schema.$defs?.safeVersion?.pattern === "^[0-9]+\\.[0-9]+\\.[0-9]+(?:-[0-9A-Za-z.-]+)?$",
    "adapter_schema_version_bound_pattern_missing",
  );
  requireFact(schema.properties?.productIntegration?.properties?.dispatchLane?.const === "AgentDispatchLane", "adapter_schema_dispatch_lane_missing");
  for (const field of ["cliUsesCanonicalReadiness", "guiUsesCanonicalReadiness", "routingUsesCanonicalReadiness", "sharedNativeHistoryAuthority", "finalReplyFeedsThreadAndDistillation"]) {
    requireFact(schema.properties?.productIntegration?.properties?.[field]?.const === true, "adapter_schema_product_integration_missing");
  }
  requireFact(schema.properties?.acceptance?.properties?.productUiRequired?.type === "boolean", "adapter_schema_product_ui_missing");
  requireFact(schema.properties?.acceptance?.properties?.releaseP10Required?.type === "boolean", "adapter_schema_release_p10_missing");
  requireFact(schema.properties?.acceptance?.properties?.liveLocalForwardingRequired?.type === "boolean", "adapter_schema_live_forwarding_missing");
  requireFact(schema.properties?.acceptance?.properties?.realtimeOutputRequired?.type === "boolean", "adapter_schema_realtime_output_missing");
  requireFact(schema.properties?.acceptance?.properties?.sameNativeSessionRequired?.type === "boolean", "adapter_schema_same_session_missing");
  requireFact(schema.properties?.acceptance?.properties?.canonicalReadinessRequired?.type === "boolean", "adapter_schema_canonical_readiness_missing");
  requireFact(schema.properties?.acceptance?.properties?.nativeToArcRequired?.type === "boolean", "adapter_schema_native_to_arc_missing");
  requireFact(schema.properties?.acceptance?.properties?.arcToNativeRequired?.type === "boolean", "adapter_schema_arc_to_native_missing");
  requireFact(schema.properties?.acceptance?.properties?.exactArtifactRequired?.type === "boolean", "adapter_schema_exact_artifact_missing");
  requireFact(schema.properties?.acceptance?.properties?.minimumConsecutivePasses?.minimum === 1, "adapter_schema_round_count_invalid");
  requireFact(schema.properties?.acceptance?.properties?.minimumConsecutiveReleaseUiPasses?.minimum === 0, "adapter_schema_release_ui_round_count_weak");
  requireFact(schema.properties?.routedContext?.properties?.rawConversationAllowed?.const === false, "adapter_schema_routed_context_privacy_missing");
  requireFact(schema.properties?.routedContext?.properties?.contextDigestRequired?.const === true, "adapter_schema_routed_context_digest_missing");
  requireFact(schema.properties?.routedContext?.properties?.fidelityFailure?.const === "fail-closed", "adapter_schema_routed_context_fidelity_missing");
  requireFact(
    schema.properties?.acceptance?.properties?.checkSemantics?.type === "object"
      && Array.isArray(schema.properties?.acceptance?.properties?.checkSemantics?.required)
      && sameSet(
        schema.properties.acceptance.properties.checkSemantics.required,
        Object.keys(checkSemantics),
      ),
    "adapter_schema_check_semantics_missing",
  );

  const ajv = new Ajv2020({ allErrors: true, strict: true, strictRequired: false });
  addFormats(ajv);
  const validate = ajv.compile(schema);
  requireFact(validate(template), "adapter_template_schema_invalid");
  const processReuseOnly = structuredClone(template);
  delete processReuseOnly.lifecycle.survivesGuiDisconnect;
  delete processReuseOnly.lifecycle.reattachScope;
  delete processReuseOnly.lifecycle.replayMode;
  requireFact(!validate(processReuseOnly), "adapter_process_reuse_only_overclaimed");
  // The interval is validated against the producers' own documents, and the
  // three refusals below are the ones a free-text assessed version used to pass:
  // a bound that is not a version, a bound with no end, and an unknown bound
  // that names no reason.
  const freeTextBound = structuredClone(template);
  freeTextBound.officialCapabilityAssessment.versionSupport = {
    kind: "bounded",
    source: "vendor-documented",
    from: "version reported by fixed probe",
    before: "2.0.0",
  };
  requireFact(!validate(freeTextBound), "adapter_version_support_free_text_bound_accepted");
  const openEndedBound = structuredClone(template);
  openEndedBound.officialCapabilityAssessment.versionSupport = {
    kind: "bounded",
    source: "vendor-documented",
    from: "1.0.0",
  };
  requireFact(!validate(openEndedBound), "adapter_version_support_open_ended_bound_accepted");
  const reasonlessUnknown = structuredClone(template);
  reasonlessUnknown.officialCapabilityAssessment.versionSupport = {
    kind: "unknown",
    source: "unverified",
  };
  requireFact(!validate(reasonlessUnknown), "adapter_version_support_reasonless_unknown_accepted");
  const unverifiedBound = structuredClone(template);
  unverifiedBound.officialCapabilityAssessment.versionSupport = {
    kind: "bounded",
    source: "unverified",
    from: "1.0.0",
    before: "2.0.0",
  };
  requireFact(!validate(unverifiedBound), "adapter_version_support_unverified_bound_accepted");
  // A bound the schema does accept still validates: the refusals above are the
  // free-text shapes, not every bound.
  const declaredBound = structuredClone(template);
  declaredBound.officialCapabilityAssessment.versionSupport = {
    kind: "bounded",
    source: "vendor-documented",
    from: "1.0.0",
    before: "2.0.0",
  };
  requireFact(validate(declaredBound), "adapter_version_support_declared_bound_rejected");
  return validate;
}

function validateInventory(validateManifest, { agentId = null, sharedOnly = false } = {}) {
  const packaged = packaging?.modules?.["target-adapters"]?.targetAdapters;
  const drivers = inventory?.drivers;
  requireFact(Array.isArray(packaged) && Array.isArray(drivers), "adapter_inventory_missing");
  const ids = drivers.map((driver) => driver.agentId);
  requireFact(new Set(ids).size === ids.length && sameSet(ids, packaged), "adapter_packaging_set_drift");
  const driverIds = new Set();
  const driverByAgent = new Map();
  for (const driver of drivers) {
    requireFact(
      [driver.agentId, driver.driverId, driver.runtimeProtocol].every((value) => typeof value === "string" && safeId.test(value)),
      "adapter_identity_invalid",
    );
    requireFact(!driverIds.has(driver.driverId), "adapter_driver_id_duplicate");
    driverIds.add(driver.driverId);
    driverByAgent.set(driver.agentId, driver);
    if (sharedOnly || (agentId && driver.agentId !== agentId)) continue;
    requireFact(["conversation", "blocked", "history-only"].includes(driver.driverMode), "adapter_mode_invalid");
    requireFact(Array.isArray(driver.blockerCodes) && driver.blockerCodes.every((code) => safeId.test(code)), "adapter_blocker_invalid");
    const matrix = driver.capabilityMatrix;
    requireFact(matrix && requiredCapabilities.every((field) => Object.hasOwn(matrix, field)), "adapter_capability_incomplete");
    requireFact(laneFamilies.has(matrix.laneFamily), "adapter_lane_family_invalid");
    requireFact(requiredCapabilities.slice(1).every((field) => typeof matrix[field] === "boolean"), "adapter_capability_type_invalid");
    if (driver.driverMode === "blocked") {
      requireFact(driver.blockerCodes.length > 0, "adapter_blocker_missing");
      requireFact(driver.blockerCodes.some((code) => safetyBlockers.has(code)), "adapter_safety_blocker_missing");
    } else {
      requireFact(driver.blockerCodes.length === 0, "adapter_nonblocked_has_blocker");
    }
    if (matrix.exactResume !== true) {
      requireFact(driver.driverMode !== "conversation", "adapter_exact_resume_overclaimed");
    }
    requireFact(
      matrix.hostSurvivesGuiDisconnect === true
        && matrix.activeTurnReattach === true
        && matrix.orderedCursorReplay === true,
      "adapter_inventory_persistent_runtime_missing",
    );
  }

  const manifests = loadManifests();
  const manifestIds = manifests.map(({ manifest }) => manifest?.identity?.agentId);
  requireFact(
    manifests.length === packaged.length &&
      new Set(manifestIds).size === manifestIds.length &&
      sameSet(manifestIds, packaged),
    "adapter_manifest_packaging_set_drift",
  );
  if (agentId) requireFact(driverByAgent.has(agentId), "adapter_agent_unknown");
  for (const { fileName, manifest } of manifests) {
    if (sharedOnly || (agentId && manifest?.identity?.agentId !== agentId)) continue;
    requireFact(validateManifest(manifest), "adapter_manifest_schema_invalid");
    const manifestAgentId = manifest.identity.agentId;
    const driver = driverByAgent.get(manifestAgentId);
    requireFact(fileName === `${manifestAgentId}.json` && driver, "adapter_manifest_identity_invalid");
    requireFact(
      manifest.identity.packagingTargetId === manifestAgentId &&
        manifest.identity.driverId === driver.driverId &&
        manifest.identity.runtimeProtocol === driver.runtimeProtocol,
      "adapter_manifest_inventory_binding_drift",
    );
    const operations = manifest.operations;
    const matrix = driver.capabilityMatrix;
    for (const [capability, operationName] of Object.entries(capabilityOperations)) {
      requireFact(
        (operations[operationName].status === "supported") === (matrix[capability] === true),
        "adapter_manifest_capability_drift",
      );
    }
    requireFact(
      (manifest.transport.origin !== "unavailable") === matrix.officialLane,
      "adapter_manifest_official_lane_drift",
    );
    requireFact(
      (operations.history.status === "supported") === driver.historyReadable,
      "adapter_manifest_history_drift",
    );
    requireFact(
      // A completed message can arrive before idle without token-delta support.
      manifest.events.realtimeKinds.includes("agent.message.chunk") === matrix.streaming
        && (manifest.events.terminalKinds.length > 0) === matrix.structuredEvents,
      "adapter_manifest_event_capability_drift",
    );
    requireFact(
      (operations.cleanup.status === "supported") === manifest.privacy.safeCleanup &&
        (manifest.privacy.safeCleanup
          ? manifest.lifecycle.cleanupScope !== "unavailable"
          : operations.cleanup.status === "blocked" && manifest.lifecycle.cleanupScope === "unavailable"),
      "adapter_manifest_cleanup_inconsistent",
    );
    requireFact(
      manifest.lifecycle.survivesGuiDisconnect === true
        && manifest.lifecycle.reattachScope === "activeTurn"
        && manifest.lifecycle.replayMode === "orderedCursor",
      "adapter_manifest_persistent_runtime_missing",
    );
    if (operations.cancel.status === "supported") {
      requireFact(
        operations.exactResume.status === "supported" &&
          manifest.lifecycle.supervision !== "unavailable" &&
          manifest.lifecycle.cancelHandleScope === "active-turn",
        "adapter_manifest_cancel_inconsistent",
      );
    } else {
      requireFact(
        manifest.lifecycle.cancelHandleScope === "unavailable",
        "adapter_manifest_cancel_handle_overclaimed",
      );
    }
    const manifestBlockers = Object.values(operations)
      .filter((operation) => operation.status === "blocked")
      .map((operation) => operation.blockerCode);
    requireFact(
      sameSet(new Set(manifestBlockers), driver.blockerCodes),
      "adapter_manifest_blocker_drift",
    );
    if (driver.driverMode === "conversation") {
      requireFact(
        ["openNew", "exactResume", "send", "cleanup"]
          .every((operation) => operations[operation].status === "supported") &&
          matrix.officialLane === true && manifest.privacy.safeCleanup === true,
        "adapter_manifest_conversation_incomplete",
      );
    }
    if (manifest.transport.family === "unavailable") {
      requireFact(
        ["openNew", "exactResume", "send", "stream"]
          .every((operation) => operationStatusIsBlocked(operations[operation])) &&
          matrix.officialLane === false,
        "adapter_manifest_unavailable_transport_overclaimed",
      );
    }
    const sameSessionGate = manifest.acceptance.checkSemantics?.["P-10"]
      === "same-session-sequential-turns"
      && manifest.acceptance.productUiRequired === false;
    const arcLocalServiceGate = manifest.acceptance.checkSemantics?.["P-10"]
      === "arc-local-service-consecutive-rounds"
      && manifest.acceptance.productUiRequired === false;
    const expectedSemantics = sameSessionGate
      ? {
        ...checkSemantics,
        "P-03": "same-session-sequential-resume",
        "P-10": "same-session-sequential-turns",
      }
      : arcLocalServiceGate
        ? {
          ...checkSemantics,
          "P-03": "bidirectional-exact-resume",
          "P-10": "arc-local-service-consecutive-rounds",
        }
        : checkSemantics;
    requireFact(
      JSON.stringify(manifest.acceptance.checkSemantics) === JSON.stringify(expectedSemantics),
      "adapter_manifest_check_semantics_drift",
    );
    if (sameSessionGate) {
      requireFact(
        manifest.acceptance.productUiRequired === false
          && manifest.acceptance.releaseP10Required === false
          && manifest.acceptance.nativeToArcRequired === false
          && manifest.acceptance.arcToNativeRequired === false
          && manifest.acceptance.exactArtifactRequired === false
          && manifest.acceptance.minimumConsecutiveReleaseUiPasses === 0
          && manifest.acceptance.minimumConsecutivePasses === 1,
        "adapter_manifest_same_session_gate_incomplete",
      );
    } else if (arcLocalServiceGate) {
      requireFact(
        manifest.acceptance.productUiRequired === false
          && manifest.acceptance.releaseP10Required === false
          && manifest.acceptance.nativeToArcRequired === true
          && manifest.acceptance.arcToNativeRequired === true
          && manifest.acceptance.exactArtifactRequired === false
          && manifest.acceptance.minimumConsecutiveReleaseUiPasses === 0
          && manifest.acceptance.minimumConsecutivePasses === 1
          && manifest.acceptance.liveLocalForwardingRequired === true,
        "adapter_manifest_arc_local_service_gate_incomplete",
      );
    } else {
      requireFact(
        manifest.acceptance.productUiRequired === true
          && manifest.acceptance.releaseP10Required === true
          && manifest.acceptance.minimumConsecutivePasses === 1
          && manifest.acceptance.minimumConsecutiveReleaseUiPasses === 1,
        "adapter_manifest_release_ui_gate_incomplete",
      );
    }
    // Argv privacy is a per-field transport claim, not an agent-id allowlist:
    // streaming CLIs can keep prompts on stdin while passing only a native
    // continuation id through a documented resume argument.
    if (manifest.privacy.promptInArguments === true) {
      requireFact(
        manifest.transport.family === "cli"
          && manifest.transport.promptChannel === "launch-argument",
        "adapter_manifest_argv_transport_drift",
      );
    } else {
      requireFact(
        manifest.privacy.promptInArguments === false
          && manifest.transport.promptChannel !== "launch-argument",
        "adapter_manifest_argv_privacy_drift",
      );
    }
    if (manifest.privacy.continuityIdInArguments === true) {
      requireFact(
        manifest.transport.continuityChannel === "launch-argument",
        "adapter_manifest_continuity_argv_transport_drift",
      );
    } else {
      requireFact(
        manifest.privacy.continuityIdInArguments === false
          && manifest.transport.continuityChannel !== "launch-argument",
        "adapter_manifest_continuity_argv_privacy_drift",
      );
    }
  }
  return agentId ? 1 : manifests.length;
}

function operationStatusIsBlocked(operation) {
  return operation?.status === "blocked";
}

try {
  const validationScope = parseValidationScope(process.argv.slice(2));
  const validateManifest = validateSchemaAuthority();
  const manifestCount = validateInventory(validateManifest, validationScope);
  const selectedCount = validationScope.agentId ? 1 : inventory.drivers.length;
  process.stdout.write(`${JSON.stringify({
    schemaVersion: "lico.agent-adapter-standard.receipt.v1",
    ok: true,
    scope: validationScope.scope,
    agentId: validationScope.agentId,
    adapters: selectedCount,
    manifests: manifestCount,
    packaged: validationScope.agentId
      ? 1
      : packaging.modules["target-adapters"].targetAdapters.length,
    productUiRequiredByDefault: true,
    cursorSameSessionGate: true,
    minimumConsecutivePasses: 1,
    minimumConsecutiveReleaseUiPassesDefault: 1,
    officialCapabilityAssessmentRequired: true,
    canonicalReadinessRequired: true,
    guiDisconnectSurvivalRequired: true,
    activeTurnReattachmentRequired: true,
    orderedCursorReplayRequired: true,
    processReuseOnlyRejected: true,
    templateValidated: true,
  })}\n`);
} catch (error) {
  const code = safeId.test(error?.message || "") ? error.message : "adapter_standard_failed";
  process.stdout.write(`${JSON.stringify({
    schemaVersion: "lico.agent-adapter-standard.receipt.v1",
    ok: false,
    errorCode: code,
  })}\n`);
  process.exitCode = 1;
}
