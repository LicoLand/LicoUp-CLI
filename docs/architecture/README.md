# LicoUp CLI Architecture

| Related Document | Language / Path | Authority |
|:---|:---|:---|
| **Normative Version** | English (Normative) | Authoritative architecture index for this repository |
| **Localization** | [简体中文](README.zh-CN.md) | Localized Chinese projection |
| **Documentation Index** | [docs/README.md](../README.md) | Documentation table of contents |

This repository owns the LicoUp CLI workspace: the Rust crates behind
`licoup`, plus their schemas, SDK and
components. The Flutter desktop client and the product-level guides live in the
LicoUp monorepo; this page indexes only the architecture documentation kept in
this tree.

## Architecture document index

| Document | Scope |
|:---|:---|
| [CLIENT-NATIVE-INTERACTION.md](CLIENT-NATIVE-INTERACTION.md) | `licoup.stdio.v1` structured method frame contracts between hosts and the native core |
| [NATIVE-CLI.md](NATIVE-CLI.md) | Registry-derived commands, persistent native methods, independent MCP process lifecycle |
| [CONVERSATION-DOMAIN.md](CONVERSATION-DOMAIN.md) | Canonical Conversation domain: memberships, dispatch door, turn host, state machine and flows |
| [CONVERSATION-VERTICAL-CONTRACT.md](CONVERSATION-VERTICAL-CONTRACT.md) | L1–L6 interface specification for the conversation vertical |
| [AGENT-ADAPTERS-ARCHITECTURE.md](AGENT-ADAPTERS-ARCHITECTURE.md) | Agent adapter taxonomy and protocol normalization (ACP, app-server, RPC, CLI) |
| [RUST-INFRASTRUCTURE-LAYER.md](RUST-INFRASTRUCTURE-LAYER.md) | SQLite storage, dynamic configuration, secret custody, transport, PTY/TTY |
| [SECURITY-AND-DATA-BOUNDARY.md](SECURITY-AND-DATA-BOUNDARY.md) | VM discovery isolation, LicoArc device boundary, platform secret custody, zero-trust data rules |
| [MODEL-REGISTRY.md](MODEL-REGISTRY.md) | Canonical model identity, dynamic directory, usage controls |
| [EXTENSION-PLATFORM.md](EXTENSION-PLATFORM.md) | Extension contract, profiles, lifecycle and the kernel decision |
| [DEPLOYMENT-PROFILES.md](DEPLOYMENT-PROFILES.md) | Installation profiles, install closure, per-capability ownership |
| [CONTINUOUS-ASSISTANT.md](CONTINUOUS-ASSISTANT.md) | Target continuity semantics |
| [ASSISTANT-WORKFLOW-CONTROL.md](ASSISTANT-WORKFLOW-CONTROL.md) | Compiler layers, node publish/subscribe, graceful intervention |
| [CLIENT-UPDATE-AND-STATE-MIGRATION.md](CLIENT-UPDATE-AND-STATE-MIGRATION.md) | Client update and client-state migration ownership |
| [../protocols/README.md](../protocols/README.md) | Wire protocols and artifact formats |
| [../adrs/README.md](../adrs/README.md) | Architecture decision records |
| [../modules/endpoint-collaboration.md](../modules/endpoint-collaboration.md) | Endpoint collaboration module notes |

## Repository structure

| Path | Purpose |
|:---|:---|
| `crates/` | The `licoup-*` workspace crates: the `licoup` host, domain crates, agent adapters and runtime, gateways, platform bridges, state stores |
| `components/` | Packaged components in their own workspaces (`appearance/`, `endpoint-collaboration/`) |
| `schemas/` | Client-owned wire and contract schemas (`client_bridge`, `conversation_protocol`, `extensions`, `subagent_mcp`) |
| `sdk/` | Adapter SDK support material |
| `tests/` | Contract, integration and smoke tests over synthetic data |
| `tools/` | Build, regression and release tooling |
