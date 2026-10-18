const freezeLane = (scripts) => Object.freeze([...scripts]);

// The Dart/Flutter packages that ship outside the desktop app. Their
// pubspec files are dependency inputs, and their sources belong to the
// Flutter/Dart lane even though they are not under apps/desktop.
export const PRESENTATION_DART_PACKAGES = Object.freeze([
  "packages/presentation_contract",
  "packages/presentation_runtime",
  "packages/presentation_flutter",
]);

export const CLIENT_GATE_SCHEMA_VERSION = "licomesh.client-gate-policy.v1";

export const CLIENT_GATE_LANES = Object.freeze({
  source: freezeLane([
    "client:gate:topology",
    "client:gate:self-test",
    "repo:client-boundary",
    "repo:identity:self-test",
    "repo:local-info-hygiene:self-test",
    "repo:local-info-hygiene",
    "repo:workspace-cache-boundary",
    "client:artifacts:self-test",
    "client:verify:build-entry:self-test",
    "client:version:check",
    "client:verify:plan",
    "client:verify:architecture",
    "client:verify:local-data-egress-boundary",
    "client:verify:agent-conversation-parity",
    "client:verify:agent-adapter-standard",
    "client:verify:agent-native-output",
    "client:verify:agent-conversations:product-e2e:self-test",
    "client:verify:agent-usage",
    "client:contracts:test",
    "client:verify:update-release",
    "client:verify:windows-file-security",
    "repo:docs",
  ]),
  flutter: freezeLane([
    "client:get",
    "client:format:check",
    "client:analyze",
    "client:test",
    "client:packages:verify",
  ]),
  rust: freezeLane([
    "client:native:fmt:check",
    "client:native:clippy",
    "client:native:test:helpers",
    "client:native:test",
    "client:native:smoke",
  ]),
  android: freezeLane([
    "client:get",
    "client:test:android:native",
  ]),
  dependencies: freezeLane([
    "client:deps:audit",
  ]),
  "release-policy": freezeLane([
    "client:promotion:self-test",
    "client:pricing:check",
    "client:verify:release-artifact-io:self-test",
    "client:verify:release-dependency-receipts:self-test",
    "client:verify:source-state-digest:self-test",
    "client:verify:linux-tar-resource-bounds:self-test",
    "client:verify:bounded-child-process:self-test",
    "client:verify:android-apk-zip-facts:self-test",
    "client:verify:consumer-verification-manifest:self-test",
    "client:verify:update-manifest:self-test",
    "client:release:packages:self-test",
    "client:verify:package-index:self-test",
    "client:verify:review-signoff:self-test",
    "client:verify:release-target-evidence:self-test",
    "client:verify:release-report-schema:self-test",
    "client:verify:macos-nested-code-bounds:self-test",
    "client:verify:package-client:self-test",
    "client:native:smoke:policy:self-test",
    "client:verify:closure-producer-writer:self-test",
    "client:verify:android-physical-install-launch:self-test",
    "client:verify:secure-mesh-macos-capabilities:self-test",
    "client:verify:secure-mesh-linux-node-matrix:self-test",
    "client:cli:vm:self-test",
    "client:state:migration:self-test",
    "client:verify:artifact-verification-receipts:self-test",
    "client:verify:secure-mesh-capability-model:self-test",
    "client:verify:secure-mesh-trust-ux:self-test",
    "client:verify:secure-mesh-report-redaction:self-test",
    "client:verify:secure-mesh-e2ee-evidence:contract-binding",
    "client:verify:secure-mesh-e2ee-evidence:authority-proof-self-test",
    "client:verify:secure-mesh-e2ee-evidence:readiness-self-test",
    "client:verify:secure-mesh-e2ee-evidence:leak-scan-self-test",
    "client:verify:client-release-acceptance:self-test",
    "client:demo:device:self-test",
  ]),
});

export const CLIENT_RELEASE_TARGETS = Object.freeze({
  "macos-direct-arm64": Object.freeze({
    publicationBlocked: false,
    localOnly: true,
    artifactName: "licoup-macos-direct-arm64",
    installerArtifact: "LicoUp-macos-arm64.dmg",
    updateArtifact: "LicoUp-macos-arm64-update.zip",
    files: Object.freeze([
      "LicoUp-macos-arm64.dmg",
      "LicoUp-macos-arm64.dmg.sha256",
      "LicoUp-macos-arm64-update.zip",
      "LicoUp-macos-arm64-update.zip.sha256",
      "LicoUp-macos-arm64.build.json",
      "LicoUp-macos-direct-arm64.package.json",
    ]),
  }),
});

export const CLIENT_CI_JOBS = Object.freeze([
  "plan",
  "engineering",
  "target-linux",
  "target-darwin",
  "target-win32",
  "client-required",
]);

const DEPENDENCY_PATHS = new Set([
  "package.json",
  "package-lock.json",
  "Cargo.toml",
  "Cargo.lock",
  "apps/desktop/pubspec.yaml",
  "apps/desktop/pubspec.lock",
  ...PRESENTATION_DART_PACKAGES.flatMap((directory) => [
    `${directory}/pubspec.yaml`,
    `${directory}/pubspec.lock`,
  ]),
]);

function normalizePath(value) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("changed path must be a non-empty repository-relative path");
  }
  const portable = value.replaceAll("\\", "/").replace(/^\.\//u, "");
  if (
    portable.startsWith("/") ||
    portable.startsWith("../") ||
    portable.includes("/../") ||
    /^[A-Za-z]:\//u.test(portable) ||
    /[\0\r\n]/u.test(portable)
  ) {
    throw new Error("changed path must stay inside the repository");
  }
  return portable;
}

function isFlutterPath(file) {
  if (
    PRESENTATION_DART_PACKAGES.some((directory) => file.startsWith(`${directory}/`))
  ) {
    return true;
  }
  return (
    file.startsWith("apps/desktop/lib/") ||
    file.startsWith("apps/desktop/test/") ||
    file.startsWith("apps/desktop/integration_test/") ||
    file.startsWith("apps/desktop/assets/") ||
    file.startsWith("apps/desktop/shaders/") ||
    file === "apps/desktop/analysis_options.yaml" ||
    file === "apps/desktop/pubspec.yaml" ||
    file === "apps/desktop/pubspec.lock"
  );
}

function isRustPath(file) {
  return (
    file === "Cargo.toml" ||
    file === "Cargo.lock" ||
    file === "rust-toolchain.toml" ||
    file.startsWith("crates/")
  );
}

function isAndroidPath(file) {
  return (
    file.startsWith("apps/desktop/android/") ||
    file.startsWith("tools/android-") ||
    file.includes("/android-") ||
    file.includes("/android_")
  );
}

function isReleasePolicyPath(file) {
  return (
    file.startsWith("docs/releases/") ||
    file.startsWith("tools/apple-release/") ||
    file.startsWith("tools/release/") ||
    file.startsWith("tools/scripts/macos-release/") ||
    file.startsWith("apps/desktop/scripts/package-client") ||
    file.startsWith(".github/workflows/client-release") ||
    file === ".github/workflows/client-source-release.yml" ||
    file === ".github/workflows/client-weekly-release.yml" ||
    /^tools\/scripts\/client-(?:promotion|release|source-release|weekly-release|update-manifest)/u.test(file)
  );
}

export function classifyClientGatePaths(changedPaths) {
  if (!Array.isArray(changedPaths)) {
    throw new Error("changed paths must be an array");
  }
  const normalized = [...new Set(changedPaths.map(normalizePath))].sort();
  const lanes = {
    source: true,
    flutter: false,
    rust: false,
    android: false,
    dependencies: false,
    "release-policy": false,
  };

  for (const file of normalized) {
    if (isFlutterPath(file)) lanes.flutter = true;
    if (isRustPath(file)) lanes.rust = true;
    if (isAndroidPath(file)) lanes.android = true;
    if (DEPENDENCY_PATHS.has(file)) lanes.dependencies = true;
    if (isReleasePolicyPath(file)) lanes["release-policy"] = true;
  }

  return Object.freeze({
    changedCount: normalized.length,
    lanes: Object.freeze(lanes),
  });
}
