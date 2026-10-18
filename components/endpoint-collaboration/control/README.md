# Endpoint remote work control (`org.licoland.feature.collaboration`)

The remote work control slice of the endpoint collaboration package. It owns the
*decision* half of a remote stop: which verified requester may ask, which
already-admitted work a request selects, which request identity has already been
answered, and what this host is allowed to claim afterwards.

It is not part of the core. It is its own workspace and reaches the work through
the consumer-owned port the kernel implements rather than keeping a second copy
of any owner.

| Module | What it owns |
| --- | --- |
| `src/control/intent.rs` | The typed intents (`work.inspect`, `work.stop`, `work.stopOwnedChild`, `work.forceStop`), the durable owner a target names, and the bounded redacted diagnostics a force control carries. |
| `src/control/authority.rs` | The ordered control authority, the local grant table, and the verified ingress facts. An announcement grants nothing; a grant is local state. |
| `src/control/ledger.rs` | The durable request ledger: admit before any effect, answer a duplicate from the record, refuse a reused identity, and keep an owner's acknowledgement apart from an observed end. |
| `src/control/owner.rs` | The consumer-owned `LocalWorkOwner` port the kernel implements, and the owner's own report of what it selected. |
| `src/control/settlement.rs` | Where a result is settled: only locally admitted execution enters the local idle guard, only an authenticated receipt moves a remote state, and a local observation proves no remote outcome. |
| `src/control/mod.rs` | The entry that orders the decisions: verified ingress, current grant, verified ownership, admission, then the owner's own answer. |

## Separation

The kernel owns every effect: the persistent conversation turn, the durable
workflow run, the Subagent MCP dispatch claim and the supervised lane session
each resolve their own durable owner, and the kernel's force control terminates
only a process group whose durable ownership record it re-verifies at execution
time. This package calls those owners through `LocalWorkOwner`; it terminates no
process, reads no protected key, opens no store and authenticates no peer.

Protocol compatibility, authorization and replay semantics over the wire stay
with the pinned SDK and the existing endpoint boundary. This slice owns only the
client-side decision and its durable record.

## Tests

```sh
cargo test --manifest-path components/endpoint-collaboration/control/Cargo.toml
```

The fixtures are synthetic: an in-memory work owner with a small target tree,
synthetic endpoint identities and synthetic request identities. No test performs
a real stop, force stop, remote control, authentication or device operation.

## Wiring this package needs from its owners

* A kernel adapter that implements `LocalWorkOwner` over
  `crates/licoup-native/src/platform/stop_control.rs` and the
  `secure_mesh_command_runtime` command arms, so the unsupported-command gap is
  replaced by a real call. The package does not depend on `licoup-native`, and
  the kernel does not yet depend on this package.
* The client-bridge schema (`schemas/client_bridge/remote_work.json`) if the
  intents are to cross the Dart boundary as generated types.
* The module catalog entry (`tools/regression/client-module-catalog`) and its
  order, and the `architecture.client-boundaries` input list.
