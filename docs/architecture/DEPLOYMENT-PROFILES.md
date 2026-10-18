# Deployment profiles, package closure and capability ownership

Updated: 2026-10-03

| Related Document | Language / Path | Authority |
|:---|:---|:---|
| **Normative Version** | English (Normative) | Authoritative contract for what is installed and what may be left out |
| **Extension contract** | [EXTENSION-PLATFORM.md](EXTENSION-PLATFORM.md) | Profiles, carrier, lifecycle and versions |
| **Schemas** | [`schemas/extensions/deployment.schema.json`](../../schemas/extensions/deployment.schema.json) | Machine-readable form of the facts below |
| **Ownership and closure rules** | [`crates/licoup-extension-contracts/src/deployment.rs`](../../crates/licoup-extension-contracts/src/deployment.rs) | The closure algorithm, the ownership table and the refusals |
| **Package lifecycle** | [`crates/licoup-native/src/platform/extension_packages/`](../../crates/licoup-native/src/platform/extension_packages) | Install, activation admission, uninstall and storage accounting |
| **Security and data boundaries** | [SECURITY-AND-DATA-BOUNDARY.md](SECURITY-AND-DATA-BOUNDARY.md) | Trust, consent and data handling |
| **Current evidence** | [STATUS.md](../STATUS.md) | Implemented and verified capability |
| **Documentation Index** | [docs/README.md](../README.md) | Complete documentation table of contents |

This document publishes what a distribution may contain, how the install closure
is computed from a package's own declarations, which capability belongs to the
untouchable kernel versus a package a user may leave out, and the rule every
first-party package follows. Per-platform delivery is a separate question and is
recorded in the platform matrix and release evidence.

## 1. One installation choice, not several products

A user selects a profile, or selects packages directly. Every selection uses the
same kernel, the same capability contracts and the same extension SDK. A profile
is a **recipe for an initial selection**, never a permanent prohibition: the user
may change profile or add, disable and remove packages at any time.

| Profile | Selected packages | Intent |
|:---|:---|:---|
| `minimal-local` | the kernel only | The whole kernel with no optional package |
| `standard` | the kernel and a small set of first-party packages | A default that runs out of the box |
| `gateway` | the kernel, the gateway package and compatible provider support | Routing across several providers |
| `peer` | the kernel, a general adapter and endpoint collaboration | Cross-device use |
| `analytics` | the kernel, a general adapter and the statistics package | Extended usage and metric surfaces |

There is deliberately no `workflow` profile. The workflow and flywheel are kernel
capabilities, so no profile selects them and no absence of a package removes them.

`minimal-local` is a complete product, not a crippled one. With no gateway, no
pairing, no orchestration and no advanced statistics, the client must still start
offline, chat, stop, show history and report basic diagnostics; the packages that
are absent must not be loaded, must not open listeners, must not require
migrations of schemas that do not exist and must not ask for keys or an online
catalog.

A released client bundle carries no optional package payload at all. Each
capability in the table below arrives as its own released package, so the minimal
client is not "the full client with features switched off": the payload is not in
the bundle, nothing starts it and no listener is opened for it. The MCP service is
the worked example — `lico-subagent-mcp` is staged into the
`org.licoland.feature.mcp` package at release time, no packaging module bundles
it, and the package's own `on-demand` activation means a caller starts it rather
than client startup.

## 2. The kernel and its rule

The installed client is the minimal trusted host, and it carries:

| Kernel capability | What it is |
|:---|:---|
| Canonical conversation | Direct and group conversation, membership and Profile |
| The Assistant | The continuous collaboration loop |
| The workflow and flywheel | Durable multi-step work over the conversation |
| The extension host and package management | Discovery, install, activation, update, disable and uninstall of packages |
| The base usage journal | The small core slice that keeps the ledger's ownership, without copying it into a second store |
| Declarative interface primitives | The compiled contributions other profiles' packages bind to |
| The generic PTY/CLI adapter with Agent Hub registration | The base lane for an ordinary CLI or a new Agent, plus its configuration registration |
| Key custody | Endpoint and credential custody layered on the platform's own store |
| Update and migration admission | The admission a client update or a state migration passes through |

**The kernel does not depend on anything removable.** An optional package may
depend on the kernel and on shared runtime; the reverse direction is refused by
contract. A kernel package that declares an install dependency on a trimmable
package is refused by the closure with `core_requires_optional_package` rather
than caught in review, because it is exactly the shape by which a "small kernel"
quietly becomes one large bundle.

Shared infrastructure — TLS, the database, the operating-system shim, the general
display primitives — stays in the kernel where it is genuinely needed, and is
counted in the kernel's real size. Logical module boundaries are not install
boundaries: one package may contain several cohesive modules, and one shared
runtime may serve several packages under explicit dependency and trust boundaries.

## 3. Per-capability ownership

Every capability has one owner: the kernel, or a specific package the user may
leave out. One capability may be *offered* by several packages — three adapters
all provide `agent-execution.v1` — and the table names the one the default
distribution provides.

| Capability | Owner | Set |
|:---|:---|:---|
| `conversation.v1` | `org.licoland.core` | Core |
| `assistant.v1` | `org.licoland.core` | Core |
| `workflow.v1` | `org.licoland.core` | Core |
| `extension-host.v1` | `org.licoland.core` | Core |
| `usage-journal.v1` | `org.licoland.core` | Core |
| `declarative-ui.v1` | `org.licoland.core` | Core |
| `agent-execution.v1` | `org.licoland.adapter.generic` | Optional |
| `model-provider.v1` | `org.licoland.provider.compat` | Optional |
| `model-gateway.v1` | `org.licoland.feature.gateway` | Optional |
| `analytics.v1` | `org.licoland.feature.analytics` | Optional |
| `endpoint-collaboration.v1` | `org.licoland.feature.collaboration` | Optional |
| `mcp-server.v1` | `org.licoland.feature.mcp` | Optional |
| `channel-connector.v1` | `org.licoland.channel.telegram` | Optional |

The default package set is the kernel plus one package per optional capability:
`org.licoland.core`, `org.licoland.adapter.generic`, `org.licoland.provider.compat`,
`org.licoland.feature.gateway`, `org.licoland.feature.analytics`,
`org.licoland.feature.collaboration`, `org.licoland.feature.mcp` and
`org.licoland.channel.telegram`.

The base usage journal is deliberately a small kernel slice that keeps the
existing ledger's ownership; advanced queries, policies and panels are optional
and do not copy the same data into a second store. A base metering fact is not
deleted because a panel was removed.

## 4. The install closure

A manifest declares two deployment relations, and they are not the same relation:

| Field | Meaning |
|:---|:---|
| `requires` | Installed together with this package. This is the install closure |
| `optionalRequires` | Enabled when the user already has it. Never installed on this package's account, and never a reason to refuse the package |

The closure follows `requires` only. An unknown package — a root that is not
present, or a required dependency that is not — refuses the plan with the
dependent package named, because "this package needs something you do not have" is
a fact the user can act on. A cycle among `requires` entries is refused outright:
there is no order in which such a set can be installed. Optional dependencies that
were declined are reported, so the user can decide rather than have the decision
made silently.

These relations are **deployment** relations. They decide which packages are
installed together and nothing about the development task graph that produced
them; package ranges are recorded rather than solved, and constraint resolution,
artifact selection and the install journal belong to the host's package manager.

## 5. The first-party package rule

Every package carries a self-described compatibility list
(`compatibility.clientVersions` in the manifest): the client versions it supports,
each entry a bare major version (`"1"`) or a semantic-version range. The kernel
loads a package only when that list covers the running client.

| Rule | Statement |
|:---|:---|
| First-party is not exempt | Every first-party package carries the list, and the sample package under the contract crate carries one too |
| A different version is not a refusal | Packages are versioned and released independently of the client; the package's own version takes no part in the decision |
| `hostProtocol` is a different fact | It is the wire contract range, negotiated per connection, and it never stands in for the compatibility list |
| Refused at install | A list that does not cover the running client refuses the install with `package_client_incompatible`, before anything is published |
| Refused at activation | Activation checks again, because a client update may have moved past a list that covered the package when it was installed. Refusing activation is not uninstalling |
| Nothing declared, nothing admitted | A package that declares no readable list is refused as `manifest_invalid` with field `compatibility` |

## 6. Permissions

A manifest asks, and the host grants. There is no field in which a package can
record that it has been granted something, a directory listing is not an
authorization, and installing from a directory does not widen a scope.

- A first install, a widened scope and a new network domain each need the user's
  agreement or an existing explicit policy.
- Post-install scripts are not executed. A descriptor's command is a start target
  the user authorized, not an installer; scripts a package carries are recorded so
  a user can see them.
- Signatures and content hashes are produced from the bytes by packaging and by
  the update mechanism. A manifest that claims it has already been verified is
  refused.
- Signing in may unlock a directory. It is never required: importing a package the
  user already has is a first-class path, and binding trust to specific content is
  enough for a self-made one.
- A permission already granted does not automatically follow a changed credential
  domain, and removing the related package does not silently restore an old
  default.

## 7. Four facts, never one

Available, installed, enabled and active are four independent facts, and there is
no single "ready" that merges them. The interesting states are the mixed ones.

| Fact | Meaning |
|:---|:---|
| Available | Present in a directory. A local import was never in a directory, so being installed does not imply this |
| Installed | On disk, under a content-addressed location that never overwrites a file in use |
| Enabled | Installed and not disabled by the user. Enabled is not started: activation stays lazy |
| Active | An instance of this package is running |

A package's own lifecycle runs from available through downloaded, verified or
locally approved, staged and installed. A publisher signature that was checked
against a trusted update mechanism and a user's trust bound to specific content
are two different facts, and neither is more installed than the other. Instance
lifecycle — discovered, preparing, active, draining, stopped, failed, quarantined
— is separate again, because one package version may produce several instances
with different permissions. Preparing an instance is the activation step, and it
is reachable only with the admission section 5 describes.

Removing a package withdraws new admission, lets existing instances drain while
the user may also cancel what remains, revokes the contribution and credential
abilities, stops the process and then removes code with no remaining reference.
History, credentials and protocol state are separate, explicitly requested
operations. A runtime the user installed is never removed, a shared runtime is
removed only when no package references it, and space that is still occupied by a
pinned old generation is not reported as freed.

## 8. An unavailable capability is a catalogue fact

A capability that is not served is reported where capabilities are listed, with a
state — served, installed but not enabled, not installed, not in this
distribution — and not as an error banner. An operation that needs it is refused
as an unavailable capability whose recovery points at installing or enabling the
package that carries it, never as a malformed request and never as a reason the
client failed.

Not installing a package is a legitimate configuration, and the client must be
able to say so. A missing optional runtime is a fact about this installation.

## 9. What this milestone implements

| Part | State |
|:---|:---|
| The ownership table, the kernel rule and the closure | Implemented in the contract crate and pinned by its tests |
| The profile list | Published by `deployment.schema.json` and checked against this document's shape; no installer selects a profile yet |
| The first-party package rule | Implemented as the manifest compatibility contract and enforced by the package store at install and at activation |
| Package facts, lifecycle states and unavailability | Implemented in the contract crate and the package store |

Selecting a profile, recommending packages at first launch and publishing an
installation's deployment document are delivered by the package-pipeline
milestone. This document does not claim a profile has been installed.

## 10. Not claimed here

This document does not state that any listed package has been built, signed,
published or measured. Package identifiers describe the default distribution's
intent; artifact inventories, link closures, digest material, download and
expanded sizes, and per-platform delivery rules are separate, measured facts
recorded in release evidence.
