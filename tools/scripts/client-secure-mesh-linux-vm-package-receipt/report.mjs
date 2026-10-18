import path from "node:path";
import process from "node:process";
import {
  LinuxEvidenceValidationError,
  linuxVmReceiptWriteFailure,
} from "../lib/secure-mesh-linux-evidence.mjs";
import {
  atomicWriteReportJson,
  SafeReportWriteError,
} from "../lib/safe-report-io.mjs";
import { requiredOption } from "./cli.mjs";
import { assert } from "./util.mjs";

export function writeReport(options, report) {
  let destination;
  try {
    destination = safeReportDestination(options);
  } catch {
    throw new LinuxEvidenceValidationError(
      "linux_vm_receipt_write_destination_invalid",
      "producer",
    );
  }
  try {
    JSON.stringify(report);
  } catch {
    throw new LinuxEvidenceValidationError(
      "linux_vm_receipt_write_payload_not_serializable",
      "producer",
    );
  }
  try {
    atomicWriteReportJson(destination.root, destination.ref, report);
  } catch (error) {
    if (error instanceof SafeReportWriteError) {
      throw linuxVmReceiptWriteFailure(error.stage);
    }
    throw new LinuxEvidenceValidationError(
      "linux_vm_receipt_write_atomic_publish_failed",
      "producer",
    );
  }
}

export function writeFailureReceipt(options, failureRecord) {
  if (!options.report) return;
  const { root, ref } = safeReportDestination(options);
  atomicWriteReportJson(root, ref, failureRecord);
}

export function safeReportDestination(options) {
  const rootValue = String(process.env.LICO_LINUX_VM_REPORT_ROOT || "").trim();
  assert(rootValue, "Linux VM report root is missing");
  const root = path.resolve(rootValue);
  const target = path.resolve(requiredOption(options, "report"));
  const relative = path.relative(root, target);
  assert(relative && !relative.startsWith("..") && !path.isAbsolute(relative),
    "Linux VM report path escapes its allowed root");
  return { root, ref: relative };
}
