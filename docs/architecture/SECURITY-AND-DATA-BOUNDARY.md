# Security Architecture and Data Boundaries

Updated: 2026-10-01

English (Normative) · [简体中文](SECURITY-AND-DATA-BOUNDARY.zh-CN.md) · [Back to Architecture README](README.md)

This document defines LicoUp client security boundaries, data flow rules, virtual machine integration isolation, and endpoint encryption standards.

## 1. Virtual Machine Discovery and Remote Protocol Boundaries

For OpenClaw and Hermes, the desktop client enumerates running local OrbStack VMs via bounded commands and checks fixed official and standard binary locations; it does not read VM configuration or history. Rust validates the VM name and returned absolute path before creating a temporary `machine@orb` route. Discovered VM routes do not enter the discovery cache.

For other VMs, Flutter collects hostname, optional port/user, in-VM binary, and absolute working directory; Rust validates a closed connection structure and persists it only in authoritative manual targets. Passwords, private keys, command fragments, relative directories, and unknown fields are rejected.

The native core starts the platform system `ssh` executable in batch mode with strict host-key checking, no TTY, no forwarding, no local command execution, no environment forwarding, and no connection multiplexing:
```bash
ssh -o BatchMode=yes -o StrictHostKeyChecking=yes <user>@<host> <command>
```
It passes one fixed, shell-quoted guest command. Both ACP and Hermes TUI gateway protocols use bounded JSON-RPC over stdin/stdout.

## 2. Retiring Endpoint-Protection Preview Layers

The current retiring endpoint-protection Preview uses a fixed security profile:

```mermaid
flowchart TB
    ID["Peer identity<br/>Ed25519 signatures"] --> SETUP["Session setup<br/>X25519 + ML-KEM-1024"]
    SETUP --> DERIVE["Key derivation and ratchets<br/>HKDF-SHA256"]
    DERIVE --> CONTENT["Message protection<br/>ChaCha20-Poly1305"]
    CONTENT --> VERIFY["Verify before use<br/>no plaintext fallback"]
```

Algorithms are combined only when they have distinct roles and validated compositions. The profile locks during the initial handshake. Missing or failed security checks strictly prohibit fallback to plaintext communication.

## 3. Platform Secret Custody

The client probes platform capabilities and selects system secure storage when available, otherwise explicitly falling back to ephemeral in-memory storage. Private key custody and local Provider selection remain LicoUp responsibilities; wire-observable profiles and negotiation belong to the fixed Lico Arc Protocol Line.

The canonical five-field Lico Arc envelope, bounded carrier, private-header format, mailbox derivation, and authenticated-padding bucket validator have one implementation in `licoup-protocol-bindings`. Native endpoint callers supply ratchet/header material and retain content encryption, bucket selection and padding effects, key custody, persistence, and Station I/O; they consume the shared codec directly.

## 4. Provider-Managed History and Recovery

Provider-managed cloud history is a separate source from the local Canonical
Conversation store. After provider authorization, retained history is readable
by default; the default history read does not call a recovery key. Client-side
encryption for this history is an explicit opt-in and is not silently enabled
by the default path.

History recovery restores every retained object that the authorized provider
still makes available. Provider access rules continue to apply, so recovery cannot
bypass access or recreate objects that are missing, deleted, expired, or
otherwise unavailable. Identity recovery material is separately authenticated
and never locks default history reads. Replacement-device recovery prepares
identity authority and the complete available history before one atomic
caller-owned commit; identity material cannot restore missing history.

LicoUp never routes this history through or stores it at a Station; malicious
Stations remain outside the history path. This path makes no mandatory notary
or endpoint attestation/evidence promise; any evidence LicoUp shows remains
local and scoped to the operation that produced it.

```mermaid
flowchart LR
    U["User"] --> A["Provider authorization"]
    A --> H["Provider-managed retained history"]
    H --> L["LicoUp local projection"]
    R["History recovery"] --> H
    E["Explicit client encryption"] -.-> H
    I["Identity recovery"] -.->|separate flow| K["Endpoint identity and keys"]
    S["Malicious Station<br/>transport only"]
```

## 5. Data Boundaries and Rules

```mermaid
sequenceDiagram
    participant A as Client A
    participant R as Compatible untrusted station
    participant B as Client B
    A->>A: User selects B and approves content
    A->>A: Encrypt for B
    A->>R: 5-field Lico Arc envelope
    R->>B: Forward opaque protected payload
    B->>B: Authenticate, verify freshness/replay, and decrypt
```

The client strictly adheres to these data boundaries:
- **Data Locality**: Local paths, logs, canonical history, local projections, usage records, credentials, and raw runtime data stay on-device. Provider-managed history remains with its authorized provider; LicoUp does not use a Station as its storage.
- **Plaintext Control**: Default scenarios never send sensitive runtime data or user content in plaintext to servers.
- **Controlled Disclosure**: External MCP requests contain only exact text and files shown in one-shot user confirmations.
- **Ciphertext in Transit**: Content leaving the client without explicit external service confirmation must be encrypted for the designated peer.
- **Encrypt-then-Send, Verify-then-Use**: Senders encrypt before network transmission; receivers authenticate and verify freshness/anti-replay before consumption.
- **Zero-Trust Stations**: Compatible stations are outside the trusted boundary and outside LicoUp's history path. Private keys and approval policies stay entirely with endpoints.
- **Safe Summaries**: Logs and reports retain only security summaries, never raw user content or secret keys.

## 6. Collaboration authority and isolation

For the accepted workflow refactor, IPC caller admission, Conversation Membership
and operation grants remain separate from operating-system isolation. An untrusted
plugin needs process, permission and resource isolation that matches its threat model;
a checked caller or working directory alone is not a sandbox. Use the owning platform
and component boundaries without granting broader access to support collaboration.

Revocation prevents new control and disclosure. An authenticated result from an already
known invocation can still settle that invocation under its original identity and
restricted visibility. Recording the fact does not renew a grant, permit another effect,
or authorize a successor to read its content. These requirements must be verified at
the actual adapter and store boundaries; this section makes no new sandbox claim.

## 7. Device Identity Ownership and Credential Custody

One subject may operate several devices, and each device owns a distinct device
identity. A replacement device provisions new per-device identity material
through the local custody owner; an active device key is never duplicated,
exported or installed from a backup. A replacement import that targets a home
already declaring a device identity is refused without mutating the target's
custody or identity state. Cross-device
activation, revocation and wiping remain separate authenticated operations and
are not provided by local recovery.

Credential classes have distinct custody and reacquisition paths:

| Credential class | Material location | Recovery or reauthorization path |
| --- | --- | --- |
| Device identity key | Platform secret custody of the owning device | Provision new per-device identity material; never copy an active key |
| Provider API key | Platform secret custody, one entry per provider credential | Reauthorize the provider and enter the key again |
| Relay access token | Platform secret custody keyed by the local custody namespace | Re-pair the device or reauthorize it with its peer |
| Non-exportable platform credential | Platform secret custody item with no portable form | Reacquire through the platform custody owner |

Relay custody namespaces derive from configuration paths. Durable configuration
retains the originating namespace as a locator, never as authorization or proof
of custody. A same-device restore may retain the same identity and fingerprint
in either the original or a different data home only after platform authorization
and verification that locally held private material matches the recorded public
identity. Destination custody takes precedence: a different existing identity
refuses the operation rather than falling back to the source. Empty destination
custody may resolve the origin locator; keys are not copied to a new namespace.
Missing custody, including loss in the original home, never permits silent rekeying.

Archive import prepares non-secret origin metadata without authorizing custody,
advancing the stored schema or granting device activation. It refuses embedded
device private material. Public configuration projections redact custody locators;
transported runtime bundles carry no local-custody verification authority. Disposable
cleanup cannot use an origin locator to delete another home's custody.

Credential inventory distinguishes authorized verified custody, unavailable custody
and custody not observed by the current operation. It exposes no secret values or
raw custody handles, and metadata never proves that key material is available.
Archive coverage remains limited by non-exportable custody. Possession of an
ordinary backup grants no takeover, revocation or wiping authority.
