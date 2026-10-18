# LicoUp Desktop — post-split slim tree plan

Decision (maintainer): the `LicoUp` repo becomes **"LicoUp Desktop"** — the
Flutter client, its packaging, and the bridge contract. Every Rust-side path
that moves to `LicoUp-CLI` is **deleted from the Desktop working tree in one
slim-down change**; Git history is untouched, so the Rust sources remain
recoverable from any pre-split commit. No subtree rewrites, no filter-branch —
the split is a deletion commit on top of intact history.

This plan is the Desktop-side mirror of [MIGRATION.md](MIGRATION.md); section
numbers cross-reference the move manifest.

## 1. Removes — deleted from the Desktop tree

| Path | MIGRATION ref |
| --- | --- |
| `crates/` — entire tree (all 41 crate dirs incl. `licoup-private-gateway/entry`, `licoup-public-gateway/entry`) | §1.2 |
| `sdk/` — `usage-source/` + `agent-adapter/` | §1.3 |
| `components/` — `analytics`, `appearance`, `endpoint-collaboration` | §1.4 |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/config.toml` | §1.1 |
| `tests/integration/` — `extension_isolation/`, `usage_sources/`, `package_lifecycle/` | §1.5 |
| `tests/smoke/` | §1.5 |
| `tests/product-e2e/cli/` | §1.5 |
| `tests/fixtures/client_state_migration/`, `tests/fixtures/continuous-assistant/` | §1.5 |
| `tests/contract/client/` Rust-bound subset (per the §3.4 grep rule) | §3.4 |
| `tools/acp-reference-agent/`, `tools/architecture-graph/` | §1.6 |
| `tools/scripts/` Rust lane: `cargo-client.mjs`, `client-deps-audit.mjs`, `client-rustsec-exceptions.mjs`, `client-native-performance.mjs`, `protocol-codegen/`, `client-agent-*`, `client-acp-conversation-parity.mjs`, `client-upstream-agent-probe.mjs`, `opencode-add-licoup-gateway.mjs`, `pi-add-licoup-gateway.mjs`, `client-secure-mesh-*`, `client-state-migration*`, `client-android-native-tests.mjs`, `model-pricing-facts.mjs`, `transcript-record/` | §1.6 |
| `tools/scripts/lib/` files only consumed by moved scripts (`test-artifact-lifecycle*`, `source-check-bundle.mjs`, `secure-mesh-capability-report.mjs`, `privacy-test-fixtures.mjs` — after checking §3.4 residual imports) | §1.6 |
| `tools/regression/` rust lane: `client-module-catalog/groups/rust-*.mjs` + cargo-attributing entries in `client-module-regression-tests/` | §1.6 |
| `schemas/conversation_protocol/`, `schemas/extensions/`, `schemas/subagent_mcp/` | §1.7 |
| `target/`, `build/crates/`, `cache/` (untracked build dirs) | — |

## 2. Keeps — the Desktop tree after slim-down

```
LicoUp/                              # = "LicoUp Desktop"
├── apps/desktop/                    # the Flutter client — lib/, test/,
│                                    #   android|ios|linux|macos|windows/,
│                                    #   packaging/, packaging.modules.json,
│                                    #   scripts/, pubspec.*, analysis_options
├── packages/                        # Dart: presentation_contract,
│                                    #   presentation_flutter, presentation_runtime;
│                                    #   contracts/client/ schemas+fixtures,
│                                    #   protocols/native-client/ docs
├── schemas/client_bridge/           # THE bridge contract (Desktop-owned)
├── tools/                           # product-side tooling
│   ├── apple-release/  distribution/  release/  templates/  development/
│   ├── data-migration/              # pending Q4 — recommended stay
│   ├── client-*.json                # version + release target manifests
│   ├── verify-client-boundary.mjs   verify-documentation.mjs
│   ├── regression/                  # flutter + packaging lanes only
│   └── scripts/                     # client-build, client-gate, release,
│                                    #   install, device-demo, version,
│                                    #   generate-client-bridge-contracts …
├── tests/
│   ├── contract/client/             # packaging/release/version/layout/golden
│   │                                #   subset that remains
│   ├── fixtures/client_package_release/
│   ├── verify-client-update-release-channel.mjs
│   └── verify-windows-file-security-boundary.mjs
├── docs/                            # RUNBOOK, CLOSURE, COMPATIBILITY, STATUS,
│                                    #   RELEASE-PACKAGES, adrs/, plans/,
│                                    #   reports/, platforms/, releases/,
│                                    #   functionality/, modules/, examples/,
│                                    #   assets/ — desktop-relevant subset of
│                                    #   architecture/, protocols/ stays here;
│                                    #   copies may also land in CLI (Q: §5.9)
├── .github/workflows/               # Flutter + packaging + release lanes
├── .githooks/  .gitignore           # unchanged
├── AGENTS.md  CONTRIBUTING*.md  CODE_OF_CONDUCT.md  SECURITY*.md
├── README*.md  PRODUCT*.md  CONTEXT.md  CHANGELOG.md  LICENSE  NOTICE
├── package.json  package-lock.json  node_modules/
├── brand/  readme-preview.html  Height  design-qa.md
└── .claude/  .cursor/  .vscode/  .tmp/  (untracked locals, unchanged)
```

`Height` and `design-qa.md` are tracked one-off artifacts at the root — not
part of either product surface; maintainer may fold them into the slim-down or
leave them (history keeps them either way).

## 3. Desktop-side fixups required by the slim-down

1. **`package.json` scripts** — prune or rewire every entry that calls a moved
   path (full list: MIGRATION §3.6). The npm surface shrinks to
   Flutter/packaging/release/contract lanes.
2. **`apps/desktop/scripts/package-client/build/native.mjs`** — `SIDECAR_CRATES`
   stops building `crates/*` manifests locally; replace with the §3.3
   binary-delivery decision (prebuilt artifact fetch vs pinned source build).
   Until that lands, `client:package`/`client:build` cannot produce sidecars.
3. **`tools/scripts/generate-client-bridge-contracts.mjs`** — keep reading
   `schemas/client_bridge/`; the `crates/licoup-native/src/ffi/generated/`
   output path no longer exists in this tree (emit only the Dart half locally;
   Rust half handled per MIGRATION §3.2).
4. **`.github/workflows/`** — remove cargo lanes (`sdk/usage-source` fetches,
   `build/crates/licoup-native/target` cache, `cargo fmt/clippy/test` steps)
   from `client-ci.yml` and `client-regression.yml`.
5. **`tools/verify-client-boundary.mjs`, `tools/verify-documentation.mjs`** —
   drop `crates/`/`sdk/`/`components/` from scanned/expected trees.
6. **`tools/regression/` catalog** — remove `rust-*` groups and their entries
   from `client-module-catalog*`; keep `flutter.mjs`,
   `bridge-packaging-release.mjs`.
7. **Contract tests** — delete the moved `tests/contract/client/*` files in the
   same commit as the sources they assert on (per MIGRATION §3.4).
8. **Docs** — `docs/` pages describing Rust internals stay (they document
   product behavior), but any doc whose *procedure* runs `cargo`/`crates/` paths
   needs a pointer to LicoUp-CLI. Sweep: `rg -n "crates/|cargo " docs/`.
9. **`.gitignore`** — keep the Rust entries (harmless) or prune; either way the
   `crates/licoup-native/target/` line becomes dead.
10. **`apps/desktop` FFI loading** — wherever the client loads
    `liblicoup_native`/`native_backend` (the cdylib/staticlib produced by the
    moved crate), the artifact now arrives via the §3.3 delivery seam instead of
    a local `cargo build`.

## 4. Sequencing

1. LicoUp-CLI source copy lands and its §4 checklist passes.
2. Binary-delivery decision (MIGRATION Q3) is made → Desktop packaging rewired.
3. Single slim-down commit in LicoUp removes §1 and applies §3 fixups.
4. Desktop gate: `npm run client:gate:verify` equivalent minus the Rust lane —
   run the remaining (Flutter/packaging/contract) profile.

## 5. Notes

- Git history is intentionally **not** rewritten: `git log -- crates/` in the
  slim repo still reaches every pre-split change, satisfying "slim tree, keep
  history".
- The two repos share `LICENSE`/`NOTICE` copies; the Desktop `NOTICE` keeps its
  `LicoUp` URL.
- Nothing in the Desktop tree may keep a `path = "../../crates/…"` style
  reference after the slim-down — `rg 'crates/|sdk/usage-source'` over tracked
  files must return only history/doc mentions and the intentional seam of §3.3.
