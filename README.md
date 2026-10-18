# LicoUp CLI

LicoUp CLI is the standalone command-line client of the LicoUp product — the
complete local engine behind the `licoup-cli` program and its companion
binaries. It owns the entire Rust world of the LicoUp project: the client
kernel, the agent adapter packages, the gateways, the workflow and
collaboration cores, and the publishable crate family.

The Flutter desktop application lives in the sibling repository
[LicoUp](https://github.com/LicoLand/LicoUp) ("LicoUp Desktop"), which consumes
this repository's binaries and crate releases. The bridge contract between the
two is documented there; this repository publishes the Rust side of it
(`crates/licoup-native` FFI surface).

## Status

This repository is being split out of the monorepo. The workspace manifest is
already complete, but the source trees (`crates/`, `sdk/`, `components/`,
`tests/`, `tools/`, `schemas/`) land through the move described in
[MIGRATION.md](MIGRATION.md) — `cargo` commands do not resolve until that copy
completes. The post-split plan for the desktop repository is in
[DESKTOP-SLIM.md](DESKTOP-SLIM.md).

## Layout

| Path | Contents |
| --- | --- |
| `crates/` | All workspace member crates, including the `licoup-native` host library that provides the `licoup-cli`, `lico-agent` and `lico-secure-mesh-kt-mock` binaries and the `cdylib`/`staticlib` FFI surface. |
| `sdk/` | Standalone SDK sources kept outside the root workspace: `usage-source` (Rust, own lockfile) and `agent-adapter` (Python SDK + samples). |
| `components/` | Optional feature packages (`analytics`, `appearance`, `endpoint-collaboration`), each a standalone workspace with its own lockfile. |
| `tests/` | Integration, contract, smoke and product-e2e suites for the CLI and its binaries. |
| `tools/` | Rust-facing tooling: cargo runners, dependency audit, codegen, agent-adapter standards checks. |
| `schemas/` | Rust-side protocol schemas (conversation protocol, extensions, subagent MCP). The `client_bridge` schema lives in the Desktop repository. |
| `docs/` | CLI documentation. |

## Binaries

| Binary | Declared by | Role |
| --- | --- | --- |
| `licoup-cli` | `crates/licoup-native` | The complete client CLI — the main entry point. |
| `lico-agent` | `crates/licoup-native` | Lico Agent program. |
| `lico-gateway` / `lico-llm-gateway` | `crates/licoup-private-gateway/entry` | Private gateway runtime programs (optional package). |
| `lico-subagent-mcp` | `crates/licoup-public-gateway` | Subagent MCP service program. |
| `lico-telegram-channel` | `crates/licoup-public-gateway/entry` | Telegram channel program (optional package). |
| `lico-agent-<name>` | `crates/licoup-agent-*` | Per-adapter helper programs (codex, claude-code, copilot, pi, antigravity, cursor, deepseek, kimi, hermes, kilo, lico-agent, openclaw, opencode). |
| `licoup-migrate` | `crates/licoup-migrate` | Standalone local-data migration tool. |

## Build

```sh
cargo build --bin licoup-cli
```

Toolchain: Rust 1.95.0 (see `rust-toolchain.toml`, edition 2024). Release builds
use `panic = "abort"`, LTO and a single codegen unit; `cargo build --release`
reproduces the published profile.

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
