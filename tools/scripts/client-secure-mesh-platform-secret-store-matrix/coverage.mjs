import { validateCapabilityReport } from "../lib/secure-mesh-capability-report.mjs";
import {
  androidPlatformCryptoCoverage as sharedAndroidPlatformCryptoCoverage,
  redactedReportReady,
  windowsImplementationCoverage as sharedWindowsImplementationCoverage
} from "../lib/secure-mesh-physical-report-coverage.mjs";

export { redactedReportReady };

function exactCapabilityReportValid(report) {
  try {
    return validateCapabilityReport(report)?.ok === true;
  } catch {
    return false;
  }
}

export function createCoverage(physicalReportRefs) {
  function androidPlatformCryptoCoverage(report) {
    return sharedAndroidPlatformCryptoCoverage(report, {
      reportRef: physicalReportRefs.androidPlatformCrypto
    });
  }

  function androidInstallLaunchCoverage(report) {
    const summary = report?.summary || {};
    const physicalDeviceProofReady =
      summary.androidPhysicalDeviceProofReady === true ||
      report?.androidPhysicalDeviceProofReady === true ||
      report?.physicalDevice === true;
    return {
      report: physicalReportRefs.androidInstallLaunch,
      present: Boolean(report && Object.keys(report).length > 0),
      ok: report?.ok === true,
      redacted: report?.redacted === true,
      physicalDeviceProofReady,
      androidSystemCredentialAuthReady:
        summary.androidSystemCredentialAuthReady === true ||
        report?.androidSystemCredentialAuthReady === true,
      androidKeyStoreHardwareAuthReady:
        summary.androidKeyStoreHardwareAuthReady === true ||
        report?.androidKeyStoreHardwareAuthReady === true,
      androidKeyStoreSecurityLevelName: String(
        summary.androidKeyStoreSecurityLevelName ||
        report?.androidKeyStoreSecurityLevelName ||
        ""
      ),
      androidKeyStoreInsideSecureHardware:
        summary.androidKeyStoreInsideSecureHardware === true ||
        report?.androidKeyStoreInsideSecureHardware === true,
      androidKeyStoreUserAuthenticationHardwareEnforced:
        summary.androidKeyStoreUserAuthenticationHardwareEnforced === true ||
        report?.androidKeyStoreUserAuthenticationHardwareEnforced === true,
      androidKeyStoreUnlockedDeviceRequired:
        summary.androidKeyStoreUnlockedDeviceRequired === true ||
        report?.androidKeyStoreUnlockedDeviceRequired === true,
      runtimeStatusRedacted:
        summary.runtimeStatusRedacted === true || report?.runtimeStatusRedacted === true,
      rawPayloadExportSurfaceAbsent:
        summary.rawPayloadExportSurfaceAbsent === true ||
        report?.rawPayloadExportSurfaceAbsent === true
    };
  }

  function macosPlatformCryptoCoverage(report) {
    const summary = report?.summary || {};
    const capabilityReport = report?.capabilityReport || {};
    const enabledCapabilities = Array.isArray(capabilityReport.enabled)
      ? [...capabilityReport.enabled]
      : [];
    const custodyStrategy = String(capabilityReport?.custody?.strategy || "");
    const exactCapabilitySetValid = report?.ok === true &&
      exactCapabilityReportValid(capabilityReport);
    const safeOsStoreAvailable = summary.safeOsStoreAvailable === true &&
      custodyStrategy === "os_secure_store";
    const singleSystemAuthorizationContextVerified =
      summary.singleAuthorizationContextUsed === true ||
      summary.singleSystemAuthorizationContextVerified === true;
    const promptBudgetSatisfied = summary.promptBudgetSatisfied === true &&
      Number(summary.interactiveAuthorizationAttemptCount || 0) <= 1 &&
      summary.appPasswordPromptUsed !== true &&
      summary.appCredentialPromptUsed !== true &&
      summary.noAutomaticAuthorizationRetry === true;
    return {
      report: physicalReportRefs.macosUserPresenceProof,
      present: Boolean(report && Object.keys(report).length > 0),
      ready: redactedReportReady(report) &&
        report?.platform === "macos" &&
        safeOsStoreAvailable &&
        exactCapabilitySetValid &&
        singleSystemAuthorizationContextVerified &&
        promptBudgetSatisfied,
      capabilityReportPresent: Object.keys(capabilityReport).length > 0,
      enabledCapabilities,
      custodyStrategy,
      exactCapabilitySetValid,
      safeOsStoreAvailable,
      standardKeychainAvailable: summary.standardKeychainAvailable === true,
      dataProtectionKeychainAvailable:
        summary.dataProtectionKeychainAvailable === true,
      userPresenceOperationSupported:
        summary.userPresenceOperationSupported === true,
      secureEnclaveOperationSupported:
        summary.secureEnclaveOperationSupported === true,
      appPasswordPromptUsed: summary.appPasswordPromptUsed === true,
      appCredentialPromptUsed: summary.appCredentialPromptUsed === true,
      singleSystemAuthorizationContextVerified,
      promptBudgetSatisfied,
      zeroBackgroundPrompts: summary.zeroBackgroundPrompts === true,
      noAutomaticAuthorizationRetry:
        summary.noAutomaticAuthorizationRetry === true,
      singleAuthorizationContextPolicySatisfied:
        summary.singleAuthorizationContextPolicySatisfied === true,
      interactiveAuthorizationAttemptCount:
        Number(summary.interactiveAuthorizationAttemptCount || 0),
      maximumInteractiveAuthorizationAttemptsPerProof: 1
    };
  }

  function ubuntuPlatformCryptoCoverage(report) {
    const secretStore = report?.secretStore || {};
    const summary = report?.summary || {};
    const backend = String(
      report?.backend ||
      secretStore.persistentBackend ||
      summary.backend ||
      ""
    );
    const sharedSecretClassPersistenceReady =
      report?.sharedSecretClassPersistenceReady === true ||
      summary.sharedSecretClassPersistenceReady === true;
    const authorizationPolicyReady =
      report?.secretStoreAuthorizationPolicyReady === true ||
      summary.secretStoreAuthorizationPolicyReady === true;
    return {
      report: physicalReportRefs.ubuntuVmSecretStore,
      present: Boolean(report && Object.keys(report).length > 0),
      ready: report?.ok === true &&
        backend === "linux-secret-service-keyring" &&
        sharedSecretClassPersistenceReady &&
        authorizationPolicyReady,
      backend,
      sharedSecretClassPersistenceReady,
      authorizationPolicyReady
    };
  }

  function genericProofCoverage(report, ref) {
    return {
      report: ref,
      present: Boolean(report && Object.keys(report).length > 0),
      ready: report?.ok === true && report?.redacted === true
    };
  }

  function windowsImplementationCoverage(report) {
    return sharedWindowsImplementationCoverage(report, {
      reportRef: physicalReportRefs.windowsImplementation
    });
  }

  return {
    androidPlatformCryptoCoverage,
    androidInstallLaunchCoverage,
    macosPlatformCryptoCoverage,
    ubuntuPlatformCryptoCoverage,
    genericProofCoverage,
    windowsImplementationCoverage
  };
}
