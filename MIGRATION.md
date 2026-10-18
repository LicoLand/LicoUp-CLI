# LicoUp → LicoUp-CLI move manifest

The Rust world leaves the `LicoUp` monorepo (which becomes "LicoUp Desktop":
Flutter app + packaging + bridge contract) and lands in this repository,
preserving the existing relative layout so every `path = "…"` dependency and
declared `[[test]]`/`[[bin]]` path keeps resolving. All paths below are
relative to the monorepo root `/Users/unka/DevSpace/LicoLand/LicoUp`; they copy
to the same relative path in this repository.

> Status: landed. Sources copied from LicoUp `f9e509fd1` on
> `feature/base-extraction-gateways` (tree clean). `cargo check --workspace
> --locked`, `cargo check -p licoup-native --tests --locked`,
> `components/analytics cargo check --tests --locked` and `cargo fmt --all --
> --check` all pass. See §6 for the copy delta beyond the original manifest.

## 1. Moves — verbatim copies

### 1.1 Root files

| Source (LicoUp) | Dest (LicoUp-CLI) | Notes |
| --- | --- | --- |
| `Cargo.toml` | `Cargo.toml` | Already staged here; `workspace.package.repository` re-pointed to `LicoLand/LicoUp-CLI`. Re-copy only if the in-flight crate split changes `members`/`[workspace.dependencies]`. |
| `Cargo.lock` | `Cargo.lock` | Move with the workspace; member paths are unchanged so the lock stays valid. Regenerate check: `cargo metadata --locked`. |
| `rust-toolchain.toml` | `rust-toolchain.toml` | Already copied (channel `1.95.0`, components rust-analyzer/clippy/rustfmt). |
| `.cargo/config.toml` | `.cargo/config.toml` | Tracked. Carries `build.target-dir = "build/crates/licoup-native/target"` and `net.git-fetch-with-cli = true` (required for the private `licoarc` git dep — Cargo's built-in git client cannot use the credential helper). Copy verbatim. |
| `LICENSE` | `LICENSE` | Already copied (AGPL-3.0). |
| `NOTICE` | `NOTICE` | Already copied; preferred-source URL updated to `LicoUp-CLI`. |
| `.githooks/` (`commit-msg`, `pre-commit`, `pre-push`) | `.githooks/` | Local GitVault gates; the same checkers guard this repo. |

### 1.2 `crates/` — entire directory, all 41 crate trees

Every workspace member in `Cargo.toml` moves; there is no Flutter-owned crate.

```
crates/lico-catalog-convergence          crates/licoup-agent-antigravity
crates/licoup-agent-adapter-sdk          crates/licoup-agent-claude-code
crates/licoup-agent-adapters             crates/licoup-agent-codex
crates/licoup-agent-copilot              crates/licoup-agent-cursor
crates/licoup-agent-deepseek             crates/licoup-agent-drivers
crates/licoup-agent-hermes               crates/licoup-agent-kilo
crates/licoup-agent-kimi                 crates/licoup-agent-lico-agent
crates/licoup-agent-openclaw             crates/licoup-agent-opencode
crates/licoup-agent-pi                   crates/licoup-agent-runtime
crates/licoup-agent-targets              crates/licoup-application
crates/licoup-client-state               crates/licoup-collaboration
crates/licoup-conversation               crates/licoup-endpoint-core
crates/licoup-extension-contracts        crates/licoup-foundation
crates/licoup-migrate                    crates/licoup-mobile-core
crates/licoup-model-catalog              crates/licoup-native
crates/licoup-platform-bridges           crates/licoup-private-gateway
crates/licoup-private-gateway/entry      crates/licoup-project
crates/licoup-protocol-bindings          crates/licoup-public-gateway
crates/licoup-public-gateway/entry       crates/licoup-state-machine-codegen
crates/licoup-statistics                 crates/licoup-vault
crates/licoup-workflow                   crates/licoup-workflow-runtime
crates/licoup-workflow-store             crates/trybuild
```

Binary ownership after the move (declared in the member manifests):

- `crates/licoup-native/Cargo.toml` — `[[bin]] licoup-cli` (`src/bin/licoup.rs`),
  `lico-agent`, `lico-secure-mesh-kt-mock` (feature `secure-mesh-acceptance-mock-kt`);
  `[lib] crate-type = ["rlib", "cdylib", "staticlib"]` — the Desktop FFI library.
- `crates/licoup-private-gateway/entry/Cargo.toml` — `lico-gateway`, `lico-llm-gateway`.
- `crates/licoup-public-gateway/Cargo.toml` — `lico-subagent-mcp` (`src/main.rs`).
- `crates/licoup-public-gateway/entry/Cargo.toml` — `lico-telegram-channel`.
- `crates/licoup-migrate/Cargo.toml` — `licoup-migrate`.
- Every `crates/licoup-agent-*/Cargo.toml` — `lico-agent-<name>` helper programs.

Publishable family (manifests carry `version = "…"` + `description`, no
`publish = false`): `licoup-foundation`, `licoup-application`,
`licoup-client-state`, `licoup-extension-contracts`, `licoup-model-catalog`,
`licoup-vault`, `licoup-statistics`, `licoup-private-gateway`,
`licoup-public-gateway`, `licoup-migrate`, all `licoup-agent-*` adapter
packages, `licoup-usage-source-sdk`. Their inter-crate deps already use
`{ path = "…", version = "…" }` form, so `cargo publish` works once versions
are confirmed.

### 1.3 `sdk/` — entire directory

| Path | Notes |
| --- | --- |
| `sdk/usage-source/` | `licoup-usage-source-sdk`, standalone `[workspace]` + own `Cargo.lock`; referenced by `crates/licoup-native` (`licoup-usage-source-sdk = { path = "../../sdk/usage-source" }`) and `crates/licoup-statistics`, `components/*`. Root `exclude = ["sdk/usage-source"]` already mirrors its standalone status. |
| `sdk/agent-adapter/` | `python/` SDK + `samples/` — the agent-adapter SDK distribution. No Cargo member. |

### 1.4 `components/` — entire directory

`components/analytics`, `components/appearance`, `components/endpoint-collaboration` —
each a standalone `[workspace]` crate with its own `Cargo.lock`, path deps into
`../../crates/` and `../../sdk/usage-source`. They are Rust source and move with
this repo; their `../../` relative deps keep working because `crates/` and `sdk/`
move alongside with the same layout. `components/analytics/Cargo.toml` declares
`[[test]] usage_sources` at `../../tests/integration/usage_sources/main.rs` —
covered by 1.5.

### 1.5 `tests/` — the Rust-bound suites

| Source | Dest | Why |
| --- | --- | --- |
| `tests/integration/extension_isolation/` | same | Declared `[[test]]` of `licoup-native` (`path = "../../tests/integration/extension_isolation/main.rs"`) — required for `cargo test` to resolve. |
| `tests/integration/usage_sources/` | same | Declared `[[test]]` of `components/analytics` — required. |
| `tests/integration/package_lifecycle/` | same | `verify_component_lifecycle.py` exercises the `components/*` package lifecycle — component-side. |
| `tests/smoke/` (`native-client-smoke.mjs`, `native-client-smoke-policy-self-test.mjs`, `windows-native-smoke.mjs`) | same | Drives the built `licoup-cli` binary. |
| `tests/product-e2e/cli/` | same | CLI product-e2e (`agent-conversations/`, `client-update/`, `continuous-assistant/`, `subagent-mcp/`). |
| `tests/fixtures/client_state_migration/` | same | Feeds `tools/scripts/client-state-migration*` probes of the native SQLite state. |
| `tests/fixtures/continuous-assistant/` | same | Assistant/workflow fixtures consumed by the Rust-side contract tests. |
| `tests/contract/client/` — the Rust-source-bound subset | same | Tests asserting on crate sources, wire contracts and agent adapters move; the shared reader `tools/scripts/lib/source-check-bundle.mjs` moves with them. See §3.4 for the classification rule. |

### 1.6 `tools/` — the Rust-facing tooling

Move:

- `tools/scripts/cargo-client.mjs` + `tools/scripts/lib/test-artifact-lifecycle.mjs`,
  `tools/scripts/lib/test-artifact-lifecycle.test.mjs`,
  `tools/scripts/lib/test-artifact-lifecycle/constants.mjs` — the cargo runner
  and its target-dir lease (⚠ needs the §3.1 rewire).
- `tools/scripts/client-deps-audit.mjs`, `client-rustsec-exceptions.mjs` — cargo
  audit gate.
- `tools/scripts/client-native-performance.mjs` — native perf harness.
- `tools/scripts/protocol-codegen/` — `generate-conversation-protocol.mjs`,
  codegen over `schemas/conversation_protocol/` into Rust.
- `tools/scripts/client-agent-adapter-standard.mjs`,
  `client-agent-native-output.mjs`, `client-agent-conversation-verify.mjs`,
  `client-agent-conversation-product-e2e.mjs`, `client-acp-conversation-parity.mjs`,
  `client-upstream-agent-probe.mjs`, `opencode-add-licoup-gateway.mjs`,
  `pi-add-licoup-gateway.mjs` — agent-adapter lane.
- `tools/acp-reference-agent/` — the ACP reference agent used by the adapter
  self-tests (`agent.mjs`, `verify.mjs`).
- `tools/scripts/client-secure-mesh-*` — the secure-mesh proof/evidence lane
  (native protocol; ~20 scripts incl. `…/native-test.mjs` sub-tools that run
  cargo).
- `tools/scripts/client-state-migration*` (`client-state-migration.mjs` +
  `client-state-migration/` probe/sqlite-contract/frontier) — probes the native
  state store.
- `tools/scripts/client-android-native-tests.mjs` — drives the Rust Android
  native tests.
- `tools/scripts/model-pricing-facts.mjs` — checks `licoup-model-catalog` facts.
- `tools/scripts/transcript-record/` — agent transcript record/replay tooling.
- `tools/architecture-graph/` — architecture graph generator over crate/module
  sources.
- `tools/regression/` — the rust lane only: `client-module-catalog/groups/rust-*.mjs`
  and the `client-module-regression-tests/` entries that select/attribute cargo
  tests. The Flutter and packaging-lane groups stay in Desktop.
- `tools/scripts/lib/` — shared libraries used by moved scripts
  (`source-check-bundle.mjs`, `sanitize-error.mjs`,
  `secure-mesh-capability-report.mjs`, `privacy-test-fixtures.mjs`, etc.).

### 1.7 `schemas/` — the Rust-side schemas

| Schema | Moves? | Consumer |
| --- | --- | --- |
| `schemas/conversation_protocol/` | yes | `tools/scripts/protocol-codegen/generate-conversation-protocol.mjs` generates Rust (`licoup-conversation`). |
| `schemas/extensions/` | yes | `licoup-extension-contracts` contract source. |
| `schemas/subagent_mcp/` | yes | `licoup-public-gateway` `lico-subagent-mcp` service. |
| `schemas/client_bridge/` | **vendored copy here too** | Contract authority stays Desktop (§3.2), but moved Rust sources embed/read the files at build/test time — `licoup-platform-bridges/src/abi.rs` `include_str!`s `client_runtime_abi.json`; `licoup-native` tests `include_str!`/`read_to_string` `secure_mesh.json`, `manifest.json`, `package.json`, `model_selection.json`. All 9 files copied as a read-only-in-spirit vendored set until §5.2 lands a pinned-revision drift check. See §6 delta D1. |

### 1.8 CI / workflow sources

- `.github/workflows/` — the cargo lanes of `client-ci.yml` and
  `client-regression.yml` are re-created here as new CLI workflows (see §3.5);
  nothing copies verbatim because the monorepo workflows interleave Flutter and
  Rust lanes.
- `AGENTS.md` — this repo needs its own agent guide derived from the monorepo's;
  write it when sources land (open question §5).

## 2. Stays — owned by LicoUp Desktop

| Path | Why it stays |
| --- | --- |
| `apps/desktop/` | The Flutter application: `lib/`, `test/`, platform dirs (`android/`, `ios/`, `linux/`, `macos/`, `windows/`), `packaging/`, `packaging.modules.json`, `scripts/`, `pubspec.*`. |
| `packages/` | Dart packages: `presentation_contract`, `presentation_flutter`, `presentation_runtime`, plus `contracts/client/` JSON schemas + fixtures and `protocols/native-client/` docs — all client-surface contracts. |
| `schemas/client_bridge/` | The bridge contract schema (decision: Desktop owns the contract). |
| `tools/scripts/` Desktop lane | `client-build.mjs`, `client-gate*.mjs`, `client-release-*.mjs`, `client-macos-*.mjs`, `client-mobile-simulator-closure*`, `client-device-demo.mjs`, `client-version.mjs`, `client-support-matrix.mjs`, `client-promotion.mjs`, `client-packages-verify.mjs`, `client-ui-state-machine.mjs`, `generate-client-bridge-contracts.mjs` (see §3.2), release/VM/install tooling. |
| `tools/apple-release/`, `tools/distribution/`, `tools/release/`, `tools/templates/`, `tools/development/` | Release authority, packaging distribution, report/plan rendering — product-side. |
| `tools/data-migration/` | Node data-migration CLI/engine — open question §5 (recommended: Desktop product tooling; the Rust `licoup-migrate` crate already moved). |
| `tools/client-*.json`, `tools/client-version.json` | Release/package target manifests and the product-version authority — product-side (see §3.1 for the version-env fixup). |
| `tests/contract/client/` Flutter/release-bound subset | Packaging, release, version, layout, golden and plan tests stay; see §3.4. |
| `tests/fixtures/client_package_release/` | Packaging fixtures. |
| `tests/verify-client-update-release-channel.mjs`, `tests/verify-windows-file-security-boundary.mjs` | Release-channel / Windows packaging boundary verifiers. |
| `docs/` (root guides) | `RUNBOOK`, `CLOSURE`, `COMPATIBILITY`, `STATUS`, `RELEASE-PACKAGES`, `adrs/`, `plans/`, `reports/`, `platforms/`, `releases/`, `functionality/`, `modules/` — repo workflow and product docs stay; Rust-relevant pages are cited selectively into `docs/` here (open question §5). |
| `brand/`, `PRODUCT*.md`, `CONTEXT.md`, `CONTRIBUTING*.md`, `CODE_OF_CONDUCT.md`, `SECURITY*.md`, `CHANGELOG.md`, `README*.md`, `Height`, `readme-preview.html`, `design-qa.md`, `package.json`, `package-lock.json`, `node_modules/` | Product and repo-governance surface of the desktop client. |
| `apps/desktop` Flutter goldens/fixtures under `apps/desktop/test/` | Golden-image baselines and layout manifests. |

## 3. Post-move fixups

### 3.1 `tools/scripts/cargo-client.mjs` — sever the `apps/` import

Line 7 imports `clientProductVersion` from
`apps/desktop/scripts/package-client/build/native.mjs`, which in turn reads the
product-version authority `tools/client-version.json`. Neither path exists here.

**Applied:** the `apps/` import is gone. `cargo-client.mjs` now reads
`tools/cli-version.json` (new, CLI-owned, same manifest shape; seeded with
`productVersion 0.3.0`, matching `workspace.package.version`). The
`LICO_CLIENT_PRODUCT_VERSION` env-var contract is unchanged —
`crates/licoup-native/build.rs` consumes it. The remaining decision for the
maintainer: is the embedded "product version" of `licoup-cli` the crate version
or still the Desktop product version? **Decision needed (§5).**

### 3.2 Bridge contract codegen — `schemas/client_bridge` ↔ `crates/licoup-native/src/ffi/generated/`

`tools/scripts/generate-client-bridge-contracts.mjs` reads
`schemas/client_bridge/manifest.json` + `schemas/client_bridge/*.json` and
emits into BOTH trees: `crates/licoup-native/src/ffi/generated/` (Rust) and
`apps/desktop/lib/src/contracts/generated/` (Dart). The schema stays in
Desktop; the Rust output must land here.

Options: (a) generator stays in Desktop and writes the Rust half into a checked-out
LicoUp-CLI working tree (cross-repo codegen step in the sync workflow); (b)
schema + generator move here and Desktop consumes the generated Dart (contradicts
"Desktop owns the contract"); (c) split the generator: schema stays in Desktop,
a copy of the generated `.rs` is committed here and a CLI-side check
(`--check` mode already exists) verifies drift against the Desktop schema at a
pinned revision. **Recommendation: (c) — deterministic, no live cross-repo writes.
Decision needed (§5).**

### 3.3 Desktop packaging native-build seam — `apps/desktop/scripts/package-client/build/native.mjs`

`SIDECAR_CRATES` (lines 17–34) builds `lico-gateway`, `lico-llm-gateway`,
`lico-subagent-mcp`, `lico-telegram-channel` and all `licoup-native` bins by
invoking `tools/scripts/cargo-client.mjs build --manifest-path crates/…` —
none of those manifests exist in Desktop after the split.

Desktop must instead: (a) fetch prebuilt binaries from LicoUp-CLI release
artifacts at a pinned version, or (b) materialize a pinned LicoUp-CLI source
checkout and run `cargo build` there. `LICO_CLIENT_PRODUCT_VERSION` /
`LICO_CLIENT_RELEASE_TRACK` remain Desktop-set env inputs either way.
**Decision needed (§5).** The same seam applies to `tools/scripts/cargo-client.mjs`
consumers that remain in Desktop (`client:native:*` npm scripts — see §3.6).

### 3.4 `tests/contract/client/` split rule

Classification rule: a test moves iff the artifact under test is a moved
source. `*-source-bundle.test.mjs` files name their target sources; grep each
bundle for `crates/`, `sdk/`, `components/`, `tests/integration`, `schemas/conversation_protocol`,
`schemas/extensions`, `schemas/subagent_mcp` → move; `apps/`, `packages/`,
`schemas/client_bridge`, `tools/{apple-release,distribution,release,templates,development}`,
packaging/release scripts → stay. Representative examples:

- Move: `antigravity-adapter-package-source-bundle`, `claude-code-driver-source-bundle`,
  `codex-app-server-source-bundle`, `deepseek-harness-source-bundle`,
  `kimi-code-acp-source-bundle`, `hermes-acp-source-bundle`, `kilo-code-serve-source-bundle`,
  `conversation-semantic-source-bundle`, `collaboration-workflow-operations-source-bundle`,
  `assistant-workflow-contract`, `endpoint-*` tests, `lico-subagent-mcp`,
  `cursor-subagent-mcp`, `antigravity-subagent-mcp`, `internal-capability-surface`.
- Stay: `layout-*`, `client-version`, `client-release-*`, `client-macos-install`,
  `apple-release-integration`, `client-promotion`, `client-weekly-release`,
  `cli-command-admission-source-bundle` (CLI VM tooling is product-side).
- Shared infra `tools/scripts/lib/source-check-bundle.mjs` + `tools/scripts/lib/privacy-test-fixtures.mjs`
  move to CLI (bundles assert on moved sources); Desktop keeps or duplicates
  the copies its staying tests need — flag any residual import errors during
  the Desktop slim-down.

### 3.5 `.github/workflows/`

`client-ci.yml` interleaves: Flutter lanes (stay) + cargo lanes (`cargo fetch
--locked --manifest-path sdk/usage-source/Cargo.toml` at lines 105/168/200/232,
`build/crates/licoup-native/target` cache path at line 90) which move here as a
new `cli-ci.yml`. `client-regression.yml`'s rust lane likewise. New workflows
are authored here, not copied — the monorepo YAML mixes both worlds.

### 3.6 `package.json` scripts

Desktop `package.json` `scripts` referencing moved paths must be pruned or
rewired (§3.3): `client:format:rust`, `client:native:*`,
`client:verify:secure-mesh-*`, `client:verify:agent-*`, `client:demo:device:*`
(cargo-build prefixes), `client:contract:bundles:test`, `client:pricing:*`,
`client:state:migration*`, `client:opencode:*`, `client:pi:*`,
`client:test:android:native`, `client:upstream:probe:nightly`,
`client:verify:acp-reference-agent:self-test`, `client:verify:agent-conversation-*`,
`client:verify:agent-adapter-standard`, `client:verify:agent-native-output`,
`client:native:smoke*`, `client:verify:licoarc-badtower-acceptance*`,
`client:verify:plan`? (plan tool is Desktop). This repo gets its own minimal
`package.json` only if the moved `.mjs` tooling is kept Node-driven —
**decision needed (§5): port moved tools' npm-script surface to a
`justfile`/`Makefile`, or keep a Node harness?**

### 3.7 Workspace-root assumptions inside moved tools

Every moved `.mjs` resolves its root as
`path.resolve(fileURLToPath(new URL("../..", import.meta.url)))` — correct while
`tools/scripts/` keeps its depth (it does). What breaks is any tool that reads
`apps/`, `packages/`, `schemas/client_bridge/`, `tools/client-version.json`,
`tools/apple-release/` or `docs/` paths at that root. Fix list is exactly the
files in the `grep -rln "crates/" tools/` result **intersected** with the move
list in §1.6 — for each moved tool, audit non-`crates/`/`sdk/`/`tests/`/`components/`
root references and either move the referenced asset or parameterize the path.

### 3.8 `licoup-mobile-core` / Android seam

`crates/licoup-mobile-core` (cdylib + `jni` dep) moves here; the Gradle app
that loads it stays in `apps/desktop/android/`. Desktop's Android build must
consume the `.so` from this repo's artifacts (same seam as §3.3).

### 3.9 Docs and guides

`docs/protocols/` and crate-relevant parts of `docs/architecture/` move here
(selective copy at copy-time; the workflow guides `RUNBOOK`/`CLOSURE` and
product docs stay in Desktop). This repo authors its own `AGENTS.md`/
`CONTRIBUTING.md` — **decision needed (§5).**

## 4. Verification checklist (run after the copy)

1. `cargo metadata --locked` — every member resolves; `sdk/usage-source` and
   `components/*` stay standalone. **PASS (44 members; standalone workspaces
   also resolve `--locked`).**
2. `cargo test --workspace --no-run` — the two `[[test]]` targets at
   `../../tests/integration/{extension_isolation,usage_sources}/main.rs` resolve.
   **Partially verified:** `cargo check -p licoup-native --tests --locked` and
   `components/analytics cargo check --tests --locked` both pass, so both
   external `[[test]]` targets compile. Full `--no-run` was not executed
   (compile-clean is this lane's bar, not test execution).
3. `cargo publish --dry-run -p licoup-vault -p licoup-statistics
   -p licoup-private-gateway -p licoup-public-gateway` — path+version deps
   hold for the publishable family. **NOT RUN** (deferred — publish is gated
   anyway).
4. `node tools/scripts/cargo-client.mjs build --bin licoup-cli` — after the
   §3.1 rewire. **NOT RUN** (`cargo check -p licoup-native` covers compile;
   the runner's Node harness smoke is deferred).
5. `rg -l "apps/|packages/|schemas/client_bridge" tools/ tests/ crates/` — zero
   hits outside an allow-list of intentional cross-repo references.
   **Residual hits exist** in copied tests/tools that assert on Desktop paths
   (`apps/`, `packages/`, `docs/modules`, `docs/RUNBOOK.md` etc.) — expected
   per §3.4 note; listed in §6.

## 5. Open questions for the maintainer

1. **Version authority**: does `licoup-cli` embed the Desktop product version
   (`tools/client-version.json`, today `0.3.0`) or its own CLI version? Drives
   the §3.1 rewire.
2. **Bridge codegen**: accept recommendation §3.2(c) — generated `.rs` committed
   here, `--check` drift gate against the Desktop schema at a pinned rev?
3. **Binary delivery**: Desktop consumes prebuilt binaries from CLI releases, or
   a pinned source checkout it builds itself? Drives §3.3/§3.8 and the Desktop
   packaging rewrite.
4. **`tools/data-migration/`**: Node migration CLI — stays in Desktop as product
   tooling (recommended) or moves here beside `licoup-migrate`?
5. **Tooling runtime**: keep the moved `.mjs` fleet Node-driven with a local
   `package.json`, or port the Rust-facing subset to a `justfile`?
6. **`schemas/` destination confirm**: `extensions/` + `subagent_mcp/` move here
   (consumers moved); `client_bridge/` — resolved differently than proposed:
   vendored copy here for compile/test inputs, Desktop remains the contract
   authority (§1.7, delta D1). OK?
7. **AGENTS/CONTRIBUTING**: author fresh minimal guides here, or adapt the
   monorepo's during the copy?

## 6. Copy delta — manifest gaps found during the copy

The copy extended the manifest where moved sources required paths the manifest
did not list. Nothing was deleted from these lists; each delta names why the
path is required.

### D1 `schemas/client_bridge/` — all 9 files (vendored)

Required to compile/test moved Rust sources:

- `crates/licoup-platform-bridges/src/abi.rs:5` —
  `include_str!("../../../schemas/client_bridge/client_runtime_abi.json")`
  (hard `cargo check` blocker; fixed the first build failure).
- `crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/tests/contract.rs` —
  `include_str!`s `secure_mesh.json` (test target).
- `crates/licoup-native/tests/{cli_command_contract_cases.rs,
  package_route_registry.rs, package_lifecycle/commands.rs}` — read
  `model_selection.json`, `package.json`, `manifest.json` at test runtime.

Ownership is unchanged: Desktop owns the contract (§3.2); this is the vendored
copy the §3.2(c) recommendation already implies. Edits must still land in
Desktop first.

### D2 `tools/scripts/config/` — 16 files

`tools/scripts/config/secure-mesh-*.json` (15) are the runtime configs consumed
by the moved `client-secure-mesh-*` tools;
`tools/scripts/config/agent-conversation-verification-models.toml` is read by
the moved `tools/scripts/lib/agent-conversation-verification-models.mjs`. The
remaining config files (`client-artifact-verification-receipts*`,
`client-release-acceptance*`, `readme-fast-files.json`) stay in Desktop.

### D3 `tools/scripts/` additional entries required by moved code

| File | Pulled in by |
| --- | --- |
| `migration-crate-tests.mjs` | Invoked (`node(...)`) by the moved regression groups `rust-catalog-convergence`/`rust-platform`; it is the cargo-test runner for the moved `licoup-migrate` crate. |
| `client-gate-policy.mjs` | `import`ed by moved `tests/contract/client/model-pricing.test.mjs` (`CLIENT_GATE_LANES`, `classifyClientGatePaths`). Pure policy-data module. |
| `client-test-artifacts.mjs` | `import`ed by moved `tools/scripts/lib/test-artifact-lifecycle.test.mjs`. |
| `client-module-regression.mjs` | `import`ed by moved `tools/regression/client-module-regression-tests/support.mjs` (the module-regression runner for the rust lane). |
| `client-toolchain-env.mjs` | `import`ed by `client-module-regression.mjs`. |
| `tools/cli-version.json` (new) | §3.1 rewire of `cargo-client.mjs`. |

### D4 `tools/scripts/lib/` — exact copied subset

The `lib/` copy is the import closure of the moved scripts/tests (computed, not
guessed): `agent-conversation-release-binding.mjs`,
`agent-conversation-verification-models{,.test}.mjs`,
`bounded-child-process.mjs`, `cargo-test-filter-runner{,.test}.mjs`,
`client-release-artifact-digest{,/constants,/helpers,/path,/read,/tree}.mjs`,
`client-release-target-evidence.mjs`, `client-source-state-digest.mjs`,
`licoarc-badtower-acceptance-report.mjs`, `licoarc-badtower-candidate-binding.mjs`,
`licoarc-v1-bundle-validator.mjs`, `linux-tar-resource-bounds.mjs`,
`macos-bundle-integrity.mjs`, `privacy-test-fixtures.mjs`,
`release-closure-challenge.mjs`, `review-signoff-verifier.mjs`,
`safe-report-io.mjs`, `sanitize-error.mjs`, `secure-client-contract.mjs`,
`secure-client-mesh-authority-proof.mjs`,
`secure-client-mesh-e2ee-ref-report.mjs`,
`secure-client-mesh-e2ee-route-plan.mjs`,
`secure-client-mesh-evidence-contract.mjs`,
`secure-client-mesh-release-contract.mjs`, `secure-mesh-*-config.mjs` (12),
`secure-mesh-android-capabilities.mjs`, `secure-mesh-capability-report.mjs`,
`secure-mesh-linux-evidence{,/…}.mjs` (6), `secure-mesh-linux-node.mjs`,
`secure-mesh-linux-secret-service-capability.mjs`,
`secure-mesh-physical-report-coverage.mjs`,
`secure-mesh-trust-ux-reducer.mjs`, `source-check-bundle.mjs`,
`test-artifact-lifecycle{,.test,/…}.mjs` (11).

Stayed in Desktop: `android-*`, `better-plan-evidence-ledger*`,
`client-package-compatibility`, `client-release-dependency-receipts`,
`client-release-package-payload`, `client-release-targets`,
`macos-app-lifecycle`, `project-temporary-directory-lifecycle*`,
`release-tool-environment`, `repository-sensitive-file-policy`,
`secure-mesh-client-boundary-config`, `source-bound-policy-snapshot`,
`update-signing-keychain`, `windows-pe-facts`.

### D5 `tools/regression/` — the catalog machinery beyond `groups/rust-*.mjs`

`client-module-catalog.mjs` imports **all** eight group files
(`flutter.mjs`, `regression.mjs`, `bridge-packaging-release.mjs` included — they
are inert module-definition data), and the moved regression tests transitively
need `helpers.mjs`, `order.mjs`, `client-module-selection.mjs`,
`client-module-execution.mjs`, `client-regression-batching.mjs`,
`client-regression-compatibility.mjs`, `client-regression-metadata.mjs`,
`client-regression-metrics.mjs`, `client-regression-report.mjs`,
`client-regression-toolchain-stats/{rust,flutter}.mjs`,
`client-regression-entries/**` (also `import`ed directly by moved
`client-upstream-agent-probe.mjs`) and
`client-module-regression-tests/{support,rust-selection,secure-mesh-ownership,
conversation-ownership,platform-driver-ownership}.mjs`. All copied.

Stayed in Desktop: `client-contract-selection.mjs`,
`client-node-test-attribution-reporter.mjs`,
`client-module-regression-tests/{catalog-integrity,flutter-selection,
runner-safety}.mjs`.

The catalog's flutter/packaging group files were copied unchanged even though
they name Desktop-only paths: they are data needed for the catalog to
assemble. Whether the CLI-side catalog should drop non-CLI groups (and whether
`catalog-integrity.mjs`/`runner-safety.mjs` should then move too) is a Desktop
slim-down + tooling-runtime decision (§5.5).

### D6 `docs/` subset (§3.9 "copy-time" clause)

`docs/protocols/` (all — `lico-subagent-mcp.test.mjs` reads
`subagent-mcp.md[.zh-CN]`), `docs/architecture/` (all — every document is
crate/native-facing; `tools/architecture-graph` also publishes here),
`docs/adrs/` (all — `assistant-workflow-contract.test.mjs` reads
0003/0004/0005; the ADR set is kept whole so numbering stays coherent),
`docs/modules/endpoint-collaboration.md` (module doc for the moved
`components/endpoint-collaboration`; read by three moved `endpoint-*` tests).

Residual known-broken refs inside moved tests/tools (Desktop-owned docs:
`docs/RUNBOOK.md`, `docs/CLOSURE.md`, `docs/plans/`, `docs/functionality/`,
`apps/`, `packages/`): intentional per §3.4 — they pin the *parity* between
moved sources and Desktop surfaces, and are flagged for the follow-on lanes.

### D7 `tests/contract/client/` — applied classification (69 moved / 51 stayed + 2 dirs)

Moved the 69 tests whose primary artifact is a moved source (see the `git log`
file list; rule from §3.4 plus the manifest's explicit examples). Judgment
calls beyond the manifest examples:

- Moved (feature is native/agent/protocol, reads a staying path only
  incidentally): `agent-scan-paths`, `agent-usage-codex-source-bundle`,
  `local-data-egress-boundary`, `native-stdio-rpc-source-bundle`,
  `conversation-attachment-wire`, `endpoint-device-entry-join`,
  `agent-conversation-parity-reducer{,-source-bundle}`,
  `acp-verification-settings`, `continuous-assistant-{adoption,evaluation,
  integration,scenarios}` (run `tests/product-e2e/cli/**` + cargo targets),
  `e2ee-evidence-bundle`, `linux-node-matrix`, `linux-vm-package-receipt`,
  `macos-keychain-user-presence`, `pairwise-content-audit`,
  `secure-mesh-release-proof-bundle` source bundles (target moved
  `client-secure-mesh-*` tools), `client-state-migration{,-diagnostic}`
  (target moved `client-state-migration*` tools + crate + fixtures),
  `client-state-source-bundle`, `client-update-source-bundle`,
  `file-security-source-bundle`, `client-gate-policy`-consuming
  `model-pricing`.
- Stayed (artifact is Desktop-owned even where the test greps a moved path):
  `cli-command-admission-source-bundle` (per manifest), bridge-contract parity
  tests — `client-state-contract`, `secure-mesh-contract`,
  `continuous-assistant-contract`, `typed-client-error-chain`,
  `unified-conversation-backend`, `adaptive-flywheel-strategy`,
  `client-bridge-generation` (load `schemas/client_bridge/*` as the contract
  under test), `client-architecture-*` (assert `apps/desktop` module rules),
  `client-gate-policy`, `client-regression-batching`,
  `client-toolchain-runner-source-bundle`, `agent-auth-status` (targets
  non-moved `client-agent-auth-status` tool), `client-agent-regression-entries`
  (targets the entries registry, copied only as a transitive dep),
  `cli-vm-source-bundle`, `android-physical-install-launch-source-bundle`,
  `artifact-verification-receipts-source-bundle`,
  `release-acceptance-source-bundle`, `readme-fast-path`,
  `repository-identity-policy`, `macos-release-candidate`, `client-build`,
  `client-promotion`, `client-weekly-release`, `package-client/`,
  `verify-client-plan/`, plus the manifest's stated stays.

### D8 Repo-identity tooling required by `.githooks/`

The copied `.githooks/{commit-msg,pre-commit,pre-push}` exec
`tools/scripts/repository-identity-policy.mjs` — discovered when the first
commit attempt failed `MODULE_NOT_FOUND`. Copied with it:
`tools/scripts/repository-rulesets.mjs` (referenced as a managed path) and
`tools/scripts/lib/repository-sensitive-file-policy.mjs` (its only import).
These are repo-governance tools; §1.6's classification placed them in the
Desktop lane, but the §1.1 hooks copy makes them load-bearing here.

Install note: the identity gate derives the canonical GitHub identity from the
already-configured `user.email`, so `repository-identity-policy.mjs install`
cannot bootstrap an unconfigured checkout. The local checkout's git config was
set to the same canonical identity the source repo carries
(`user.name`/`user.email` = GitHub noreply form, `core.hooksPath=.githooks`);
the commit was made unsigned (`--no-gpg-sign`) because GitVault owner rules are
not yet configured in this repo — both expected at this stage.

### D9 Root/additional files

`tools/cli-version.json` (new, §3.1). `Cargo.lock` copied verbatim — `cargo
metadata --locked` and `cargo check --workspace --locked` both pass, so it was
NOT regenerated. `rust-toolchain.toml` already matched. Member manifests that
carried `repository = ".../LicoUp.git"` (15 files incl.
`sdk/usage-source/Cargo.toml`) were re-pointed to `LicoUp-CLI.git` to match the
staged root manifest.
