# LicoUp CLI documentation

Documentation for the standalone CLI client. This tree is populated as the
split from the monorepo progresses; the authoritative engineering guides today
live in the LicoUp monorepo's `docs/` (`RUNBOOK.md`, `CLOSURE.md`,
`COMPATIBILITY.md`) and migrate selectively per [../MIGRATION.md](../MIGRATION.md).

Planned sections:

- `architecture/` — crate-layer and module-boundary documentation for the CLI
  workspace (source: `docs/architecture/` + `tools/architecture-graph` output).
- `protocols/` — wire-protocol notes for the conversation protocol, extension
  SDK contracts and the subagent MCP service (source: `docs/protocols/`).
- `cli/` — `licoup-cli` command reference and compatibility policy.
- `crates/` — per-crate publishing notes for the `licoup-*` crate family.
