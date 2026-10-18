import { SAFE_REPORT_WRITE_STAGES } from "../safe-report-io.mjs";
import {
  LINUX_EVIDENCE_FAILURE_CATEGORIES,
  LINUX_EVIDENCE_RULE_ID,
  LINUX_VM_FAILURE_PHASES,
  linuxVmPackageReceiptSchema,
  linuxVmPackageReceiptSchemaVersion,
} from "./constants.mjs";
import {
  LinuxEvidenceValidationError,
  assertRule,
  classifyLinuxEvidenceValidationFailure,
} from "./error.mjs";
import { linuxEvidencePrivacyRecord } from "./shared.mjs";

export function createLinuxVmPackageFailureRecord(phase, failure) {
  assertRule(LINUX_VM_FAILURE_PHASES.has(phase),
    "linux_vm_failure_phase_valid", "schema");
  assertRule(failure && LINUX_EVIDENCE_RULE_ID.test(String(failure.ruleId || "")),
    "linux_vm_failure_rule_id_valid", "schema");
  assertRule(LINUX_EVIDENCE_FAILURE_CATEGORIES.has(failure.category),
    "linux_vm_failure_category_valid", "schema");
  return Object.freeze({
    schema: linuxVmPackageReceiptSchema,
    schemaVersion: linuxVmPackageReceiptSchemaVersion,
    ok: false,
    artifactKind: "linux-vm-installed-client",
    reason: "linux_vm_package_receipt_incomplete",
    phase,
    validationRuleId: failure.ruleId,
    failureCategory: failure.category,
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    privacy: linuxEvidencePrivacyRecord(),
  });
}

export function classifyLinuxVmProducerFailure(phase, error) {
  if (phase === "receipt_validation") {
    return classifyLinuxEvidenceValidationFailure(
      error,
      "linux_vm_receipt_validation_unclassified",
    );
  }
  if (phase === "receipt_write" && error instanceof LinuxEvidenceValidationError) {
    return classifyLinuxEvidenceValidationFailure(
      error,
      "linux_vm_producer_receipt_write_failed",
    );
  }
  const phaseRules = Object.freeze({
    input_validation: "linux_vm_producer_input_validation_failed",
    archive_binding: "linux_vm_producer_archive_binding_failed",
    archive_install: "linux_vm_producer_archive_install_failed",
    cli_smoke: "linux_vm_producer_cli_smoke_failed",
    gui_session: "linux_vm_producer_gui_session_failed",
    gui_display: "linux_vm_producer_gui_display_failed",
    gui_process: "linux_vm_producer_gui_process_failed",
    gui_window: "linux_vm_producer_gui_window_failed",
    gui_interaction: "linux_vm_producer_gui_interaction_failed",
    gui_shutdown: "linux_vm_producer_gui_shutdown_failed",
    gui_stderr: "linux_vm_producer_gui_stderr_failed",
    receipt_write: "linux_vm_producer_receipt_write_failed",
  });
  return Object.freeze({
    ruleId: phaseRules[phase] || "linux_vm_producer_unclassified_failure",
    category: "producer",
  });
}

export function linuxVmReceiptWriteFailure(stage) {
  if (!SAFE_REPORT_WRITE_STAGES.includes(stage)) {
    return new LinuxEvidenceValidationError(
      "linux_vm_receipt_write_stage_invalid",
      "producer",
    );
  }
  return new LinuxEvidenceValidationError(
    `linux_vm_receipt_write_${stage}_failed`,
    "producer",
  );
}
