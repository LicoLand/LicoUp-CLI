export const SECURE_MESH_TRUST_UX_REPORT_SCHEMA_VERSION =
  "licomesh.secure-mesh.trust-ux-report.v2";

export const SECURE_MESH_TRUST_UX_SELECTED_TARGETS = Object.freeze([
  "macos",
  "android"
]);

export const SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS =
  "unsupported_not_claimed";

export const SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID =
  "desktop-trust-model-and-widget-tests";

const PRODUCT_TRUST_UX_SOURCE_CHECK_IDS = Object.freeze([
  "rust-public-config-exposes-redacted-exact-trust-presentation",
  "product-ui-renders-verified-and-blocked-trust-states",
  "product-model-rejects-invalid-safety-number-groups"
]);

const AUTHORITY_FIELD_PATTERN = /(?:trust|release|support|ready|status|complete)/iu;
const TOP_LEVEL_AUTHORITY_FIELDS = new Set([
  "diagnosticStatus",
  "productionReady",
  "releaseReady",
  "physicalTrustEvidence",
  "physicalTrustMatrix",
  "trustEvidence"
]);
const SUMMARY_AUTHORITY_FIELDS = new Set([
  "mobileNativeTrustActionsReady",
  "productTrustUxTestsReady",
  "productTrustUxReady",
  "macosTrustReceiptReady",
  "androidPhysicalTrustLifecycleReady",
  "iosSupportStatus",
  "iosReleaseGate",
  "selectedTargetReleaseReady",
  "productionReady",
  "releaseReady"
]);
const SELECTED_TARGET_AUTHORITY_FIELDS = new Set([
  "productTrustUxReady",
  "androidPhysicalTrustReady",
  "macosTrustReceiptReady",
  "selectedTargetReleaseReady",
  "iosSupportStatus",
  "iosReleaseGate"
]);
const IOS_PHYSICAL_AUTHORITY_FIELDS = new Set([
  "releaseGate",
  "supportStatus",
  "status"
]);
const ANDROID_PHYSICAL_AUTHORITY_FIELDS = new Set([
  "lifecycleFfiReady",
  "mandatoryFoundationComplete",
  "qrVerificationReady",
  "restartReplayReady",
  "rotateLifecycleReady",
  "safeCustodyReady",
  "sasVerificationReady",
  "status",
  "trustLifecycleReady"
]);
const SAFE_CUSTODY_STRATEGIES = new Set([
  "os_secure_store",
  "memory_only_ephemeral"
]);

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function exactStringArray(value, expected) {
  return Array.isArray(value) &&
    value.length === expected.length &&
    value.every((item, index) => item === expected[index]);
}

function hasUnknownAuthorityField(record, allowedFields) {
  return Object.keys(asRecord(record)).some((key) =>
    AUTHORITY_FIELD_PATTERN.test(key) && !allowedFields.has(key)
  );
}

function embeddedAndroidPhysicalTrustReady(value) {
  const evidence = asRecord(value);
  return evidence.ok === true &&
    evidence.present === true &&
    evidence.platform === "android" &&
    evidence.physicalDevice === true &&
    evidence.peerVerified === true &&
    evidence.capabilityReportValid === true &&
    evidence.mandatoryFoundationComplete === true &&
    evidence.safeCustodyReady === true &&
    SAFE_CUSTODY_STRATEGIES.has(evidence.custodyStrategy) &&
    evidence.portableConfigPrivateMaterialAbsent === true &&
    evidence.restartReplayReady === true &&
    evidence.lifecycleFfiReady === true &&
    evidence.trustLifecycleReady === true &&
    evidence.qrVerificationReady === true &&
    evidence.sasVerificationReady === true &&
    evidence.keyChangeBlocksSensitive === true &&
    evidence.rotateLifecycleReady === true &&
    evidence.revokeBlocksSensitive === true &&
    evidence.recoveryRequiresConfirmation === true;
}

function macosReceiptReady(receipt) {
  const report = asRecord(receipt);
  return report.ok === true &&
    report.platform === "macos" &&
    report.redacted === true &&
    report.reportLeakScan === true &&
    Array.isArray(report.receipts) &&
    report.receipts.some((entry) =>
      entry?.installReceiptReady === true &&
      entry?.launchReady === true &&
      entry?.smokeReady === true &&
      entry?.capabilityProofReady === true
    );
}

export function reduceSecureMeshTrustUxReadiness({
  verificationPassed,
  mobileNativeActionsComplete,
  sourceResults,
  productTestResults,
  physicalTrust,
  macosAdaptiveReceipt
} = {}) {
  const checks = Array.isArray(sourceResults) ? sourceResults : [];
  const physical = asRecord(physicalTrust);
  const productTrustUxTestsReady = Array.isArray(productTestResults) &&
    productTestResults.some((item) =>
      item?.id === SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID && item?.ok === true
    );
  const productTrustUxReady = productTrustUxTestsReady &&
    PRODUCT_TRUST_UX_SOURCE_CHECK_IDS.every((id) =>
      checks.some((item) => item?.id === id && item?.ok === true)
    );
  const androidPhysicalTrustReady = embeddedAndroidPhysicalTrustReady(physical.android);
  const macosTrustReceiptReady = macosReceiptReady(macosAdaptiveReceipt);
  const selectedTargetReleaseReady =
    verificationPassed === true &&
    mobileNativeActionsComplete === true &&
    productTrustUxReady &&
    androidPhysicalTrustReady &&
    macosTrustReceiptReady;

  return Object.freeze({
    productTrustUxReady,
    productTrustUxTestsReady,
    androidPhysicalTrustReady,
    macosTrustReceiptReady,
    selectedTargetReleaseReady,
    selectedTargets: SECURE_MESH_TRUST_UX_SELECTED_TARGETS,
    iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
    iosReleaseGate: false
  });
}

export function validateSecureMeshTrustUxV2Report(report) {
  const payload = asRecord(report);
  const summary = asRecord(payload.summary);
  const selected = asRecord(payload.selectedTargetAcceptance);
  const androidPhysical = asRecord(payload.physicalTrustEvidence?.android);
  const iosPhysical = asRecord(payload.physicalTrustEvidence?.ios);
  const productTestResults = Array.isArray(payload.productTestResults)
    ? payload.productTestResults
    : [];
  const unknownAuthorityFieldsAbsent = ![
    [payload, TOP_LEVEL_AUTHORITY_FIELDS],
    [summary, SUMMARY_AUTHORITY_FIELDS],
    [selected, SELECTED_TARGET_AUTHORITY_FIELDS],
    [androidPhysical, ANDROID_PHYSICAL_AUTHORITY_FIELDS],
    [iosPhysical, IOS_PHYSICAL_AUTHORITY_FIELDS]
  ].some(([record, allowedFields]) =>
    hasUnknownAuthorityField(record, allowedFields)
  );
  const schemaReady =
    payload.schemaVersion === SECURE_MESH_TRUST_UX_REPORT_SCHEMA_VERSION;
  const selectedTargetsReady = exactStringArray(
    selected.selectedTargets,
    SECURE_MESH_TRUST_UX_SELECTED_TARGETS
  );
  const iosNonGateReady =
    summary.iosSupportStatus === SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS &&
    selected.iosSupportStatus === SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS &&
    iosPhysical.supportStatus === SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS &&
    summary.iosReleaseGate === false &&
    selected.iosReleaseGate === false &&
    iosPhysical.releaseGate === false &&
    iosPhysical.ok === false;
  const productTrustUxTestsReady =
    summary.productTrustUxTestsReady === true &&
    productTestResults.some((item) =>
      item?.id === SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID && item?.ok === true
    );
  const productTrustUxReady =
    summary.productTrustUxReady === true &&
    selected.productTrustUxReady === true &&
    productTrustUxTestsReady;
  const embeddedAndroidReady = embeddedAndroidPhysicalTrustReady(androidPhysical);
  const androidPhysicalTrustReady =
    summary.androidPhysicalTrustLifecycleReady === embeddedAndroidReady &&
    selected.androidPhysicalTrustReady === embeddedAndroidReady &&
    embeddedAndroidReady;
  const macosTrustReceiptReady =
    summary.macosTrustReceiptReady === true &&
    selected.macosTrustReceiptReady === true;
  const selectedTargetReleaseReady =
    summary.selectedTargetReleaseReady === true &&
    selected.selectedTargetReleaseReady === true;
  const productionClaimSuppressed =
    payload.productionReady === false &&
    summary.productionReady === false;
  const expectedSelectedTargetReleaseReady =
    payload.ok === true &&
    summary.verificationPassed === true &&
    summary.mobileNativeTrustActionsReady === true &&
    productTrustUxReady &&
    androidPhysicalTrustReady &&
    macosTrustReceiptReady;
  const consistencyReady =
    summary.verificationPassed === (payload.ok === true) &&
    summary.productTrustUxReady === selected.productTrustUxReady &&
    summary.androidPhysicalTrustLifecycleReady === embeddedAndroidReady &&
    selected.androidPhysicalTrustReady === embeddedAndroidReady &&
    summary.macosTrustReceiptReady === selected.macosTrustReceiptReady &&
    summary.selectedTargetReleaseReady === selected.selectedTargetReleaseReady &&
    selectedTargetReleaseReady === expectedSelectedTargetReleaseReady &&
    payload.releaseReady === expectedSelectedTargetReleaseReady &&
    summary.releaseReady === expectedSelectedTargetReleaseReady;
  const contractReady =
    schemaReady &&
    selectedTargetsReady &&
    iosNonGateReady &&
    productionClaimSuppressed &&
    unknownAuthorityFieldsAbsent &&
    consistencyReady;

  return Object.freeze({
    contractReady,
    schemaReady,
    selectedTargetsReady,
    iosNonGateReady,
    productionClaimSuppressed,
    unknownAuthorityFieldsAbsent,
    consistencyReady,
    productTrustUxReady: contractReady && productTrustUxReady,
    productTrustUxTestsReady: contractReady && productTrustUxTestsReady,
    androidPhysicalTrustReady: contractReady && androidPhysicalTrustReady,
    macosTrustReceiptReady: contractReady && macosTrustReceiptReady,
    selectedTargetReleaseReady: contractReady && selectedTargetReleaseReady
  });
}

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function readySourceResults() {
  return PRODUCT_TRUST_UX_SOURCE_CHECK_IDS.map((id) => ({ id, ok: true }));
}

function readyMacosReceipt() {
  return {
    ok: true,
    platform: "macos",
    redacted: true,
    reportLeakScan: true,
    receipts: [{
      installReceiptReady: true,
      launchReady: true,
      smokeReady: true,
      capabilityProofReady: true
    }]
  };
}

function androidPhysicalEvidence(ready) {
  return {
    ok: ready,
    present: ready,
    platform: "android",
    physicalDevice: ready,
    peerVerified: ready,
    capabilityReportValid: ready,
    mandatoryFoundationComplete: ready,
    custodyStrategy: ready ? "os_secure_store" : "",
    safeCustodyReady: ready,
    portableConfigPrivateMaterialAbsent: ready,
    restartReplayReady: ready,
    lifecycleFfiReady: ready,
    trustLifecycleReady: ready,
    qrVerificationReady: ready,
    sasVerificationReady: ready,
    keyChangeBlocksSensitive: ready,
    rotateLifecycleReady: ready,
    revokeBlocksSensitive: ready,
    recoveryRequiresConfirmation: ready,
    status: ready ? "android-physical-trust-lifecycle-verified" : "missing"
  };
}

function v2ReportFixture(readiness) {
  return {
    schemaVersion: SECURE_MESH_TRUST_UX_REPORT_SCHEMA_VERSION,
    ok: readiness.verificationPassed,
    productionReady: false,
    releaseReady: readiness.selectedTargetReleaseReady,
    productTestResults: [{
      id: SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID,
      ok: readiness.productTrustUxTestsReady
    }],
    physicalTrustEvidence: {
      android: androidPhysicalEvidence(readiness.androidPhysicalTrustReady),
      ios: {
        ok: false,
        supportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
        releaseGate: false
      }
    },
    selectedTargetAcceptance: {
      selectedTargets: [...SECURE_MESH_TRUST_UX_SELECTED_TARGETS],
      productTrustUxReady: readiness.productTrustUxReady,
      androidPhysicalTrustReady: readiness.androidPhysicalTrustReady,
      macosTrustReceiptReady: readiness.macosTrustReceiptReady,
      selectedTargetReleaseReady: readiness.selectedTargetReleaseReady,
      iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
      iosReleaseGate: false
    },
    summary: {
      verificationPassed: readiness.verificationPassed,
      mobileNativeTrustActionsReady: readiness.mobileNativeActionsComplete,
      productTrustUxTestsReady: readiness.productTrustUxTestsReady,
      productTrustUxReady: readiness.productTrustUxReady,
      androidPhysicalTrustLifecycleReady: readiness.androidPhysicalTrustReady,
      macosTrustReceiptReady: readiness.macosTrustReceiptReady,
      selectedTargetReleaseReady: readiness.selectedTargetReleaseReady,
      productionReady: false,
      iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
      iosReleaseGate: false,
      releaseReady: readiness.selectedTargetReleaseReady
    }
  };
}

export function runSecureMeshTrustUxReducerSelfTest() {
  const baseInput = {
    verificationPassed: true,
    mobileNativeActionsComplete: true,
    sourceResults: readySourceResults(),
    productTestResults: [{ id: SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID, ok: true }],
    physicalTrust: {
      android: androidPhysicalEvidence(true),
      ios: { ok: true, releaseGate: true }
    },
    macosAdaptiveReceipt: readyMacosReceipt()
  };
  const ready = reduceSecureMeshTrustUxReadiness(baseInput);
  requireValue(ready.productTrustUxReady, "complete product trust UX evidence must reduce ready");
  requireValue(ready.selectedTargetReleaseReady, "macOS and Android selected-target evidence must reduce ready");
  requireValue(
    ready.iosSupportStatus === SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS &&
      ready.iosReleaseGate === false,
    "iOS must remain unsupported, unclaimed, and outside the selected-target gate"
  );

  const cases = [
    {
      name: "missing-product-source-check",
      input: { ...baseInput, sourceResults: readySourceResults().slice(1) },
      expected: "productTrustUxReady"
    },
    {
      name: "product-tests-failed",
      input: {
        ...baseInput,
        productTestResults: [{ id: SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID, ok: false }]
      },
      expected: "productTrustUxReady"
    },
    {
      name: "verification-failed",
      input: { ...baseInput, verificationPassed: false }
    },
    {
      name: "mobile-actions-missing",
      input: { ...baseInput, mobileNativeActionsComplete: false }
    },
    {
      name: "android-physical-receipt-missing",
      input: { ...baseInput, physicalTrust: { android: { ok: false } } }
    },
    {
      name: "macos-install-receipt-missing",
      input: { ...baseInput, macosAdaptiveReceipt: {} }
    }
  ];
  for (const item of cases) {
    const reduced = reduceSecureMeshTrustUxReadiness(item.input);
    requireValue(!reduced.selectedTargetReleaseReady, `${item.name} must fail closed`);
    if (item.expected === "productTrustUxReady") {
      requireValue(!reduced.productTrustUxReady, `${item.name} must fail product trust UX closed`);
    }
  }

  const readyReport = v2ReportFixture({
    ...ready,
    verificationPassed: true,
    mobileNativeActionsComplete: true
  });
  requireValue(
    validateSecureMeshTrustUxV2Report(readyReport).contractReady,
    "consistent Trust UX v2 report must validate"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      schemaVersion: "licomesh.secure-mesh.trust-ux-report.unsupported"
    }).contractReady,
    "unsupported Trust UX report schema must fail closed"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      summary: { ...readyReport.summary, unrecognizedTrustAuthorityOverride: true }
    }).contractReady,
    "unknown trust authority field must fail closed"
  );
  for (const field of [
    "unrecognizedCapabilityReady",
    "unrecognizedLifecycleStatus",
    "unrecognizedProofComplete"
  ]) {
    requireValue(
      !validateSecureMeshTrustUxV2Report({
        ...readyReport,
        summary: { ...readyReport.summary, [field]: true }
      }).contractReady,
      `${field} must fail closed`
    );
  }
  requireValue(
    validateSecureMeshTrustUxV2Report({
      ...readyReport,
      diagnosticStatus: "selected-target-ready"
    }).contractReady,
    "listed non-authority diagnostics must not be mistaken for readiness overrides"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      productionReady: true
    }).contractReady,
    "Trust UX report must not claim production certification"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      summary: { ...readyReport.summary, productionReady: true }
    }).contractReady,
    "Trust UX summary must not claim production certification"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      physicalTrustEvidence: {
        ...readyReport.physicalTrustEvidence,
        android: {
          ...readyReport.physicalTrustEvidence.android,
          qrVerificationReady: false
        }
      }
    }).contractReady,
    "Android readiness must be recomputed from complete embedded lifecycle evidence"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      physicalTrustEvidence: {
        ...readyReport.physicalTrustEvidence,
        android: {
          ...readyReport.physicalTrustEvidence.android,
          capabilityReportValid: false
        }
      }
    }).contractReady,
    "Android readiness must require valid exact capability evidence"
  );
  requireValue(
    !validateSecureMeshTrustUxV2Report({
      ...readyReport,
      selectedTargetAcceptance: {
        ...readyReport.selectedTargetAcceptance,
        selectedTargetReleaseReady: false
      }
    }).contractReady,
    "inconsistent selected-target readiness must fail closed"
  );

  return Object.freeze({ ok: true, caseCount: 19 });
}
