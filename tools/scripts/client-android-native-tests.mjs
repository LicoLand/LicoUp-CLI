#!/usr/bin/env node
import { existsSync, realpathSync } from "node:fs";
import { execFileSync, spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { runCargoTestFilter } from "./lib/cargo-test-filter-runner.mjs";
import { acquireTestArtifactLease } from "./lib/test-artifact-lifecycle.mjs";
import {
  atomicWriteReportJson,
  removeContainedReportIfExists
} from "./lib/safe-report-io.mjs";

const workspaceRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const androidRoot = path.join(workspaceRoot, "apps", "desktop", "android");
const reportRef = "build/reports/secure-mesh-android-platform-crypto-acceptance.json";
const verifier = "tools/scripts/client-android-native-tests.mjs";
const testClasses = [
  "land.lico.licoup.ReleaseAcceptanceChannelTest",
  "land.lico.licoup.ReleaseAcceptanceIngressTest",
  "land.lico.licoup.ReleaseClosureBindingTest",
  "land.lico.licoup.SecureMeshAndroidAdaptiveCustodyTest",
  "land.lico.licoup.SecureMeshAndroidAuthorizationPolicyTest"
];
const rustFfiTestFilters = Object.freeze([
  "mobile_ffi_native_action_contract_is_shared_by_platform_bridges",
  "mobile_ffi_unsupported_action_uses_calling_platform_error_code"
]);
const artifactTargets = Object.freeze([
  "apps/desktop/build",
  "build/crates/licoup-native/android-target"
]);

function javaExecutable(javaHome) {
  return path.join(javaHome, "bin", process.platform === "win32" ? "java.exe" : "java");
}

function validJavaHome(javaHome) {
  if (!javaHome || !existsSync(javaExecutable(javaHome))) {
    return false;
  }
  const probe = spawnSync(javaExecutable(javaHome), ["-version"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout: 5000
  });
  return probe.status === 0;
}

function javaHomeFromPath() {
  try {
    const executable = execFileSync(
      process.platform === "win32" ? "where" : "which",
      ["java"],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }
    ).trim().split(/\r?\n/u)[0];
    if (!executable) return "";
    return path.dirname(path.dirname(realpathSync(executable)));
  } catch {
    return "";
  }
}

function javaHomeFromFlutterDoctor() {
  try {
    const output = execFileSync("flutter", ["doctor", "-v"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"]
    });
    const match = output.match(/Java binary at:\s*(.+?)(?:\r?\n|$)/u);
    if (!match) return "";
    return path.dirname(path.dirname(match[1].trim()));
  } catch {
    return "";
  }
}

function resolveJavaHome() {
  const candidates = [
    process.env.JAVA_HOME || "",
    javaHomeFromPath(),
    javaHomeFromFlutterDoctor(),
    ...(process.platform === "darwin" ? [
      "/Applications/Android Studio.app/Contents/jbr/Contents/Home",
      "/Applications/Android Studio Preview.app/Contents/jbr/Contents/Home"
    ] : [])
  ];
  const selected = candidates.find(validJavaHome);
  if (!selected) {
    throw new Error("A Java runtime compatible with the Android toolchain is required.");
  }
  return selected;
}

function redactOutput(value) {
  const replacements = [
    [workspaceRoot, "<repo>"],
    [os.homedir(), "<home>"]
  ];
  return replacements.reduce(
    (text, [sensitive, replacement]) => text.split(sensitive).join(replacement),
    String(value || "")
  ).slice(-12000);
}

const artifactLeases = [];
try {
  for (const targetPath of artifactTargets) {
    artifactLeases.push(acquireTestArtifactLease({
      repoRoot: workspaceRoot,
      scope: "android-native-tests",
      targetPath
    }));
  }
  removeContainedReportIfExists(
    path.join(workspaceRoot, "build"),
    reportRef.replace(/^build\//u, "")
  );
  const env = {
    ...process.env,
    JAVA_HOME: resolveJavaHome()
  };
  const args = [
    "-q",
    "--offline",
    "--no-daemon",
    "--warning-mode",
    "none",
    "app:testDebugUnitTest",
    ...testClasses.flatMap((name) => ["--tests", name])
  ];
  const dependencyBootstrap = spawnSync(path.join(androidRoot, "gradlew"), [
    ...args.filter((arg) => arg !== "--offline")
  ], {
    cwd: androidRoot,
    env,
    encoding: "utf8",
    timeout: 10 * 60 * 1000,
    stdio: ["ignore", "pipe", "pipe"]
  });
  if (dependencyBootstrap.status !== 0) {
    const output = redactOutput(
      `${dependencyBootstrap.stdout || ""}\n${dependencyBootstrap.stderr || ""}`
    );
    throw new Error(`Android native dependency bootstrap failed.\n${output}`);
  }
  const result = spawnSync(path.join(androidRoot, "gradlew"), args, {
    cwd: androidRoot,
    env,
    encoding: "utf8",
    timeout: 10 * 60 * 1000,
    stdio: ["ignore", "pipe", "pipe"]
  });
  if (result.status !== 0) {
    const output = redactOutput(`${result.stdout || ""}\n${result.stderr || ""}`);
    throw new Error(`Android native tests failed.\n${output}`);
  }
  const rustFfiChecks = rustFfiTestFilters.map((filter) => runCargoTestFilter({
    repoRoot: workspaceRoot,
    manifestPath: "crates/licoup-native/Cargo.toml",
    filter,
    env,
    sanitizeError: redactOutput
  }));
  const failedRustFfiCheck = rustFfiChecks.find((check) => !check.ok);
  if (failedRustFfiCheck) {
    throw new Error(
      `Rust FFI contract test failed: ${failedRustFfiCheck.id}. ` +
      failedRustFfiCheck.failureSummary
    );
  }
  const checkedAt = new Date().toISOString();
  const report = {
    ok: true,
    schemaVersion: "licomesh.secure-mesh.android-platform-crypto-acceptance.v1",
    verifier,
    generatedBy: verifier,
    generatedAt: checkedAt,
    checkedAt,
    ...optionalReleaseInvocationBinding(),
    platform: "android",
    evidenceKind: "android-jvm-platform-custody-authorization-and-rust-ffi-contract",
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    summary: {
      ok: true,
      platformCryptoAcceptanceReady: true,
      platformCustodyContractReady: true,
      platformAuthorizationContractReady: true,
      rustFfiActionContractReady: true,
      mlsMemberRemoveReleaseActionReady: true,
      unknownReleaseActionsFailClosed: true,
      nativeTestClassCount: testClasses.length,
      rustFfiTestCount: rustFfiChecks.reduce(
        (total, check) => total + check.executedTestCount,
        0
      ),
      privatePathsIncluded: false
    }
  };
  atomicWriteReportJson(
    path.join(workspaceRoot, "build"),
    reportRef.replace(/^build\//u, ""),
    report
  );
  process.stdout.write(`${JSON.stringify({
    ok: true,
    suite: "android-platform-crypto-acceptance",
    report: reportRef,
    testClassCount: testClasses.length,
    rustFfiTestCount: rustFfiChecks.reduce(
      (total, check) => total + check.executedTestCount,
      0
    ),
    privatePathsIncluded: false
  })}\n`);
} catch (error) {
  process.stderr.write(`${redactOutput(error?.message || error)}\n`);
  process.exitCode = 1;
} finally {
  for (const lease of artifactLeases.reverse()) lease.release();
}
