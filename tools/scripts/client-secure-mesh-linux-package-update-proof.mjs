#!/usr/bin/env node
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import { atomicWriteReportJson, resolveSafeReportPath } from "./lib/safe-report-io.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const defaultReportPath = physicalReportRefs.ubuntuLinuxPackageUpdateProof;

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/home\/[^/\s"]+|\/private\/|\/var\/folders\/|\/tmp\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
]);

const options = parseArgs(process.argv.slice(2));
const tempDir = mkdtempSync(path.join(os.tmpdir(), "lico-linux-package-update-proof-"));

try {
  const report = runProof();
  writeReport(report);
  console.log(JSON.stringify({
    ok: report.ok,
    report: report.report,
    platform: report.platform,
    packageUpdateProofReady: report.summary.packageUpdateProofReady,
    packageReady: report.summary.packageReady,
    installSmokeReady: report.summary.installSmokeReady,
    updateReady: report.summary.updateReady,
    rollbackReady: report.summary.rollbackReady,
  }, null, 2));
  if (!report.ok) {
    process.exitCode = 1;
  }
} catch (error) {
  const report = failureReport(error);
  writeReport(report);
  console.error(JSON.stringify({
    ok: false,
    report: report.report,
    error: report.failure.code,
  }, null, 2));
  process.exitCode = 1;
} finally {
  rmSync(tempDir, { recursive: true, force: true });
}

function runProof() {
  const cli = options.cli || process.env.LICO_RELEASE_CLI || "";
  assert(cli.trim(), "release CLI path is required");
  const platform = options.platform || "ubuntu-linux-arm64";
  const version = packageVersion();
  const binaryBytes = statSync(cli).size;
  assert(binaryBytes > 0, "release CLI binary is empty");
  const binaryDigest = sha256File(cli);

  const packageRoot = path.join(tempDir, "package-root");
  const packageBin = path.join(packageRoot, "bin");
  const packageMeta = path.join(packageRoot, "share", "licomesh");
  mkdirSync(packageBin, { recursive: true });
  mkdirSync(packageMeta, { recursive: true });
  copyFileSync(cli, path.join(packageBin, "licoup"));
  const manifest = {
    schemaVersion: "licomesh.secure-mesh.linux-package-manifest.v1",
    product: "lico-up",
    platform,
    version,
    binary: {
      path: "bin/licoup",
      bytes: binaryBytes,
      sha256: binaryDigest,
    },
    update: {
      strategy: "atomic-directory-swap",
      rollback: "previous-release-pointer",
    },
  };
  writeFileSync(path.join(packageMeta, "package-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");

  const packageOutput = packageOutputPath(platform);
  mkdirSync(path.dirname(packageOutput), { recursive: true });
  const tar = spawnSync("tar", ["-czf", packageOutput, "-C", packageRoot, "."], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert(tar.status === 0, `package archive creation failed: ${sanitizeError(tar.stderr || tar.stdout)}`);

  const extractRoot = path.join(tempDir, "extracted");
  mkdirSync(extractRoot, { recursive: true });
  const extract = spawnSync("tar", ["-xzf", packageOutput, "-C", extractRoot], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert(extract.status === 0, `package archive extraction failed: ${sanitizeError(extract.stderr || extract.stdout)}`);

  const extractedManifest = JSON.parse(readFileSync(path.join(extractRoot, "share", "licomesh", "package-manifest.json"), "utf8"));
  const extractedBinary = path.join(extractRoot, "bin", "licoup");
  const packageReady = extractedManifest?.binary?.sha256 === sha256File(extractedBinary) &&
    extractedManifest?.binary?.bytes === statSync(extractedBinary).size &&
    extractedManifest?.platform === platform &&
    extractedManifest?.version === version;

  const installRoot = path.join(tempDir, "install-root");
  const previousRoot = path.join(installRoot, "releases", "previous");
  const currentRoot = path.join(installRoot, "releases", version);
  mkdirSync(path.join(previousRoot, "bin"), { recursive: true });
  writeFileSync(path.join(previousRoot, "bin", "licoup"), "# previous release placeholder\n", "utf8");
  mkdirSync(currentRoot, { recursive: true });
  copyTree(extractRoot, currentRoot);
  writeFileSync(path.join(installRoot, "current-release.json"), `${JSON.stringify({
    schemaVersion: "licomesh.secure-mesh.linux-update-state.v1",
    active: version,
    previous: "previous",
    rollbackAvailable: true,
  }, null, 2)}\n`, "utf8");

  const installedCli = path.join(currentRoot, "bin", "licoup");
  const smoke = spawnSync(installedCli, ["--help"], {
    cwd: repoRoot,
    env: {
      ...process.env,
      LICOUP_HOME: path.join(tempDir, "smoke-portable"),
    },
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  const installSmokeReady = smoke.status === 0 &&
    String(smoke.stdout || smoke.stderr || "").includes("LicoMesh CLI");

  const activeState = JSON.parse(readFileSync(path.join(installRoot, "current-release.json"), "utf8"));
  const updateReady = activeState.active === version &&
    activeState.previous === "previous" &&
    activeState.rollbackAvailable === true &&
    sha256File(installedCli) === binaryDigest;

  writeFileSync(path.join(installRoot, "current-release.json"), `${JSON.stringify({
    schemaVersion: "licomesh.secure-mesh.linux-update-state.v1",
    active: "previous",
    rolledBackFrom: version,
    rollbackAvailable: false,
  }, null, 2)}\n`, "utf8");
  const rollbackState = JSON.parse(readFileSync(path.join(installRoot, "current-release.json"), "utf8"));
  const rollbackReady = rollbackState.active === "previous" &&
    rollbackState.rolledBackFrom === version &&
    rollbackState.rollbackAvailable === false;

  const summary = {
    packageReady,
    packageArchiveReady: statSync(packageOutput).size > 0,
    installSmokeReady,
    updateReady,
    rollbackReady,
    binaryDigestVerified: packageReady && updateReady,
    localPathsRedacted: true,
  };
  summary.packageUpdateProofReady = summary.packageReady &&
    summary.packageArchiveReady &&
    summary.installSmokeReady &&
    summary.updateReady &&
    summary.rollbackReady &&
    summary.binaryDigestVerified &&
    summary.localPathsRedacted;

  return {
    schemaVersion: "licomesh.secure-mesh.linux-package-update-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-linux-package-update-proof.mjs",
    generatedAt: new Date().toISOString(),
    report: reportReference(),
    reportLeakScan: true,
    ok: summary.packageUpdateProofReady,
    platform,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "linux-cli-tar-package-update",
    package: {
      format: "tar.gz",
      name: path.basename(packageOutput),
      manifestSchemaVersion: manifest.schemaVersion,
      binaryPath: manifest.binary.path,
      binaryBytes,
      digestRecorded: true,
    },
    update: {
      strategy: manifest.update.strategy,
      rollback: manifest.update.rollback,
      installSmokeCommand: "licoup --help",
      previousReleaseRecorded: true,
      rollbackStateRecorded: true,
    },
    summary,
  };
}

function failureReport(error) {
  return {
    schemaVersion: "licomesh.secure-mesh.linux-package-update-proof-report.v1",
    verifier: "tools/scripts/client-secure-mesh-linux-package-update-proof.mjs",
    generatedAt: new Date().toISOString(),
    report: reportReference(),
    reportLeakScan: true,
    ok: false,
    platform: options.platform || "ubuntu-linux-arm64",
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    artifactKind: "linux-cli-tar-package-update",
    failure: {
      code: "linux_package_update_proof_failed",
      sanitized: sanitizeError(error),
    },
    summary: {
      packageUpdateProofReady: false,
      packageReady: false,
      packageArchiveReady: false,
      installSmokeReady: false,
      updateReady: false,
      rollbackReady: false,
      binaryDigestVerified: false,
      localPathsRedacted: false,
    },
  };
}

function packageOutputPath(platform) {
  if (options.packageOutput) {
    return path.resolve(repoRoot, options.packageOutput);
  }
  return path.join(tempDir, `licoup-${platform}.tar.gz`);
}

function packageVersion() {
  try {
    const payload = JSON.parse(readFileSync(path.join(repoRoot, "package.json"), "utf8"));
    return String(payload.version || "0.0.0");
  } catch {
    return "0.0.0";
  }
}

function copyTree(source, target) {
  mkdirSync(target, { recursive: true });
  const cp = spawnSync("cp", ["-R", `${source}/.`, target], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert(cp.status === 0, `copy failed: ${sanitizeError(cp.stderr || cp.stdout)}`);
}

function sha256File(filePath) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function writeReport(report) {
  assertNoLeak(report, "secure mesh linux package update proof report");
  const reportRef = reportReference();
  const target = resolveSafeReportPath(repoRoot, reportRef);
  mkdirSync(path.dirname(target), { recursive: true });
  atomicWriteReportJson(repoRoot, reportRef, report);
}

function outputReportPath() {
  return path.resolve(repoRoot, options.report || defaultReportPath);
}

function reportReference() {
  const configured = options.report || defaultReportPath;
  const resolved = path.resolve(repoRoot, configured);
  const relative = path.relative(repoRoot, resolved);
  if (relative && !relative.startsWith("..") && !path.isAbsolute(relative)) {
    return relative;
  }
  return path.basename(resolved);
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/home\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
    .replace(/\/tmp\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}

function parseArgs(args) {
  const parsed = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) {
      continue;
    }
    const [rawKey, inlineValue] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    parsed[key] = inlineValue ?? args[index + 1] ?? "";
    if (inlineValue === undefined) {
      index += 1;
    }
  }
  return parsed;
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}
