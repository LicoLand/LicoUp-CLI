import path from "node:path";
import { fileURLToPath } from "node:url";
import { CANONICAL_CLIENT_SOURCE_ROOTS } from "../lib/client-source-state-digest.mjs";

export const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
export const verifier = "tools/scripts/client-secure-mesh-macos-capabilities.mjs";
export const schemaVersion = "licomesh.secure-mesh.macos-adaptive-capabilities-receipt.v3";
export const reportRef = "build/reports/secure-mesh-macos-capabilities.json";
export const builtAppRef = "build/apps/desktop/runnable/macos/release/LicoUp.app";
export const builtApp = path.join(repoRoot, builtAppRef);
export const installedApp = "/Applications/LicoUp.app";
export const capabilityProofRef =
  "build/reports/secure-mesh-macos-keychain-user-presence-proof.json";
export const packageManifestRef =
  "build/apps/desktop/runnable/macos/release/package-metadata/licoup/packaging-modules.json";
export const packageManifestPath = path.join(repoRoot, packageManifestRef);
export const releaseEntitlementsRef = "apps/desktop/macos/Runner/Release.entitlements";
export const releaseEntitlementsPath = path.join(repoRoot, releaseEntitlementsRef);
export const clientVersionPath = path.join(repoRoot, "tools/client-version.json");
export const sourceRoots = CANONICAL_CLIENT_SOURCE_ROOTS;
export const sha256Pattern = /^sha256:[a-f0-9]{64}$/u;
