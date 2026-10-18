import {
  NATIVE_MANIFEST,
  command,
  defineModule,
  node,
} from "../helpers.mjs";

const CATALOG_CONVERGENCE_MANIFEST =
  "crates/lico-catalog-convergence/Cargo.toml";

const nativeLibraryCheck = () => command(
  "cargo",
  ["check", "--manifest-path", NATIVE_MANIFEST, "--lib"],
  10 * 60_000,
);

export const RUST_CATALOG_CONVERGENCE_MODULES = Object.freeze([
  defineModule({
    id: "rust.crate.catalog-convergence",
    kind: "rust-crate",
    summary: "Portable catalog revision, cache, cohort, invalidation, and privacy-safe receipt engine",
    inputs: [
      CATALOG_CONVERGENCE_MANIFEST,
      "crates/lico-catalog-convergence/src/lib.rs",
      "crates/lico-catalog-convergence/src/dispatch.rs",
      "crates/lico-catalog-convergence/src/engine.rs",
      "crates/lico-catalog-convergence/src/model.rs",
      "crates/lico-catalog-convergence/src/receipt.rs",
      "crates/lico-catalog-convergence/src/store.rs",
      "crates/lico-catalog-convergence/src/tests.rs",
    ],
    command: command(
      "cargo",
      ["test", "--manifest-path", CATALOG_CONVERGENCE_MANIFEST],
      10 * 60_000,
    ),
  }),
  defineModule({
    id: "rust.crate.migrate",
    kind: "rust-crate",
    summary: "Standalone migration binary and its released-root conversion, archive and cross-container suites",
    targetEvidenceHosts: ["darwin", "linux", "win32"],
    inputs: [
      "crates/licoup-migrate/**",
      "tools/scripts/migration-crate-tests.mjs",
      "tests/fixtures/client_state_migration/released_source.rs",
    ],
    // The runner constructs the planned candidate identity through the native build
    // script's real product-version input and builds the client CLI the oracle compares
    // against; see the script header for why neither of those is a fixture rewrite.
    command: node("tools/scripts/migration-crate-tests.mjs", [], 20 * 60_000),
  }),
  defineModule({
    id: "rust.domain.catalog-convergence-adapter",
    kind: "rust-domain",
    summary: "Native domain boundary re-exporting the portable catalog convergence contract",
    inputs: [
      "crates/licoup-native/src/domain/catalog_convergence.rs",
    ],
    command: nativeLibraryCheck(),
  }),
  defineModule({
    id: "rust.platform.catalog-cache-store",
    kind: "rust-platform",
    summary: "Private local catalog-cache path and portable store composition",
    inputs: [
      "crates/licoup-native/src/platform/catalog_cache_store.rs",
    ],
    command: nativeLibraryCheck(),
  }),
  defineModule({
    id: "rust.ffi.catalog-convergence",
    kind: "rust-ffi",
    summary: "Native catalog command registration and bounded domain dispatch bridge",
    inputs: [
      "crates/licoup-native/src/bin/licoup/stdio_rpc/server.rs",
    ],
    command: nativeLibraryCheck(),
  }),
]);
