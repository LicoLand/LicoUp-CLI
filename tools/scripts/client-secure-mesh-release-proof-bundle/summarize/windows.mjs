import { windowsImplementationReportPath } from "../config.mjs";
import { reportRecord } from "../lists.mjs";
import {
  windowsImplementationReady,
  windowsPersistentCustodyBoundaryValid,
} from "../../lib/secure-mesh-physical-report-coverage.mjs";

export function summarizeWindowsImplementation(report = {}) {
  report = reportRecord(report);
  const summary = report?.summary || {};
  const present = Boolean(report && Object.keys(report).length > 0);
  const conservativeBoundaryValid =
    windowsPersistentCustodyBoundaryValid(report);
  const ready = windowsImplementationReady(report);
  return {
    report: windowsImplementationReportPath,
    present,
    ok: report?.ok === true,
    redacted: report?.redacted === true,
    blocker: String(report?.blocker || ""),
    diagnosticStatus: String(report?.diagnosticStatus || ""),
    conservativeBoundaryValid,
    windowsLocalBlockersCleared: summary.windowsLocalBlockersCleared === true,
    nativeHostEvidencePending: summary.nativeHostEvidencePending === true,
    dpapiOrWindowsHelloProofReady: summary.dpapiOrWindowsHelloProofReady === true,
    windowsSignedInstallerProofReady: summary.windowsSignedInstallerProofReady === true,
    productionReady: report?.productionReady === true,
    releaseReady: report?.releaseReady === true,
    ready
  };
}
