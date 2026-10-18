import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadSecureClientContract } from "../lib/secure-client-contract.mjs";
import { loadSecureMeshReleaseProofConfig } from "../lib/secure-mesh-release-proof-config.mjs";

export const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
export const releaseProofConfig = await loadSecureMeshReleaseProofConfig();
export const reportPath = releaseProofConfig.reportOutput;
export const {
  updateRelease: updateReleaseReportPath,
  physicalMatrix: physicalMatrixReportPath,
  androidPhysicalInstallLaunch: androidPhysicalInstallLaunchReportPath,
  physicalEvidenceManifest: physicalEvidenceManifestReportPath,
  windowsImplementation: windowsImplementationReportPath,
  reportRedaction: reportRedactionReportPath,
  stationAcceptance: stationAcceptanceReportPath,
  rustCrypto: rustCryptoReportPath,
  platformCrypto: platformCryptoReportPath,
  androidPlatformCrypto: androidPlatformCryptoReportPath
} = releaseProofConfig.inputReports;
export const {
  updateRelease: updateReleaseVerifierCommand,
  physicalEvidenceManifest: physicalEvidenceManifestVerifierCommand,
  reportRedaction: reportRedactionVerifierCommand
} = releaseProofConfig.verifierCommands;
export const sourceChecks = Object.freeze(releaseProofConfig.sourceChecks);
export const freshnessWindows = Object.freeze(releaseProofConfig.freshnessWindows);

export const contract = await loadSecureClientContract();
export const {
  evaluateSecureClientMeshEvidenceRefReportReadiness,
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} = contract;
