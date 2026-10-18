import { createHash, createPrivateKey, createPublicKey, verify } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  renameSync,
  statSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import {
  linuxEvidencePrivacyRecord,
  linuxVmPackageReceiptSchema,
  linuxVmPackageReceiptSchemaVersion,
} from "../lib/secure-mesh-linux-evidence.mjs";
import {
  resolveContainedExistingPath,
  sha256File as stableSha256File,
  stableReadFile,
  stableSnapshotFile,
} from "../lib/client-release-artifact-digest.mjs";
import {
  inspectLinuxTarGzipArchive,
  LINUX_TAR_RESOURCE_LIMITS,
} from "../lib/linux-tar-resource-bounds.mjs";
import { runGuiSession } from "./gui.mjs";
import {
  assert,
  assertArm64Executable,
  assertTargetScan,
  decodeCanonicalBase64,
  requiredFile,
  run,
  runJson,
  sha256File,
} from "./util.mjs";

export async function runReceipt(ctx) {
  const { options, workRoot, repoRoot, releaseContext } = ctx;
  assert(process.platform === "linux", "Linux VM package receipt requires Linux");
  assert(["arm64", "aarch64"].includes(process.arch), "Linux VM package receipt requires ARM64");
  const archive = requiredFile(options.archive, "Linux archive");
  const verificationManifestPath = requiredFile(
    options.verificationManifest,
    "Linux verification manifest"
  );
  const signaturePath = requiredFile(`${archive}.sig`, "Linux archive signature");
  const validationKeyPath = requiredFile(
    process.env.LICO_LINUX_VERIFICATION_SIGNING_KEY_PATH,
    "Linux VM validation signing key"
  );
  const distribution = JSON.parse(stableReadFile(verificationManifestPath, {
    maxBytes: 2 * 1024 * 1024,
  }).toString("utf8"));
  const versionManifestPath = resolveContainedExistingPath(
    repoRoot,
    path.join(repoRoot, "tools/client-version.json"),
    { expectedKind: "file" },
  );
  const versionManifest = JSON.parse(stableReadFile(versionManifestPath, {
    maxBytes: 1024 * 1024,
  }).toString("utf8"));
  assert(String(versionManifest.productVersion || "").trim() !== "" &&
    Number.isInteger(versionManifest.buildNumber) && versionManifest.buildNumber > 0,
  "Linux client version manifest is invalid");
  ctx.verificationPhase = "archive_binding";
  const stableArchive = stableSnapshotFile(
    archive,
    workRoot,
    "release-archive.tar.gz",
    { maxBytes: LINUX_TAR_RESOURCE_LIMITS.maxCompressedBytes },
  );
  const archiveDigest = stableSha256File(stableArchive);
  assert(distribution.schemaVersion === "licomesh.client-linux.verification-carrier.v1" &&
    distribution.mode === "verification" && distribution.verificationReady === true &&
    distribution.publicReleaseBlocked === true,
  "Linux verification carrier policy is invalid");
  assert(distribution.targetId === "linux-glibc-arm64", "Linux distribution target is invalid");
  assert(distribution.archive === path.basename(archive), "Linux distribution archive binding is invalid");
  assert(distribution.sha256 === archiveDigest.slice("sha256:".length),
    "Linux distribution archive digest is invalid");
  assert(distribution.sourceStateDigest === options.expectedSourceDigest,
    "Linux archive source-state digest is stale");
  assert(distribution.productVersion === versionManifest.productVersion &&
    distribution.buildNumber === versionManifest.buildNumber,
  "Linux distribution version binding is invalid");
  assert(distribution.signature?.algorithm === "Ed25519" &&
    distribution.signature?.payload === "archive-sha256-digest" &&
    statSync(signaturePath).size > 0,
    "Linux archive signature is incomplete");
  assert(distribution.signature?.keyId === "linux-vm-acceptance",
    "Linux VM archive does not carry the validation-only signature marker");
  assert(distribution.signature?.file === path.basename(signaturePath),
    "Linux VM archive signature file binding is invalid");
  const privateKey = createPrivateKey(stableReadFile(validationKeyPath, {
    maxBytes: 64 * 1024,
  }));
  assert(privateKey.asymmetricKeyType === "ed25519",
    "Linux VM validation signing key is not Ed25519");
  const publicKey = createPublicKey(privateKey);
  const publicKeyDer = publicKey.export({ type: "spki", format: "der" });
  const publicKeyFingerprint = `sha256:${createHash("sha256")
    .update(publicKeyDer)
    .digest("hex")}`;
  const declaredPublicKeyDer = decodeCanonicalBase64(
    distribution.signature?.publicKeySpkiBase64,
    "Linux archive public verification key",
  );
  assert(declaredPublicKeyDer.equals(publicKeyDer),
    "Linux archive public verification key does not match the validation key");
  const signature = Buffer.from(stableReadFile(signaturePath, {
    maxBytes: 16 * 1024,
  }).toString("utf8").trim(), "base64");
  assert(signature.length === 64 && verify(
    null,
    Buffer.from(archiveDigest.slice("sha256:".length), "hex"),
    publicKey,
    signature,
  ),
    "Linux VM archive signature verification failed");
  assert(distribution.signature?.publicKeyFingerprint === publicKeyFingerprint,
    "Linux VM archive signature fingerprint is invalid");

  const archiveEntries = inspectLinuxTarGzipArchive(stableArchive).entries;
  ctx.verificationPhase = "archive_install";
  assert(archiveEntries.length > 0, "Linux archive is empty");
  assert(
    archiveEntries.every((entry) =>
      entry === "bundle" || entry.startsWith("bundle/") || entry === "bundle/"
    ),
    "Linux archive layout escaped the bundle root"
  );
  const extractRoot = path.join(workRoot, "extract");
  const installRoot = path.join(workRoot, "installed");
  mkdirSync(extractRoot, { recursive: true });
  mkdirSync(installRoot, { recursive: true });
  run("/usr/bin/tar", ["-xzf", stableArchive, "-C", extractRoot], {
    timeout: LINUX_TAR_RESOURCE_LIMITS.extractTimeoutMs,
  });
  const extractedBundle = path.join(extractRoot, "bundle");
  const installedBundle = path.join(installRoot, "client");
  assert(existsSync(extractedBundle), "Linux archive did not contain a bundle");
  renameSync(extractedBundle, installedBundle);

  const flutterClient = requiredFile(path.join(installedBundle, "licoup"),
    "installed Linux desktop executable");
  const nativeClient = requiredFile(path.join(installedBundle, "licoup-cli"),
    "installed Linux native sidecar");
  const bundleManifestPath = requiredFile(
    path.join(installedBundle, "package-metadata", "licoup", "packaging-modules.json"),
    "installed Linux bundle manifest"
  );
  const bundleManifest = JSON.parse(stableReadFile(bundleManifestPath, {
    maxBytes: 2 * 1024 * 1024,
  }).toString("utf8"));
  const bundleManifestDigest = sha256File(bundleManifestPath);
  const canonicalPackagingConfigDigest = stableSha256File(path.join(
    repoRoot,
    "apps/desktop/packaging.modules.json",
  ), { maxBytes: 2 * 1024 * 1024 });
  assert(bundleManifest.schemaVersion ===
    "v0.0.1:client-desktop:bundle-manifest-2" &&
    bundleManifest.platform === "linux" && bundleManifest.mode === "release",
    "Installed Linux bundle manifest target is invalid");
  assert(bundleManifest.architecture === "arm64", "Installed Linux bundle architecture is invalid");
  assert(bundleManifest.sourceStateDigest === options.expectedSourceDigest,
    "Installed Linux bundle source-state digest is stale");
  assert(bundleManifest.configPath === "apps/desktop/packaging.modules.json" &&
    bundleManifest.packagingConfigDigest === canonicalPackagingConfigDigest,
  "Installed Linux bundle packaging policy binding is invalid");
  assert(bundleManifest.productVersion === versionManifest.productVersion &&
    bundleManifest.buildNumber === versionManifest.buildNumber,
  "Installed Linux bundle version binding is invalid");
  assert(distribution.bundleManifestDigest === bundleManifestDigest,
    "Linux distribution bundle-manifest digest is invalid");
  assert(existsSync(path.join(installedBundle, "data", "flutter_assets")),
    "Installed Linux bundle is missing Flutter assets");
  assert(existsSync(path.join(installedBundle, "lib")),
    "Installed Linux bundle is missing runtime libraries");
  assertArm64Executable(flutterClient);
  assertArm64Executable(nativeClient);

  ctx.verificationPhase = "cli_smoke";
  const smokeState = path.join(workRoot, "smoke-state");
  mkdirSync(smokeState, { recursive: true });
  const targetScan = runJson(nativeClient, [
    "targets",
    "scan",
    "--include-accessible-environments",
    "false",
    "--include-history-model-catalog",
    "false",
  ], {
    ...process.env,
    LICOUP_HOME: smokeState
  });
  assertTargetScan(targetScan);
  const secureMeshStatus = runJson(nativeClient, ["secure-mesh", "status"], {
    ...process.env,
    LICOUP_HOME: smokeState
  });
  const capabilityReport = secureMeshStatus.capabilityReport;
  ctx.verificationPhase = "gui_session";
  const gui = await runGuiSession(ctx, flutterClient, installedBundle, smokeState);

  const sourceBinding = {
    sourceStateDigest: options.expectedSourceDigest,
    sourceStateDigestProvenance: String(
      bundleManifest.sourceStateDigestProvenance || distribution.sourceStateDigestProvenance || ""
    ),
    archiveDigest,
    bundleManifestDigest,
    nativeClientDigest: stableSha256File(nativeClient, { maxBytes: 512 * 1024 * 1024 }),
    stale: false
  };
  return {
    schema: linuxVmPackageReceiptSchema,
    schemaVersion: linuxVmPackageReceiptSchemaVersion,
    ok: true,
    producer: "linux-vm-package-receipt",
    generatedAt: new Date().toISOString(),
    closureChallengeDigest: releaseContext.closureChallengeDigest,
    invocationNonceDigest: releaseContext.invocationNonceDigest,
    productVersion: versionManifest.productVersion,
    buildNumber: versionManifest.buildNumber,
    artifactKind: "linux-vm-installed-client",
    target: "ubuntu-linux-arm64",
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    sourceBinding,
    package: {
      format: "tar.gz",
      layoutClasses: [
        "desktop_executable",
        "native_sidecar",
        "flutter_assets",
        "package_metadata"
      ],
      executableCount: 2,
      signaturePresent: true,
      validationSignature: true,
      signatureVerified: true,
      archiveDigestVerified: true,
      bundleManifestDigestVerified: true,
      installedFromArchive: true
    },
    session: {
      kind: "x11_virtual_display",
      clientStarted: gui.clientStarted,
      visibleWindow: gui.visibleWindow,
      interactionSmoke: gui.interactionSmoke,
      boundedShutdown: gui.boundedShutdown
    },
    smoke: {
      cliTargetScan: true,
      guiSession: true,
      exactCapabilitySchema: true
    },
    capabilityReport,
    privacy: linuxEvidencePrivacyRecord(),
    nonBlockingDistributionGuidance: {
      blocking: false,
      storeListingStatus: "not-configured",
      platformSigningStatus: "not-configured",
      publicDownloadStatus: "not-configured",
      updateChannelStatus: "not-configured",
      rollbackChannelStatus: "not-configured",
    },
    summary: {
      currentSourceArchive: true,
      installReceiptReady: true,
      sessionLaunchReady: true,
      smokeReady: true,
      privacyReady: true
    }
  };
}
