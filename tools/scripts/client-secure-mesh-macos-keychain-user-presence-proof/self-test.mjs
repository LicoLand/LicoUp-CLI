import {
  reduceCapabilityFacts,
  validateCapabilityReport,
} from "../lib/secure-mesh-capability-report.mjs";
import { createCapabilityFacts } from "./capability/facts.mjs";
import { swiftSource } from "./helper/swift-source.mjs";
import { assertNoLeak } from "./privacy.mjs";
import { normalizeReportReference } from "./report.mjs";

export function runPolicySelfTest() {
  const standardOnly = fixturePayload({ standard: true });
  const standardReport = reduceCapabilityFacts(createCapabilityFacts(standardOnly));
  validateCapabilityReport(standardReport);
  if (standardReport.custody?.strategy !== "os_secure_store" ||
      !standardReport.enabled.includes("custody.apple_keychain") ||
      standardReport.enabled.includes("custody.data_protection_keychain")) {
    throw new Error("standard Keychain must remain a valid lower safe exact capability set");
  }

  const stronger = fixturePayload({ standard: true, dataProtection: true, userPresence: true, secureEnclave: true });
  const strongerReport = reduceCapabilityFacts(createCapabilityFacts(stronger));
  validateCapabilityReport(strongerReport);
  for (const capability of [
    "custody.data_protection_keychain",
    "custody.os_user_presence",
    "custody.device_credential",
    "custody.secure_enclave",
  ]) {
    if (!strongerReport.enabled.includes(capability)) {
      throw new Error(`stronger verified capability did not accumulate: ${capability}`);
    }
  }
  if (!standardReport.enabled.every((capability) => strongerReport.enabled.includes(capability))) {
    throw new Error("stronger macOS facts must monotonically accumulate over standard Keychain");
  }

  const falseClaim = structuredClone(standardReport);
  falseClaim.enabled.push("custody.secure_enclave");
  let falseClaimRejected = false;
  try {
    validateCapabilityReport(falseClaim);
  } catch {
    falseClaimRejected = true;
  }
  if (!falseClaimRejected) throw new Error("false Secure Enclave enhancement claim was accepted");

  const source = swiftSource();
  if ((source.match(/context\.evaluatePolicy\(/gu) || []).length !== 1 ||
      !source.includes("interactiveAuthorizationAttemptCount = 1") ||
      !source.includes("automaticAuthorizationRetryUsed") ||
      !source.includes("kSecUseAuthenticationUIFail") ||
      !source.includes("prepareUserPresence(dataProtection: false)")) {
    throw new Error("macOS proof must preserve one-prompt adaptive fallback semantics");
  }

  let leakRejected = false;
  try {
    const privatePathFixture = ["", "Users", "example", "private"].join("/");
    assertNoLeak({ localArtifact: privatePathFixture }, "privacy fixture");
  } catch {
    leakRejected = true;
  }
  if (!leakRejected) throw new Error("macOS proof privacy fixture was accepted");

  let absoluteReportRejected = false;
  try {
    normalizeReportReference(["", "private", "fixture", "proof.json"].join("/"));
  } catch {
    absoluteReportRejected = true;
  }
  if (!absoluteReportRejected) {
    throw new Error("absolute macOS proof output was accepted");
  }

  return {
    ok: true,
    caseCount: 6,
    standardKeychainAccepted: true,
    strongerCapabilitiesAccumulate: true,
    falseEnhancementClaimRejected: true,
    singlePromptFlowEnforced: true,
    privacyFixtureRejected: true,
    absoluteReportRejected: true,
  };
}

export function fixturePayload({
  standard = false,
  dataProtection = false,
  userPresence = false,
  secureEnclave = false,
} = {}) {
  const store = (ready) => ({
    itemCreated: ready,
    readMatched: ready,
    itemDeleted: true,
    deviceOnlyAccessibilityObserved: ready,
  });
  return {
    standardKeychain: store(standard),
    dataProtectionKeychain: store(dataProtection),
    userPresence: {
      selectedStore: userPresence
        ? dataProtection ? "data_protection_keychain" : "standard_keychain"
        : "none",
      accessControlCreated: userPresence,
      itemCreated: userPresence,
      nonInteractiveReadBlocked: userPresence,
      authorizedReadSucceeded: userPresence,
      itemDeleted: true,
    },
    localAuthenticationAvailable: userPresence,
    biometricAuthenticationAvailable: false,
    secureEnclaveOperationSucceeded: secureEnclave,
    singleAuthorizationContextCreated: true,
    singleAuthorizationContextSharedByOperations: true,
    interactiveWorkflowSelected: userPresence,
    interactiveAuthorizationAttemptCount: userPresence ? 1 : 0,
    interactiveAuthorizationSucceeded: userPresence,
    interactiveAuthorizationTimedOut: false,
    automaticAuthorizationRetryUsed: false,
    appPasswordPromptUsed: false,
    appCredentialPromptUsed: false,
  };
}
