# Lico Arc Station Adapter

English (normative) ·
[简体中文](licoarc-station-adapter.zh-CN.md) ·
[Protocol index](README.md)

This document describes LicoUp's boundary against the published
`licoarc-client` adapter. Lico Arc Protocol remains the wire authority; the
station-facing transport contract, envelope validation, Pairwise Protection,
session, and replay policy are implemented by the pinned `licoarc` /
`licoarc-client` releases, not by this repository. A station is independently
implemented and intentionally untrusted; no station repository is a runtime
or release dependency of LicoUp.

## Protocol and endpoint authority

Lico Arc Protocol owns every stable wire-observable Pairwise Protection,
Generic Message, Reliable Exchange, negotiation, and Transport Profile
contract. LicoUp owns conforming local execution, private keys, Provider
configuration, plaintext, history, backups, user trust, approvals, and local
effects.

The retired in-repository endpoint-protection preview — the local
`licoarc.relay.v1` outer adapter, its OpenMLS pairwise sessions, ratchets,
key transparency, and the HTTP BadTower station transport — is removed. The
published LicoArc client stack is the only station adapter this repository
executes; there is no dual station-wire compatibility mode or translation
gateway.

## Local adapter boundary

LicoUp's side of the adapter owns four bounded responsibilities:

- Credentials. The station URL and access token are explicit client
  configuration; no official-network default is populated. The device
  signing key never leaves custody: `SessionCustody` resolves key handles
  through `KeyCustody` into the operating-system secret store and hands the
  SDK custody handles, never exported key material.
- Durable state. Pairing offers/claims, peer sessions, cursors, and the
  outbound outbox are persisted atomically under the device data home by
  `SessionState` / `DevicesState`. `devices sync` retries durable outbound
  work for every peer session, including sessions untouched by the current
  process.
- Receive loop. While the device is enrolled, the host runs
  `MailboxClient` with the SDK's reconnect semantics: unbounded retry with
  jittered backoff, terminal on superseded, revoked, device-mismatch, or
  refused. Every delivery is forwarded to the processing thread; a
  delivery's ACK is sent only after the processor reports `Done`, and a
  `Defer` leaves it stored for the next connection to redeliver.
- Bridged dispatch. Session protocol work runs on a synchronous processing
  thread; each outbound packet crosses a bridged `Transport` that blocks
  until the async half reports the station outcome — accepted, rejected,
  transient, or ambiguous.

## Untrusted station boundary

Station-reported state is a transport hint, never endpoint evidence. A
lease, an acceptance flag, a duplicate flag, a deletion acknowledgement, an
HTTP status, a station timestamp, or queue state cannot establish delivery,
admission, read, or acceptance — the SDK's `StationHint` type makes this
constant (`is_endpoint_evidence()` is `false`).

The adapter accepts HTTPS origins and loopback HTTP for bounded local work.
It does not discover a station, import sibling source, or accept a
station-provided algorithm, key, trust root, identity, policy, or executable
code.

## Acceptance order

A received packet is eligible for ACK and removal only after the client:

1. accepts it through the SDK's closed outer validation;
2. authenticates and verifies it inside the LicoArc session engine —
   expected peer, session, direction, freshness, and replay state; and
3. the processor applies the endpoint-authenticated command or result
   transition and reports `Done`.

No station hint can bypass this order or establish final receipt.

## Local verification

Deterministic in-process coverage exercises two data homes end to end:
pairing offer/claim/cancel, paired message send and result retrieval,
duplicate idempotency, unpair refusal while live work remains, completed-
pairing cancellation that cannot destroy a live session, and `devices sync`
draining durable outbound work across all peer sessions. Live station
acceptance remains a separately assigned workflow and is not engineering
proof for this boundary.

Diagnostics stay bounded: no private paths, stored values, credential
material, raw secrets, or ciphertext appear in command output or evidence.
