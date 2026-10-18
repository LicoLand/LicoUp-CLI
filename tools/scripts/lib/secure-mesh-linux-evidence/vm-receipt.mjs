import { validateCapabilityReport } from "../secure-mesh-capability-report.mjs";
import {
  VM_KEYS,
  linuxVmPackageReceiptSchema,
  linuxVmPackageReceiptSchemaVersion,
} from "./constants.mjs";
import { LinuxEvidenceValidationError, assertRule } from "./error.mjs";
import { isPlainObject, scanPrivacy } from "./shared.mjs";

export function validateLinuxVmPackageReceipt(
  report,
  expectedSourceDigest = "",
  expectedProductVersion = "",
  expectedBuildNumber = 0,
) {
  try {
    return validateLinuxVmPackageReceiptInternal(
      report,
      expectedSourceDigest,
      expectedProductVersion,
      expectedBuildNumber,
    );
  } catch (error) {
    if (error instanceof LinuxEvidenceValidationError) throw error;
    throw new LinuxEvidenceValidationError(
      "linux_vm_validator_internal_operation_failed",
      "schema",
    );
  }
}

function validateLinuxVmPackageReceiptInternal(
  report,
  expectedSourceDigest,
  expectedProductVersion,
  expectedBuildNumber,
) {
  assertRule(isPlainObject(report) && Object.keys(report).length === VM_KEYS.length &&
    VM_KEYS.every((key) => Object.hasOwn(report, key)),
  "linux_vm_receipt_fields_exact", "schema");
  assertRule(report.schema === linuxVmPackageReceiptSchema,
    "linux_vm_receipt_schema_match", "schema");
  assertRule(report.schemaVersion === linuxVmPackageReceiptSchemaVersion,
    "linux_vm_receipt_schema_version_match", "schema");
  assertRule(Number.isFinite(Date.parse(String(report.generatedAt || ""))),
    "linux_vm_receipt_generated_at_valid", "schema");
  assertRule(/^sha256:[a-f0-9]{64}$/u.test(String(report.closureChallengeDigest || "")),
    "linux_vm_closure_challenge_digest_valid", "binding");
  assertRule(/^sha256:[a-f0-9]{64}$/u.test(String(report.invocationNonceDigest || "")),
    "linux_vm_invocation_nonce_digest_valid", "binding");
  assertRule(typeof report.productVersion === "string" && report.productVersion.trim() !== "",
    "linux_vm_product_version_present", "binding");
  assertRule(Number.isInteger(report.buildNumber) && report.buildNumber > 0,
    "linux_vm_build_number_valid", "binding");
  if (expectedProductVersion) {
    assertRule(report.productVersion === expectedProductVersion,
      "linux_vm_product_version_match", "binding");
  }
  if (expectedBuildNumber) {
    assertRule(report.buildNumber === expectedBuildNumber,
      "linux_vm_build_number_match", "binding");
  }
  assertRule(report.nonBlockingDistributionGuidance?.blocking === false,
    "linux_vm_distribution_guidance_non_blocking", "readiness");
  assertRule(report.redacted === true && report.reportLeakScan === true,
    "linux_vm_root_redaction_declared", "privacy");
  for (const key of [
    "rawPrivateMaterialIncluded",
    "rawPlaintextIncluded",
    "rawPublicWireBytesIncluded",
  ]) {
    assertRule(report[key] === false, `linux_vm_root_${key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase()}_false`,
      "privacy");
  }
  const binding = report.sourceBinding;
  const bindingKeys = [
    "sourceStateDigest",
    "sourceStateDigestProvenance",
    "archiveDigest",
    "bundleManifestDigest",
    "nativeClientDigest",
    "stale",
  ];
  assertRule(isPlainObject(binding) && Object.keys(binding).length === bindingKeys.length &&
    bindingKeys.every((key) => Object.hasOwn(binding, key)),
  "linux_vm_source_binding_fields_exact", "schema");
  for (const [key, ruleId] of [
    ["sourceStateDigest", "linux_vm_source_state_digest_valid"],
    ["archiveDigest", "linux_vm_archive_digest_valid"],
    ["bundleManifestDigest", "linux_vm_bundle_manifest_digest_valid"],
    ["nativeClientDigest", "linux_vm_native_client_digest_valid"],
  ]) {
    assertRule(/^sha256:[a-f0-9]{64}$/u.test(String(binding[key] || "")), ruleId, "binding");
  }
  assertRule(["git-worktree", "vm-orchestrator-verified"].includes(
    binding.sourceStateDigestProvenance),
  "linux_vm_source_provenance_valid", "binding");
  assertRule(binding.stale === false, "linux_vm_source_not_stale", "binding");
  if (expectedSourceDigest) {
    assertRule(/^sha256:[a-f0-9]{64}$/u.test(String(expectedSourceDigest)),
      "linux_vm_expected_source_digest_valid", "binding");
    assertRule(binding.sourceStateDigest === expectedSourceDigest,
      "linux_vm_expected_source_digest_match", "binding");
  }
  const packageKeys = [
    "format",
    "layoutClasses",
    "executableCount",
    "signaturePresent",
    "validationSignature",
    "signatureVerified",
    "archiveDigestVerified",
    "bundleManifestDigestVerified",
    "installedFromArchive",
  ];
  assertRule(isPlainObject(report.package) &&
    Object.keys(report.package).length === packageKeys.length &&
    packageKeys.every((key) => Object.hasOwn(report.package, key)),
  "linux_vm_package_fields_exact", "schema");
  assertRule(report.package.format === "tar.gz", "linux_vm_archive_format_valid", "artifact");
  assertRule(
    JSON.stringify(report.package.layoutClasses) === JSON.stringify([
      "desktop_executable",
      "native_sidecar",
      "flutter_assets",
      "package_metadata"
    ]), "linux_vm_archive_layout_valid", "artifact"
  );
  assertRule(report.package.executableCount === 2,
    "linux_vm_package_executable_count_valid", "artifact");
  for (const key of [
    "signaturePresent",
    "validationSignature",
    "signatureVerified",
    "archiveDigestVerified",
    "bundleManifestDigestVerified",
    "installedFromArchive"
  ]) {
    const snake = key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase();
    assertRule(report.package[key] === true, `linux_vm_package_${snake}_ready`, "artifact");
  }
  const sessionKeys = [
    "kind",
    "clientStarted",
    "visibleWindow",
    "interactionSmoke",
    "boundedShutdown",
  ];
  assertRule(isPlainObject(report.session) &&
    Object.keys(report.session).length === sessionKeys.length &&
    sessionKeys.every((key) => Object.hasOwn(report.session, key)),
  "linux_vm_session_fields_exact", "schema");
  assertRule(report.session.kind === "x11_virtual_display",
    "linux_vm_session_kind_valid", "session");
  for (const key of ["clientStarted", "visibleWindow", "interactionSmoke", "boundedShutdown"]) {
    const snake = key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase();
    assertRule(report.session[key] === true, `linux_vm_session_${snake}_ready`, "session");
  }
  const smokeKeys = ["cliTargetScan", "guiSession", "exactCapabilitySchema"];
  assertRule(isPlainObject(report.smoke) && Object.keys(report.smoke).length === smokeKeys.length &&
    smokeKeys.every((key) => Object.hasOwn(report.smoke, key)),
  "linux_vm_smoke_fields_exact", "schema");
  for (const key of smokeKeys) {
    const snake = key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase();
    assertRule(report.smoke[key] === true, `linux_vm_smoke_${snake}_ready`, "readiness");
  }
  try {
    validateCapabilityReport(report.capabilityReport);
  } catch {
    throw new LinuxEvidenceValidationError("linux_vm_capability_report_valid", "capability");
  }
  const privacyKeys = [
    "redacted",
    "runtimeIdentityIncluded",
    "localPathIncluded",
    "dbusOrObjectDataIncluded",
    "rawLogsIncluded",
    "rawPlaintextIncluded",
    "rawCiphertextIncluded",
    "rawSecretsIncluded",
  ];
  assertRule(isPlainObject(report.privacy) &&
    Object.keys(report.privacy).length === privacyKeys.length &&
    privacyKeys.every((key) => Object.hasOwn(report.privacy, key)),
  "linux_vm_privacy_fields_exact", "schema");
  assertRule(report.privacy.redacted === true, "linux_vm_privacy_redacted", "privacy");
  for (const key of privacyKeys.filter((key) => key !== "redacted")) {
    const snake = key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase();
    assertRule(report.privacy[key] === false, `linux_vm_privacy_${snake}_false`, "privacy");
  }
  const summaryKeys = [
    "currentSourceArchive",
    "installReceiptReady",
    "sessionLaunchReady",
    "smokeReady",
    "privacyReady",
  ];
  assertRule(isPlainObject(report.summary) &&
    Object.keys(report.summary).length === summaryKeys.length &&
    summaryKeys.every((key) => Object.hasOwn(report.summary, key)),
  "linux_vm_summary_fields_exact", "schema");
  for (const key of summaryKeys) {
    const snake = key.replaceAll(/([A-Z])/gu, "_$1").toLowerCase();
    assertRule(report.summary[key] === true, `linux_vm_summary_${snake}`, "readiness");
  }
  assertRule(report.ok === true, "linux_vm_receipt_ready", "readiness");
  try {
    scanPrivacy(report);
  } catch {
    throw new LinuxEvidenceValidationError("linux_vm_privacy_value_scan_clean", "privacy");
  }
  return Object.freeze({ ok: true, sourceStateDigest: report.sourceBinding.sourceStateDigest });
}
