# Endpoint cleanup (`org.licoland.feature.collaboration`)

The cleanup slice of the endpoint collaboration package. It owns the file half
of a staged app-data erase for one revoked LicoUp endpoint: closing the data
root's writer admission, settling the frozen file inventory entry by entry with
durable, replay-safe progress, and reporting a partial result to the
replacement endpoint over the restricted control path.

It is not part of the core. It is its own workspace and composes owners that
already exist rather than keeping a second copy of any of them.

| Module | What it owns |
| --- | --- |
| `src/cleanup/target.rs` | The frozen erase set's vocabulary: subject, old device, operation, inventory entries and the digest that binds them. |
| `src/cleanup/file_owner.rs` | The bounded platform file owner: writer quiescence through the data root's own admission lock, one-entry removal that never follows a link and never recurses, and terminal removal of this owner's own progress material. |
| `src/cleanup/journal.rs` | Durable, replay-safe progress: one ordered stage vocabulary, an atomic revision-checked document, and the refusal of a stale or rewinding writer. |
| `src/cleanup/stage.rs` | The file stage: close admission, settle each frozen entry, persist each observed outcome, and stop at `FilesSettled`. |
| `src/cleanup/receipt.rs` | The restricted receipt path and the file-stage receipt, which reports `complete: false` and restores no admission. |

## Separation

`licoup-foundation` owns the data home, the bounded private-file primitives,
the data-root admission lock and the data-root inventory grammar; this package
calls those owners.

`licoup-native` owns platform credential custody, the authenticated
replacement-endpoint authority, the bounded custody inventory and the erase
loop that walks it. This package deletes no credential, reads no protected key
and never reaches the platform keychain.

Credential deletion and terminal settlement arrive as later stages over the
same journal and the same receipt path; this slice reports them as outstanding
and implements neither.

## Tests

```sh
cargo test --manifest-path components/endpoint-collaboration/cleanup/Cargo.toml
```

The fixtures are synthetic: an in-memory file owner, a recording receipt path
and disposable roots under the platform temporary directory. No test reads,
writes or removes a real data root, a real credential, an installed
application's data or an operating-system keychain entry.

The proofs the suite ships are listed in
`tests/contract/client/endpoint-cleanup-app-files.test.mjs`; the stage-restart,
external-data-preservation, pending-entry and partial-receipt behaviours are
asserted there and in `src/cleanup/tests.rs`.

## Wiring this package needs from its owners

* The module catalog entry (`tools/regression/client-module-catalog`) and its
  order, and the `architecture.client-boundaries` input list.
* A committed `Cargo.lock` refresh in CI (`.github/workflows/client-ci.yml`)
  when the package becomes a release artifact.
* The `org.licoland.feature.collaboration` ratchet entry in
  `apps/desktop/scripts/client-architecture/ratchet/definitions.mjs` still
  declares an empty crate list; the integrator owns that registration.
