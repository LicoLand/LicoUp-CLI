export const TEST_ARTIFACT_SCHEMA_VERSION = "licoup.test-artifact.v1";
export const NATIVE_CARGO_TEST_TARGET = "build/crates/licoup-native/target";
export const ANDROID_NATIVE_CARGO_TARGET = "build/crates/licoup-native/android-target";
export const FLUTTER_TEST_TARGET = "apps/desktop/build";
export const ROOT_CARGO_TARGET = "target";
export const REGISTRY_PATH = "build/.test-artifacts";
export const LOCK_WAIT_MS = 25;
export const DEAD_LEASE_GRACE_MS = 10 * 60_000;

export const FORBIDDEN_TARGET_SEGMENTS = new Set([
  ".cargo",
  ".gradle",
  ".pub-cache",
  "cargo-home",
  "gradle-user-home",
  "node_modules",
  "pub-cache",
  "registry",
  "sdk",
  "toolchain",
]);
