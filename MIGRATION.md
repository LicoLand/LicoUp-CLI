# LicoUp → LicoUp-CLI move manifest

The Rust world leaves the `LicoUp` monorepo (which becomes "LicoUp Desktop":
Flutter app + packaging + bridge contract) and lands in this repository,
preserving the existing relative layout so every `path = "…"` dependency and
declared `[[test]]`/`[[bin]]` path keeps resolving. All paths below are
relative to the monorepo root `/Users/unka/DevSpace/LicoLand/LicoUp`; they copy
to the same relative path in this repository.

> Status: scaffold only. The workspace manifest is final; the source copy waits
> for the in-flight crate-split landing inside LicoUp (the working tree there
> currently deletes `crates/licoup-gateway-core/` and edits member manifests).
> Copy sources only after that branch lands, then regenerate this file's
> verification block.

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
| `schemas/client_bridge/` | **stays in Desktop** | The bridge contract is Desktop's; see §3.2 for the generated-Rust half. |

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

Required change in this repo: source the version from a CLI-owned file
(proposal: `tools/cli-version.json`, or reuse the workspace
`workspace.package.version = "0.3.0"`) and drop the `apps/` import. Keep the
`LICO_CLIENT_PRODUCT_VERSION` env-var contract — `crates/licoup-native/build.rs`
consumes it — but rename the decision to the maintainer: is the embedded
"product version" of `licoup-cli` the crate version or still the Desktop
product version? **Decision needed (§5).**

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
   `components/*` stay standalone.
2. `cargo test --workspace --no-run` — the two `[[test]]` targets at
   `../../tests/integration/{extension_isolation,usage_sources}/main.rs` resolve.
3. `cargo publish --dry-run -p licoup-vault -p licoup-statistics
   -p licoup-private-gateway -p licoup-public-gateway` — path+version deps
   hold for the publishable family.
4. `node tools/scripts/cargo-client.mjs build --bin licoup-cli` — after the
   §3.1 rewire.
5. `rg -l "apps/|packages/|schemas/client_bridge" tools/ tests/ crates/` — zero
   hits outside an allow-list of intentional cross-repo references.

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
   (consumers moved); `client_bridge/` stays. OK?
7. **AGENTS/CONTRIBUTING**: author fresh minimal guides here, or adapt the
   monorepo's during the copy?
