import { androidPhysicalInstallLaunchReportPath } from "../config.mjs";
import { reportRecord } from "../lists.mjs";

const hasOwn = (value, key) =>
  Object.prototype.hasOwnProperty.call(value, key);

export function summarizeAndroidPhysicalInstallLaunchReport(report = {}) {
  report = reportRecord(report);
  const summary = report?.summary || {};
  const runtimeStatus = report?.runtimeStatus || {};
  const mobileRelaySecretStore = runtimeStatus.mobileRelaySecretStore || {};
  const userAuthentication = runtimeStatus.userAuthentication || {};
  const present = Boolean(report && Object.keys(report).length > 0);

  const jniSecretCallbackInProcessReady =
    mobileRelaySecretStore.ffiBoundary === "jni" &&
    mobileRelaySecretStore.secretTransport ===
      "jni_callback_in_process_secret_bytes" &&
    mobileRelaySecretStore.decryptedSecretCrossesJniInProcess === true;
  const mobileRelaySecretStoreContractReady =
    jniSecretCallbackInProcessReady &&
    mobileRelaySecretStore.provider === "AndroidKeyStore" &&
    mobileRelaySecretStore.secretStoreBackend === "android-keystore" &&
    mobileRelaySecretStore.secretStoreContract ===
      "rust_secure_mesh_secret_store_handle_v1" &&
    mobileRelaySecretStore.secretStoreAccountPrefix === "mobileRelayE2ee" &&
    mobileRelaySecretStore.secretStoreNamespace === "mobileRelayRuntime" &&
    mobileRelaySecretStore.sharedRustSecretStoreHandleContract === true &&
    mobileRelaySecretStore.portableConfigAuthority === "rust_generation_cas" &&
    mobileRelaySecretStore.kotlinConfigReadWrite === false &&
    mobileRelaySecretStore.getNotFoundSeparatedFromFailure === true;
  const rawJsonSecretOverridesUsedPresent =
    hasOwn(mobileRelaySecretStore, "rawJsonSecretOverridesUsed");
  const rawJsonSecretOverridesUnknown = rawJsonSecretOverridesUsedPresent !== true;
  const rawJsonSecretOverridesProvenAbsent =
    rawJsonSecretOverridesUnknown !== true &&
    mobileRelaySecretStore.rawJsonSecretOverridesProvenAbsent === true &&
    mobileRelaySecretStore.rawJsonSecretOverridesUsed === false;
  const androidKeyMaterialExportedPresent =
    hasOwn(mobileRelaySecretStore, "androidKeyMaterialExported");
  const statusProbeSideEffectFree =
    mobileRelaySecretStore.statusProbeSideEffectFree === true;
  const freshOneShotAuthorizationPolicyReady =
    runtimeStatus.freshOneShotAuthorizationPolicyReady === true &&
    userAuthentication.physicalUserPresenceRequired === true &&
    userAuthentication.systemAuthenticationOnly === true &&
    userAuthentication.appLockScreenCredentialCollection === false &&
    userAuthentication.appCredentialPromptUsed === false &&
    userAuthentication.appPasswordPromptUsed === false &&
    userAuthentication.systemCredentialPromptReused === false &&
    userAuthentication.systemCredentialPromptReusedFromPendingRequest === false &&
    userAuthentication.authorizationGrantPersisted === false &&
    userAuthentication.authorizationGrantExtendedByDispatch === false;
  const androidSystemCredentialAuthReady =
    mobileRelaySecretStore.applicationAuthorizationGrantRequired === true &&
    freshOneShotAuthorizationPolicyReady;
  const localReadyDiagnostic =
    report?.schemaVersion ===
      "licomesh.secure-mesh.android-physical-install-launch-report.v3" &&
    report?.ok === true &&
    report?.physicalDevice === true &&
    summary.apkReady === true &&
    summary.installReady === true &&
    summary.launchReady === true &&
    summary.runtimeStatusReady === true &&
    summary.nativeRuntimeReady === true &&
    summary.androidCustodyReady === true &&
    summary.adaptiveAuthorizationReady === true &&
    summary.evidenceBindingReady === true &&
    mobileRelaySecretStoreContractReady &&
    rawJsonSecretOverridesProvenAbsent &&
    androidKeyMaterialExportedPresent &&
    mobileRelaySecretStore.androidKeyMaterialExported === false &&
    statusProbeSideEffectFree &&
    androidSystemCredentialAuthReady;

  return {
    report: androidPhysicalInstallLaunchReportPath,
    present,
    ok: report?.ok === true,
    physicalDevice: report?.physicalDevice === true,
    packageName: String(report?.packageName || ""),
    apkReady: summary.apkReady === true,
    installReady: summary.installReady === true,
    launchReady: summary.launchReady === true,
    runtimeStatusReady: summary.runtimeStatusReady === true,
    nativeRuntimeReady: summary.nativeRuntimeReady === true,
    androidCustodyReady: summary.androidCustodyReady === true,
    adaptiveAuthorizationReady: summary.adaptiveAuthorizationReady === true,
    evidenceBindingReady: summary.evidenceBindingReady === true,
    mobileRelaySecretStoreContractReady,
    jniSecretCallbackInProcessReady,
    statusProbeSideEffectFree,
    freshOneShotAuthorizationPolicyReady,
    rawJsonSecretOverridesUnknown,
    rawJsonSecretOverridesProvenAbsent,
    androidKeyMaterialExportedPresent,
    androidKeyMaterialExported:
      mobileRelaySecretStore.androidKeyMaterialExported === true,
    appCredentialPromptUsed:
      userAuthentication.appCredentialPromptUsed === true,
    appCredentialPromptUsedPresent:
      hasOwn(userAuthentication, "appCredentialPromptUsed"),
    appPasswordPromptUsed:
      userAuthentication.appPasswordPromptUsed === true,
    appPasswordPromptUsedPresent:
      hasOwn(userAuthentication, "appPasswordPromptUsed"),
    androidSystemCredentialAuthReady,
    localReadyDiagnostic,
  };
}
