# Endpoint collaboration

Endpoint collaboration (`endpoint-collaboration.v1`,
`org.licoland.feature.collaboration`) is the optional package that carries
cross-device collaboration for one LicoUp account: the transfer of an account's
data to a replacement endpoint, and the bounded, recoverable removal of the old
endpoint's LicoUp-owned data afterwards.

This document is the owning module guidance for the package. The package
manifest, availability and profile rules belong to
[Deployment profiles](../architecture/DEPLOYMENT-PROFILES.md) and the extension
platform; the protocol semantics belong to Lico Arc Protocol. Nothing here
restates them.

## Modules

| Module | What it owns |
| --- | --- |
| `components/endpoint-collaboration/src/lib.rs` | The package's availability boundary: which of *missing*, *disabled*, *capability-undeclared*, *unreadable* and *active* the host resolved, and the single outbound cut every caller checks. |
| `components/endpoint-collaboration/src/capability_catalogue.rs` | The client-side catalogue capability flows are announced into: device and group identity kept apart, explicit local grants, rotation epochs that only advance, and pending protected material that outlives every catalogue change. |
| `components/endpoint-collaboration/src/durable_delivery.rs` | The package's implementation of the caller-owned durable protocol store: one commit applies snapshot, custody mutations and delivery units together, and only an accepted attempt settles a committed unit. |
| `components/endpoint-collaboration/cleanup/` | The file half of a staged erase: close the data root's writer admission, settle the frozen file inventory entry by entry with durable, replay-safe progress, and report a partial file-stage result to the replacement endpoint. It deletes no credential and reads no protected key. |
| `components/endpoint-collaboration/control/` | Remote work control and settlement: the typed inspect/stop/owned-child/force intents, the local grant and verified-ingress decision, the durable request ledger that admits before any effect, the consumer-owned port the kernel's own work owners implement, and the rules for what the local update gate may wait for. It performs no effect itself. |
| `components/endpoint-collaboration/transfer/` | Device-transfer ownership inventory: classify one full-data-root archive into managed payloads, external references and nonportable credential requirements. |
| `crates/licoup-native/src/domain/mobile_relay/secret_custody/cleanup_authority.rs` | The cleanup-specific authorization: derive the custody subject from persisted identity, accept a replacement endpoint only against a locally signed device trust record that verifies, enumerate the bounded custody inventory, and require an explicit informed confirmation naming that subject, that replacement endpoint, the inventory digest and every scope entry. |
| `crates/licoup-native/src/domain/mobile_relay/secret_custody/cleanup.rs` | The custody consumer: delete exactly the authorized credentials through the platform secret store's own authorized session, remove exactly the authorized durable-store files, and report observed settlement. |

## Capability synchronisation

`capability_catalogue.rs` is the client-side catalogue the package's capability
flows are announced into. It keeps apart the facts that are easy to collapse:

* A **device identity** and a **group identity** are different facts, and a peer
  is the pair. The same device in two groups is two peers, and a group is never
  re-owned by whichever device announced it last.
* An **announcement is not authority**. A recorded announcement answers
  `AnnouncedNotAuthorized` until this client grants that capability explicitly;
  recording a peer's own claim grants nothing, and no announcement revokes a
  grant either.
* A **stale announcement is an explicit answer**. An older revision is refused
  with both revisions, because offline catch-up must be able to tell "nothing
  new" from "I am behind" instead of silently dropping the batch.
* A **rotation epoch only advances**, mirroring the directory's own
  `identityRotationEpoch` rule. A rolled-back epoch is refused with both epochs;
  an advancing one keeps the device identity, its local grants and its protected
  material, so material committed before the rotation stays recoverable
  afterwards.
* A **required capability cannot be downgraded away**, and neither can one this
  client still holds protected material for. Protected material leaves the
  catalogue only through an explicit settlement of that material's own identity.

None of this changes a session. Applying a catalogue change never rebuilds a
peer, re-admits a call or rewrites a stored conversation, and the module performs
no cryptographic operation, opens no store and sends no packet.

## Remote work control and settlement

`control/` owns the decision half of a remote stop. It performs no effect: the
kernel's own work owners — the persistent conversation turn, the durable
workflow run, the Subagent MCP dispatch claim and the supervised lane session —
implement the `LocalWorkOwner` port, and the kernel's force control terminates
only a process group whose durable ownership record it re-verifies at execution
time.

* **Admission precedes effect.** A request is recorded before any owner is asked,
  and a second delivery of one request identity is answered from the record, so a
  duplicate or replayed control cannot repeat a stop. A request that reuses an
  identity with different content is refused rather than treated as new work.
* **Authority is local and current.** A verified ingress is necessary and not
  sufficient; the requester must also hold a current local grant covering the
  intent. Force control additionally requires a target scope this host verifies
  as its own and the locally produced redacted diagnostics.
* **A request is never proof of exit.** Only an observed end is recorded as
  confirmed; an owner's acknowledgement, an unobserved end and an unknown effect
  each stay visible as what they are.
* **Only locally admitted execution enters the local idle guard.** Work this host
  performs blocks a local update until its end is authenticated, including work a
  peer asked for, which still passes this host's own admission. Awaiting or
  displaying a peer-owned result creates no local work and never holds the gate.
* **A local observation is not a remote outcome.** Losing the carrier, losing a
  grant or letting time pass moves no recorded remote state and permits no blind
  retry. Only an authenticated receipt settles a remote state, and only forwards:
  an older cursor is refused as stale and a confirmed end is absorbing.
* **Compatible local updates preserve what this host owes.** Identities, cursors
  and unknown-effect records survive an update that keeps the local identity; an
  incompatible update changes nothing here, because remote abandonment belongs to
  the protocol owner rather than to this slice.

## The approved credential deletion route

The old endpoint is revoked and is not present to cooperate. Its manual-presence
requirement is therefore replaced — **only** for the deletion of its own bound
LicoUp inventory — by two facts established on the replacement endpoint:

1. **Strict authentication.** The replacement endpoint is the persisted peer
   identity, accepted only when the device trust record the subject itself
   signed for that peer verifies, is unexpired and reports the `verified` trust
   state. A persisted flag or any caller-supplied claim authenticates nothing.
2. **Explicit informed confirmation.** The confirmation is a record, not a
   boolean. It must name the derived subject (endpoint identifier, identity
   fingerprint, custody namespace), the authenticated replacement endpoint and
   its trust fingerprint, the enumerated inventory digest, the consequence
   `irreversibleLocalSecretErasure`, and every entry of the bounded scope. An
   entry outside the enumerated inventory is refused rather than erased.

The authorized set is the exact, deduplicated custody inventory owned by that
subject: the root secret handles, the pairwise snapshot handles and the fixed
set of durable-store files the pairwise store owns. The erase loop walks that
set and nothing else, sizes the platform secret store's authorization batch from
it, and checks the session's own consumed-operation count afterwards so a wider
session than declared cannot pass unnoticed.

Ordinary protected operations keep their existing authorization. The cleanup
route neither reads, exports nor signs with a protected key, and it returns no
reusable handle or session to any other caller.

## Settlement is observed, never assumed

A cleanup reports `complete` only when every authorized credential deletion and
every authorized durable-store file was observed settled. A locked platform
store, an unavailable backend or a denied file removal leaves the report
`partial` with the exact pending entries; it is never converted into a finished
erase. After the last observation the cleaned side writes nothing — no data
root, log, temporary payload or credential is recreated there.

The cleanup's own progress material is owned by the data root's cleanup state
directory and is settled by the terminal stage. Only the replacement endpoint
holds the final evidence: the receipt block is issued with
`replacementEndpointConfirmed: false` until that endpoint observes arrival, so a
lost receipt is never silently converted into success.

## What this package does not claim

Removing LicoUp-managed files and credentials leaves forensic traces on the
storage device and in operating-system backups. Neither the package nor this
document promises forensic or backup erasure.

## Tests

```sh
cargo test --manifest-path components/endpoint-collaboration/Cargo.toml
cargo test --manifest-path components/endpoint-collaboration/cleanup/Cargo.toml
cargo test -p licoup-native --lib secret_custody
```

The component's fixtures are synthetic: an in-memory file owner, a recording
receipt path, disposable temporary roots and synthetic peers with synthetic
capability announcements. The native custody fixtures seed synthetic identities,
trust records and an ephemeral secret store; no test reads or removes a real
credential, keychain entry or installed application's data.
