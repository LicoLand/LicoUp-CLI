# LicoUp CLI

LicoUp CLI is the standalone command-line client of the LicoUp product — the
complete local engine behind the `licoup` program and its process entries. It owns the entire Rust world of the LicoUp project: the client
kernel, the agent adapter packages, the gateways, the workflow and
collaboration cores, and the publishable crate family.

The Flutter desktop application lives in the sibling repository
[LicoUp](https://github.com/LicoLand/LicoUp) ("LicoUp Desktop"), which consumes
this repository's binaries and crate releases. This repository owns the
canonical wire and bridge schemas under `schemas/` (`client_bridge`,
`conversation_protocol`, `extensions`, `subagent_mcp`) and generates the Rust
FFI and CLI contracts from them (`crates/licoup` FFI surface); the Desktop
repository syncs those schema definitions for Dart-side code generation.

## Status

The Rust world of LicoUp lives here; the Desktop repository consumes the
released `licoup` binary through the release archive and its `contract.json`
(command catalog and conversation protocol digest). Cross-device pairing runs
on LicoArc through the `licoarc-client` crate (`licoup devices …`).

## Layout

| Path | Contents |
| --- | --- |
| `crates/` | All workspace member crates, including the `licoup` host library that provides the `licoup` binary and the `cdylib`/`staticlib` FFI surface. |
| `sdk/` | Standalone SDK sources kept outside the root workspace: `agent-adapter` (Python SDK + samples). Usage detection moved to the `Agent-Statistics` repository (`token-usage-detector`). |
| `components/` | Optional feature packages (`analytics`, `appearance`, `endpoint-collaboration`), each a standalone workspace with its own lockfile. |
| `tests/` | Integration, contract, smoke and product-e2e suites for the CLI and its binaries. |
| `tools/` | Rust-facing tooling: cargo runners, dependency audit, codegen, agent-adapter standards checks. |
| `schemas/` | Canonical wire and bridge contract schemas owned by this repository (`client_bridge`, `conversation_protocol`, `extensions`, `subagent_mcp`). |
| `docs/` | CLI documentation. |

## Binaries

| Binary | Declared by | Role |
| --- | --- | --- |
| `licoup` | `crates/licoup` | The complete client program: the CLI, the Lico Agent runtime (`licoup agent run`), the gateway service process (`licoup gateway run`) and the migration tool (`licoup migrate`). |
| `lico-subagent-mcp` | `crates/licoup-public-gateway` | Subagent MCP service program. |
| `lico-telegram-channel` | `crates/licoup-public-gateway/entry` | Telegram channel program (optional package). |
| `lico-agent-<name>` | `crates/licoup-agent-*` | Per-adapter helper programs (codex, claude-code, copilot, pi, antigravity, cursor, deepseek, kimi, hermes, kilo, lico-agent, openclaw, opencode). |

## Build

```sh
cargo build --bin licoup
```

Toolchain: Rust 1.95.0 (see `rust-toolchain.toml`, edition 2024). Release builds
use `panic = "abort"`, LTO and a single codegen unit; `cargo build --release`
reproduces the published profile.

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
