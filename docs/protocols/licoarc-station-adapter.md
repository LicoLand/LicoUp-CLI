# Lico Arc Candidate Station Adapter

English (normative) ·
[简体中文](licoarc-station-adapter.zh-CN.md) ·
[Protocol index](README.md)

This document describes LicoUp's client-owned adaptation of endpoint-protected
content to the candidate `licoarc.relay.v1` station-facing contract. Lico Arc
Protocol remains the wire authority. BadTower remains an independently
implemented, intentionally untrusted station. Neither repository is a runtime
or release dependency of LicoUp.

## Protocol and endpoint authority

Lico Arc Protocol owns every stable wire-observable Pairwise Protection,
Generic Message, Reliable Exchange, negotiation, and Transport Profile
contract. LicoUp owns conforming local execution, private keys, Provider
configuration, plaintext, history, backups, user trust, approvals, and local
effects.

The [current retiring endpoint-protection Preview](../STATUS.md) is a LicoUp
implementation, not a Lico Arc Profile. It has no future interoperability
promise and is to be retired directly when a complete pinned Lico Arc Protocol
Line replaces it. This is independent of the current `licoarc.relay.v1` outer
adapter described below, which remains an implemented and locally verified
Candidate adapter.

## Closed outer boundary

The adapter emits and accepts exactly the five fields defined by the pinned
candidate:

- `contractVersion`;
- `envelopeId`;
- `mailboxId`;
- `ciphertext`; and
- `expiresAt`.

The client rejects an unknown field, duplicate field, unsupported contract
identifier, invalid identifier, invalid expiry, malformed carrier, or
over-limit value. The outer object contains no plaintext field.

The endpoint-protected carrier is one canonical value inside `ciphertext`.
LicoUp binds the complete outer routing context as authenticated data and
protects the private header with XChaCha20-Poly1305. The endpoint content
remains protected by the current endpoint-protection Preview session and
ratchet. Those inner carrier, session, and ratchet details describe the current
LicoUp preview; they are not a normative Lico Arc Pairwise Protection or
Transport Profile and are not BadTower algorithms.

## Four station operations

The BadTower transport adapter exposes only four bounded operations:

| Operation | Station-local effect | Endpoint meaning |
| --- | --- | --- |
| Lease mailbox | Request temporary mailbox work eligibility | Untrusted transport hint |
| Send envelope | Submit one closed Lico Arc envelope | Station acceptance is not peer receipt |
| Receive envelopes | Read one bounded candidate set | Every value requires strict endpoint verification |
| Delete envelope | Request removal of one received envelope | Station acknowledgement is not endpoint evidence |

The station URL is explicit client configuration. No official-network default
is populated. The adapter accepts HTTPS origins and loopback HTTP for bounded
local work; it does not discover a station, import sibling source, or accept a
station-provided algorithm, key, trust root, identity, policy, or executable
code.

## Endpoint acceptance order

A received envelope is eligible for deletion only after the client:

1. strictly validates the five-field outer object and encrypted carrier;
2. authenticates and decrypts the private header and protected content;
3. checks the expected endpoint, session, direction, freshness, and replay
   state; and
4. accepts the endpoint-authenticated command or result transition.

A lease, HTTP status, station timestamp, queue state, acceptance flag,
duplicate flag, or deletion acknowledgement cannot bypass this order or
establish final receipt.

## Verified candidate scenario

The bounded local acceptance uses two freshly initialized endpoints with
separate client state, a pinned Lico Arc candidate bundle, and an actual
BadTower candidate process. It verifies:

- one protected command and authenticated result round trip;
- exactly five station-visible outer fields;
- absence of endpoint plaintext from station-visible storage;
- rejection of unsupported and extended envelopes; and
- no promotion of station transport hints into endpoint evidence.

The receipt is privacy-minimal and excludes endpoint content, ciphertext, key
material, endpoint or machine identity, private addresses, and raw runtime
records.

This scenario is candidate interoperability evidence only. It does not publish
Lico Arc Protocol, release LicoUp or BadTower, declare platform support, or
prove that an official network is operating.

## Migration state

The retired client-specific station envelope/API, route family,
service-session scope, configuration, fixtures, and documentation are removed.
There is no dual station-wire compatibility mode or station translation
gateway. That completed station migration does not mean that the
current retiring endpoint-protection Preview has already been removed or
accepted as a Lico Arc Profile.
