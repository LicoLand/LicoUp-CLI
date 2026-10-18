# Semantic Conversation Contract

[Documentation](../README.md) · [Canonical schema](../../packages/contracts/client/semantic-conversation.schema.json)

Canonical read-only model for native agent history in LicoUp.

## Authority

- Schema: `packages/contracts/client/semantic-conversation.schema.json`
- Fixtures: `packages/contracts/client/fixtures/semantic-conversation/`
- Native assembler/validator: `crates/licoup-native/src/domain/conversation_semantic.rs`
- Desktop models: `apps/desktop/lib/src/contracts/agent_conversation_models.dart`

Native source histories remain read-only. This contract does **not** introduce a parallel client-owned conversation store and must not write back into provider history databases.

## Layers

| Layer | Default visibility | Contents |
| --- | --- | --- |
| `thread` | Shown | User-authored turns and assistant replies, cleaned of injected environment context and storage metadata. |
| `execution` | Collapsed | Tool calls/results, terminal activity, plans, progress, retries, errors, reasoning summaries. |
| `artifacts` | Structured refs | Files, diffs, documents, summaries, indexes, validation outputs, archive paths. |
| `audit` | Diagnostics only | Adapter, host app, source kind, native session id, evidence refs, parse warnings, redaction/validation status. |
| `raw` | Diagnostics only | Original JSONL/JSON/SQLite/markdown/text evidence by `pathRef` + `contentHash`. |

## Privacy defaults

- `defaultView` is always `thread`.
- Default views must not dump raw JSON, absolute private workstation paths, tokens, injected environment blocks, or full command payloads.
- Execution summaries are short and redacted; full payloads stay in raw evidence.
- Audit and raw layers are reachable only through explicit diagnostic affordances or archive evidence files.

## Provider mapping

Adapters (Codex, Claude Code, Cursor/VS Code SQLite, Antigravity, and peers) classify native records into the shared layers. Provider-specific edge cases belong in adapter mappers that emit this model — not in per-provider UI forks.

## Wire shape

Sessions returned by `conversations list|stream` include a top-level `semantic` object conforming to this schema. Timeline `messages` are a projection of `thread` + `execution` only, derived from the semantic document so there is a single authority.
