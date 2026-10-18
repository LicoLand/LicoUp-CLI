# licoup-conversation

Extracted Conversation domain crate.

## Migration Target

This crate will own the canonical Conversation domain logic, extracted from
`licoup-native/src/domain/client_conversation/` and related conversation modules.

## Responsibilities

- Conversation identity, lifecycle, and state machine
- Membership management (Human + Agent)
- Canonical Event store and paging
- Turn settlement and session binding
- Goal, Decision, Work, Artifact, Evidence, Authority bounded contexts

## Does NOT Own

- Agent adapter protocol details (→ `licoup-agent-runtime`)
- FFI/binary entry points (→ `licoup-native`)
- Crypto and key custody (→ `licoup-endpoint-core`)
- Storage engine (→ uses SQLite via typed port)

## Migration Source

```
crates/licoup-native/src/domain/client_conversation/
crates/licoup-native/src/domain/conversation_semantic.rs
crates/licoup-native/src/domain/conversation_snapshots.rs
crates/licoup-native/src/domain/conversations.rs
crates/licoup-native/src/domain/conversation_archive_jobs.rs
```
