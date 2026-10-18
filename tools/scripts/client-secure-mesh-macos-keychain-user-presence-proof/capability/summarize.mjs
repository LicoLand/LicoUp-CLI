import {
  probeSucceeded,
  userPresenceOperationSucceeded,
} from "./facts.mjs";

export function summarize(payload, helper, capabilityReport, capabilityValidation) {
  const standardKeychainAvailable = probeSucceeded(payload.standardKeychain);
  const dataProtectionKeychainAvailable = probeSucceeded(payload.dataProtectionKeychain);
  const safeOsStoreAvailable = capabilityReport.custody?.strategy === "os_secure_store";
  const interactiveWorkflowSelected = payload.interactiveWorkflowSelected === true;
  const interactiveAuthorizationAttemptCount = Number(
    payload.interactiveAuthorizationAttemptCount || 0,
  );
  const promptBudgetSatisfied =
    interactiveAuthorizationAttemptCount <= 1 &&
    (interactiveWorkflowSelected || interactiveAuthorizationAttemptCount === 0) &&
    payload.appPasswordPromptUsed !== true &&
    payload.appCredentialPromptUsed !== true &&
    payload.automaticAuthorizationRetryUsed !== true;
  const zeroBackgroundPrompts = interactiveWorkflowSelected ||
    interactiveAuthorizationAttemptCount === 0;
  const protectedItemCreated = payload.userPresence?.itemCreated === true;
  const protectedItemCleaned = !protectedItemCreated ||
    payload.userPresence?.itemDeleted === true;
  const basicItemsCleaned =
    (!payload.standardKeychain?.itemCreated || payload.standardKeychain?.itemDeleted === true) &&
    (!payload.dataProtectionKeychain?.itemCreated ||
      payload.dataProtectionKeychain?.itemDeleted === true);
  const singleAuthorizationContextUsed =
    interactiveAuthorizationAttemptCount === 1 &&
    payload.singleAuthorizationContextCreated === true &&
    payload.singleAuthorizationContextSharedByOperations === true;
  const singleAuthorizationContextPolicySatisfied =
    interactiveAuthorizationAttemptCount === 0 || singleAuthorizationContextUsed;

  return {
    exactCapabilitySetValid: capabilityValidation.ok === true,
    safeOsStoreAvailable,
    standardKeychainAvailable,
    dataProtectionKeychainAvailable,
    strongestObservedKeychainConfiguration: dataProtectionKeychainAvailable
      ? "data_protection_keychain"
      : standardKeychainAvailable
        ? "standard_keychain"
        : "memory_only_ephemeral",
    deviceOnlyAccessibilityObserved:
      payload.standardKeychain?.deviceOnlyAccessibilityObserved === true ||
      payload.dataProtectionKeychain?.deviceOnlyAccessibilityObserved === true,
    localAuthenticationAvailable: payload.localAuthenticationAvailable === true,
    biometricMechanismAvailable: payload.biometricAuthenticationAvailable === true,
    userPresenceOperationSupported: userPresenceOperationSucceeded(payload),
    secureEnclaveOperationSupported: payload.secureEnclaveOperationSucceeded === true,
    interactiveWorkflowSelected,
    interactiveAuthorizationAttemptCount,
    interactiveAuthorizationSucceeded: payload.interactiveAuthorizationSucceeded === true,
    interactiveAuthorizationTimedOut: payload.interactiveAuthorizationTimedOut === true,
    singleAuthorizationContextUsed,
    singleAuthorizationContextPolicySatisfied,
    promptBudgetSatisfied,
    zeroBackgroundPrompts,
    noAutomaticAuthorizationRetry: payload.automaticAuthorizationRetryUsed !== true,
    appPasswordPromptUsed: payload.appPasswordPromptUsed === true,
    appCredentialPromptUsed: payload.appCredentialPromptUsed === true,
    helperSignatureValid: helper.signatureValid === true,
    helperSignatureMode: helper.signatureMode,
    basicItemsCleaned,
    protectedItemCleaned,
    adaptiveCustodyProofReady:
      helper.signatureValid === true &&
      helper.ran === true &&
      safeOsStoreAvailable &&
      capabilityValidation.ok === true &&
      promptBudgetSatisfied &&
      zeroBackgroundPrompts &&
      singleAuthorizationContextPolicySatisfied &&
      basicItemsCleaned &&
      protectedItemCleaned,
  };
}

export function observedProjection(payload, helper) {
  return {
    signatureMode: helper.signatureMode,
    signedHelperRan: helper.ran === true,
    signedEntitlementSetApplied: helper.entitlementsApplied === true,
    standardKeychain: sanitizeStoreProbe(payload.standardKeychain),
    dataProtectionKeychain: sanitizeStoreProbe(payload.dataProtectionKeychain),
    userPresence: {
      selectedStore: String(payload.userPresence?.selectedStore || "none"),
      accessControlCreated: payload.userPresence?.accessControlCreated === true,
      itemCreated: payload.userPresence?.itemCreated === true,
      nonInteractiveReadBlocked: payload.userPresence?.nonInteractiveReadBlocked === true,
      authorizedReadSucceeded: payload.userPresence?.authorizedReadSucceeded === true,
      itemDeleted: payload.userPresence?.itemDeleted === true,
    },
    localAuthentication: {
      deviceOwnerAuthenticationAvailable: payload.localAuthenticationAvailable === true,
      biometricMechanismAvailable: payload.biometricAuthenticationAvailable === true,
    },
    secureEnclave: {
      privateKeyOperationSucceeded: payload.secureEnclaveOperationSucceeded === true,
    },
    interaction: {
      workflowSelected: payload.interactiveWorkflowSelected === true,
      authorizationAttemptCount: Number(payload.interactiveAuthorizationAttemptCount || 0),
      authorizationSucceeded: payload.interactiveAuthorizationSucceeded === true,
      authorizationTimedOut: payload.interactiveAuthorizationTimedOut === true,
      automaticRetryUsed: payload.automaticAuthorizationRetryUsed === true,
      singleAuthorizationContextUsed:
        Number(payload.interactiveAuthorizationAttemptCount || 0) === 1 &&
        payload.singleAuthorizationContextSharedByOperations === true,
    },
  };
}

export function sanitizeStoreProbe(probe) {
  return {
    itemCreated: probe?.itemCreated === true,
    readMatched: probe?.readMatched === true,
    itemDeleted: probe?.itemDeleted === true,
    deviceOnlyAccessibilityObserved: probe?.deviceOnlyAccessibilityObserved === true,
  };
}
