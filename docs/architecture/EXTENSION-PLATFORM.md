# Extension platform and SDK contract

Updated: 2026-10-03

| Related Document | Language / Path | Authority |
|:---|:---|:---|
| **Normative Version** | English (Normative) | Authoritative contract for the extension platform |
| **Deployment and ownership** | [DEPLOYMENT-PROFILES.md](DEPLOYMENT-PROFILES.md) | Profiles, install closure, the kernel table and the first-party package rule |
| **Schemas** | [`schemas/extensions/`](../../schemas/extensions) | Machine-readable form of every shape below |
| **Contract crate** | [`crates/licoup-extension-contracts/`](../../crates/licoup-extension-contracts) | The rules, catalogs and refusals in executable form |
| **Package lifecycle** | [`crates/licoup-native/src/platform/extension_packages/`](../../crates/licoup-native/src/platform/extension_packages) | Install, journal, state machines, storage accounting and uninstall |
| **Agent adapters** | [AGENT-ADAPTERS-ARCHITECTURE.md](AGENT-ADAPTERS-ARCHITECTURE.md) | Current adapter and runtime layering |
| **Assistant and workflow** | [CONTINUOUS-ASSISTANT.md](CONTINUOUS-ASSISTANT.md) · [ASSISTANT-WORKFLOW-CONTROL.md](ASSISTANT-WORKFLOW-CONTROL.md) | Kernel capabilities, not packages |
| **Security and data boundaries** | [SECURITY-AND-DATA-BOUNDARY.md](SECURITY-AND-DATA-BOUNDARY.md) | Trust, isolation and data-handling rules |
| **Current evidence** | [STATUS.md](../STATUS.md) | Implemented and verified capability |
| **Documentation Index** | [docs/README.md](../README.md) | Complete documentation table of contents |

This document publishes the contract an extension is written against, and the
kernel decision that decides which packages the client may load at all. It is a
contract, not a claim about a shipped feature set: what the running client already
enforces is recorded in [STATUS.md](../STATUS.md) and in release evidence, and
section 12 states which parts of this document this milestone implements.

## 1. What an extension is

An extension is a **separate program**. The client starts it, speaks one
line-delimited JSON-RPC 2.0 protocol to it, and renders its results. It is not a
Dart widget injected into the shipped client, not a dynamically loaded library in
the client's address space, and not a second copy of the client's internal
objects.

The consequence that matters for authors is that the contract is
**language-agnostic**. A Rust, Go, Python, Node or shell program that reads one
JSON object per line and writes one JSON object per line is a complete extension.
No SDK runtime is required, and no client internals are exposed: an extension
never sees the conversation domain, the workflow engine or the client's state
root.

Three properties are guaranteed by contract rather than by convention:

- **Local import is first-class.** A package a user built, a directory on this
  machine, an official directory and a third-party mirror are all just sources.
  Installing a local package skips the network entirely, and no part of this
  contract requires a registry, a directory service or an account.
- **Absence is a catalogue fact.** A capability whose package is not installed is
  reported as an unavailable capability with a stated next step. It is never a
  malformed request and never a reason the rest of the client fails.
- **Nothing is bundled for a vendor.** The kernel carries the generic PTY/CLI
  adapter and the extension host; a vendor implementation is a package a user
  installs and may remove. See [DEPLOYMENT-PROFILES.md](DEPLOYMENT-PROFILES.md).

## 2. The five profiles

A package declares which of five **narrow** profiles it serves. There is no
universal `call` interface: a universal interface would make every extension
responsible for every host feature and would make "this pack does not do models"
indistinguishable from "this pack is broken".

| Id | Contract | Required methods | Optional methods | What holds one profile |
|:---|:---|:---|:---|:---|
| `agent-execution` | C09 | `extension.initialize`, `extension.ready`, `extension.shutdown`, `agent.describe`, `agent.execute`, `agent.event` | `agent.cancel`, `agent.observe`, `agent.resume`, `agent.steer`, `agent.fork`, `agent.reconcile`, `agent.history`, `agent.models` | Local import and process carriage |
| `model-provider` | C10 | `extension.initialize`, `extension.ready`, `extension.shutdown`, `modelProvider.describe`, `modelProvider.stream` | `modelProvider.models`, `modelProvider.cancel`, `modelProvider.reconcile`, `auth.begin`, `auth.continue`, `auth.refresh`, `auth.revoke` | User-supplied configuration or a provider package |
| `usage-metric` | C11 | the handshake, and at least one of `usage.publish` or `usage.query` | `usage.describe` | The kernel usage journal, or an optional source package |
| `package-deployment` | C12 | none | none | Declarations and package facts; no call is made |
| `declarative-ui` | C13 | none | none | A resource other profiles' packages contribute |

A profile decision is **local**. An extension that declares a profile this host
does not publish keeps working — the declaration is preserved and never acted on.
An extension whose `agent-execution` major is 2 is refused for Agent work and for
nothing else. An extension that implements `agent.describe` and `agent.execute`
but not `agent.event` is refused for Agent work with a refusal that names the
missing method. The same extension's other profiles are unaffected, and so is the
rest of the client.

The smallest useful Agent implements exactly the six `agent-execution` methods and
is complete. It needs no model, no tool call, no session resume, no usage
reporting and no remote idempotency, and none of those absences is a defect. The
sample under
[`samples/echo-agent/`](../../crates/licoup-extension-contracts/samples/echo-agent)
is that smallest complete Agent.

## 3. The carrier

The baseline carrier is a child process: one frame per line, `stdout` carrying the
program protocol and nothing else, diagnostics on `stderr` and bounded. A local
IPC channel and an already-connected service are the other two carriers; they
speak the same profile, so an extension does not become a different kind of
program by moving from a pipe to a socket. A connected service is bridged rather
than started, and reports its own cancellation and recovery abilities.

| Bound | Value | Why it exists |
|:---|:---|:---|
| Initial frame bound | 64 KiB, negotiated to the smaller of the two sides | A frame bound that only one side sets is a memory ceiling the other can raise |
| Accepted range | 4 KiB to 8 MiB | Below the floor, ordinary descriptions do not fit; above the ceiling, "negotiable" is an unbounded buffer |
| Diagnostic line | 8 KiB | A chatty adapter must not be able to stop the client from responding |
| Reserved control slots | 4 frames | A cancellation may not queue behind a large data frame |

Oversize payloads are **chunked** or replaced by a **controlled blob handle** the
host issued. A handle is an opaque token, never a filesystem path: accepting a
path would turn "here is a large file" into an instruction for the host to read a
file the extension chose.

On a carrier that supports only one stream, the honest statement about
cancellation is a **worst-case wait** — the negotiated frame bound times the
reserved slots — not a claim that an unsplittable write can be preempted in the
middle. Cancellation is a request: once an effect has left the machine, the client
can stop waiting and stop local work, and it cannot claim the remote effect was
withdrawn.

A `process` runtime may declare the interpreter or virtual machine its entry needs,
and where that reference points decides who owns it
(`crates/licoup-extension-contracts/src/manifest.rs`). A `user:` reference names
something the user installed: the host reuses it, never removes it, and never
bundles an interpreter of its own for it. Any other shared runtime the host
installed is reference-counted and released only when no package needs it. A
`declarative` descriptor, a `service` endpoint and a `data` package own no runtime
at all: the first two are projections of something that already exists, and the
third is its typed resources. The two categories are exclusive in both directions
— a package that declares typed resources must be carried by `data`
(`data_package_executable_refused`), and a data package may declare no profile
(`data_package_profile_refused`).

## 4. Lifecycle and identity

The handshake is three methods and no business call:

1. `extension.initialize` negotiates the host protocol and the profile set.
2. `extension.ready` publishes the fact that the extension is prepared.
3. `extension.shutdown` asks for an ordered exit.

An extension with no ordinary business call is never started. Declaring a profile
costs nothing until something needs it.

Instance binding is established **once** per stream, not repeated per event:

```
extensionId / packageDigest / packageVersion / instanceId / generation /
registryEpoch / profileVersion / configRevision / authorityRef
```

A content event therefore carries only its invocation reference, a monotone
sequence and its own kind; the cursor and the identity come from the binding. The
permission handle in that list is assembled by the host and cannot be supplied by
input with the same name.

Package state and instance state are different facts and are never collapsed into
one version token. A package version may produce several instances with different
permissions, and `packageVersion`, `instanceId`, `generation` and the registry
epoch are four separate identities.

Replacing an installed version, or activating one, changes state that running work
is reading, so both mutating maintenance operations pass one seam
(`platform/extension_packages/maintenance.rs`). The verdict that no local work is
in flight belongs to a single idle-guard owner, it is read as data for the data
root the operation would change, and a verdict nobody read is a refusal rather
than an assumed idle host. Read-only work — checking for an update, reading the
catalogue — is not gated at all. Asking is not holding: the seam answers whether
the operation may proceed, and the caller that actually changes installed state
takes the durable close-admission barrier itself.

## 5. Agent execution

| Method or event | Requirement | Meaning |
|:---|:---|:---|
| `agent.describe` | required | Instance kind, input kinds, capabilities, interface version. Answerable from the package manifest, and it performs no paid inference |
| `agent.execute` | required | Submits an admission the host has already accepted, and returns a receipt that is not a completion |
| `agent.event` | required | `text`, `artifact`, `progress`, `state`, `terminal`. The SDK owns the envelope; the body is carried verbatim |
| `agent.cancel` | optional | `requested`, `acknowledged`, `unsupported` and `unknown` are four separately visible outcomes |
| `agent.observe`, `agent.resume` | optional | Address a specific prior invocation and cursor. Re-sending the same task is not a resume |
| `agent.steer`, `agent.fork` | optional | Only where the native capability exists; a native session is not a Conversation identity |
| `agent.reconcile` | optional | Queries a prior effect identity; when it is unsupported the result stays Unknown |
| `agent.history`, `agent.models` | optional | Paginated read-only history and catalog; they never block ordinary execution |

Two rules keep the client from telling a comfortable story:

- **Admission is not completion.** The receipt of `agent.execute` reports that the
  work was taken, that it was already taken, that it was never started, or that
  the outcome was not observed. A terminal *event* is the only thing that reports
  an end, and it is a different fact from "the call came back".
- **A dropped connection is an unknown effect, not a retry.** With no remote
  idempotency, a submission that was sent and never answered may or may not have
  started something. Only an extension that reports *not started* is a basis for
  submitting the same invocation again; everything else is reconciled against the
  durable record. An equal outer request id proves nothing about the remote
  service.

An ordinary CLI is carried by the generic adapter in the kernel, which converts
its output into text and terminal events. A user does not have to modify an Agent
to emit client JSON, and text that merely *looks* like an envelope is still text.

A declared cancel capability states what the extension can do; the host never
upgrades it. The four `agent.cancel` outcomes stay distinct
(`crates/licoup-extension-contracts/src/agent.rs`): only `acknowledged` means the
work has demonstrably stopped, `requested` means the request left the machine and
the effect may still be running, `unsupported` means the extension has no cancel at
all, and `unknown` means the answer was not observed. Cancellation is a request in
every case, and no outcome settles an external effect.

Manual stop and force stop are separate authorities
(`platform/stop_control.rs`). One manual-stop entry point resolves the durable
owner of a piece of admitted work — the persistent conversation turn, the durable
workflow run, the Subagent MCP dispatch claim, or the supervised lane session — and
routes the request to that owner's own dispatcher. Force stop is narrower: it may
terminate only a LicoUp-owned process group whose durable ownership record is
re-verified at execution time, and it never widens to a shared or external process.
A request is never proof of exit: a bounded observation that sees no exit reports
the stop as unconfirmed rather than as a stopped process, and every outcome is
recorded through the private activity log as a bounded, redacted event keyed by an
opaque correlation id.

## 6. Model providers

A provider owns a model protocol, its authentication, its model catalog and its
stream. Gateway functionality is one consumer of providers, and it is not a
prerequisite for defining one: a user who already has a local endpoint never has
to install a gateway to reach it, and removing a gateway does not remove a
provider another consumer still uses.

Configuration carries `id`, `displayName`, `baseUrl`, `apiDialect`,
`credentialRef`, `configRevision`, `catalogSource`, models and versioned
compatibility options. No endpoint and no model id is pre-listed, and the
compatibility parameters are separate from key material.

| Rule | Statement |
|:---|:---|
| Unknown stays unknown | A model whose vendor publishes no context limit, tool support or reasoning options reports nothing. No default window, no implied `false` |
| Keys are handles | The configuration carries a `credential:` handle. Key material pasted inline is refused before the configuration is stored, logged or graphed |
| Scope is per endpoint | A handle issued for one provider and origin is not resolved for another. Pointing a provider at a new base URL does not inherit the old endpoint's secret |
| Custom APIs are custom | A dialect that is not a published compatible one owes its own stream adapter rather than imitating a compatible API |
| Catalog keys are tuples | `(providerId, providerGeneration, vendorModelId)`. A human alias is a separate mapping, and only an explicit replacement changes future selection |

A price table is estimate material and is not an incurred cost. Cost is an
observation under C11, an unknown cost is reported as unknown rather than as zero,
and one provider's removal neither rewrites another provider's configuration nor
restores an arbitrary earlier default.

## 7. Usage and metrics

A usage source produces **observations**, not charges. The host decides what an
observation means for budget and settlement.

| Field | Meaning |
|:---|:---|
| `observationId`, `revision`, `operation` | Stable identity within one source epoch; a correction raises the revision; `retract` withdraws that observation and nothing else |
| `sourceEpoch` | Reset and replay defence. A new epoch starts new cumulative series |
| `scopeRef` | An invocation or an authorized aggregate read range issued by the host |
| `observedAt`, `intervalStart` | UTC as observed, and the window or series origin. An anomalous clock is preserved as the source's own fact and never overrides the host's monotonic clock |
| `metrics` | Namespaced metric to reading with `value`, `unit`, `temporality`, `quality` and included subsets |

The **source** is bound by the transport and is never self-declared. An
observation carries no `extensionId`, no `instanceId` and no principal, and a
payload that asserts one is refused, so no producer can attach itself to another
user's run.

Money is an exact decimal string with an ISO 4217 code; binary floating point does
not enter a ledger. A cumulative counter is differenced only within one epoch, one
metric and one origin, and a regressed counter means a reset and a new origin
rather than a negative amount of work. A value the producer does not know is
`unknown` with no number at all, because a zero would be a claim that nothing
happened. A producer reporting `reported` gains no settlement authority from
saying so.

Sources push through `usage.publish` or are pulled through
`usage.query(cursor, limit, scopeRef)`; `usage.describe` declares the fields and
series they offer. Log and OpenTelemetry adapters normalize at the boundary, and
the client's surfaces read standardized queries rather than a vendor's own files.

## 8. Declarative interface contributions

A contribution is **data**. It names one of the client's precompiled primitives,
the resource or action it binds to, and the least profile it needs. Allowed
contributions are `settings`, `command`, `navigation`, `metric-panel` and
`resource-view`; the primitives are form, table, chart, progress, text and action.
Focus, input-method editing, accessibility, theming and prepared-value consistency
belong to those host primitives.

- There is no script field, no widget tree and no direct RPC handle. A downloaded
  package cannot put code into an already-shipped client, and a capability that
  genuinely needs a new primitive negotiates a core version instead of shipping
  one.
- A contribution that needs a profile this host does not serve is not mounted.
  Every other contribution, and every other feature of the client, mounts exactly
  as before; a missing optional package cannot stop an unrelated page from
  building.
- A field may declare that a secret is needed and may not carry one. The host
  collects it in its own control and stores a credential handle; the raw secret
  never enters the contribution's data.
- A prepared value computed for a generation that has been replaced is refused by
  generation, whatever its arrival time says, and no unrelated feature re-reads
  its source on its account.

## 9. Versions: the wire range and the compatibility list

Two declarations are separate facts, and neither substitutes for the other:

| Declaration | Field | What it decides |
|:---|:---|:---|
| Wire contract range | `hostProtocol` (`major` plus `minimumMinor`) | Which protocol generation this package speaks. It is negotiated per connection, and a different major refuses this package's operations and nothing else |
| Client compatibility list | `compatibility.clientVersions` | Which client builds may load this package at all. Each entry is a bare major version (`"1"`) or a semantic-version range (`">=0.3.0, <0.5.0"`) |

The package's **own** version is a third fact and decides nothing about
admission. A package is versioned, released and updated independently of the
client, so a package version that differs from the client is not itself a refusal.
The client loads a package only when the package's own compatibility list covers
the running client, and it refuses one that does not:

| Moment | Behaviour |
|:---|:---|
| Install | Refused before anything is published, with the stable reason `package_client_incompatible` and the client version it was decided against. Nothing is left staged |
| Activation | Checked again, because a client update may have moved past a list that covered the package when it was installed. Refusing activation is not uninstalling: the files stay where they are |

A list that is absent or names no readable version requirement is a structural
defect of the manifest (`manifest_invalid`, field `compatibility`), so a package
that declares nothing is admitted by nothing rather than by everything.

Package ranges declared in `requires` and `optionalRequires` are recorded, not
solved, by the planning tools; constraint resolution, artifact selection and the
install journal belong to the host's package manager. A declaration graph
validating does not prove that the code is separated.

## 10. Trust and permissions

| Rule | Statement |
|:---|:---|
| A manifest has no digest and no signature | The content hash and publisher verification are produced from the bytes by packaging and by the update mechanism. A manifest that declares itself verified is refused by the contract reader, field by field |
| A permission request is a request | There is no `granted` field anywhere in the manifest, and a directory listing is not an authorization |
| Consent is explicit | A first install, a widened scope and a new network domain need the user's or an existing policy's agreement. Post-install scripts are not executed: they are recorded so a user can see them |
| Local import is legal | A user may import a self-made package or point at an existing adapter process without a marketplace and without signing in. Binding trust to the specific content is enough |
| Isolation is stated honestly | Where the operating system provides file, network and process permissions, they are enforced. Where it does not, the client says it is in trusted local mode instead of implying a sandbox |
| Secrets are brokered | Extensions receive host handles; raw credentials are passed only to a trusted extension that has been granted that specifically |

## 11. The kernel and the first-party package rule

The installed client is a **kernel**, and the kernel is a complete product:

- canonical conversation — direct and group conversation, membership and Profile;
- the Assistant, the continuous collaboration loop;
- the workflow and flywheel;
- the extension host and package management;
- the base usage journal;
- the declarative interface primitives;
- the generic PTY/CLI adapter with Agent Hub configuration registration;
- key custody;
- update and migration admission.

Vendor adapters, Subagent MCP, the gateway and channels, endpoint collaboration,
analytics and data packages are **optional packages**: a user downloads what they
use and can uninstall it at any time. The kernel depends on no optional package,
and no optional package may force the kernel to load it. The per-capability
ownership table is published in
[DEPLOYMENT-PROFILES.md](DEPLOYMENT-PROFILES.md#3-per-capability-ownership).

Two consequences follow, and both are checked by contract tests rather than by
review:

- The Assistant and the workflow/flywheel stay in the kernel. They are not a
  profile choice, and no package absence removes them.
- Every first-party package carries a self-described compatibility list, and the
  kernel admits it only when that list covers the running client — exactly as
  section 9 describes for any other package. A first-party package gets no
  exemption from the rule, and no first-party package is admitted by version
  equality with the client.
- Every first-party package says what persisted data it owns. Its manifest either
  declares the native `conversion` for the published client-state formats it
  reads and produces, or carries its own namespaced `persistentData` answer with
  the `persistentDataReason` that justifies it: `none` when the package writes
  and keeps nothing, `self-owned` when it keeps data under its own directory
  whose format only that package reads and rewrites and which is not one of the
  formats a migration converts. A silent manifest is a gap rather than a complete
  declaration, and the release contract test reads every `crates/*/package` and
  `components/*/package` manifest to refuse one.

A package that converts a published client-state format declares how it does so,
and the contract publishes one kind: a native executable the package itself
carries (`ConverterKind::NativeExecutable`,
`crates/licoup-extension-contracts/src/manifest.rs`). The entry must be a program
inside the package payload, so a converter cannot borrow the client's own runtime
and make a migration depend on what happens to be installed instead of on the
package that declared the format. The declaration names the published source
formats it reads and the one it produces, each a format identity rather than a
client version; the two endpoints may not be the same format, the source list is
bounded, and every refusal names the rule and the field it broke
(`manifest_converter_not_native`, `manifest_converter_entry_outside_package`,
`manifest_conversion_incomplete`, `manifest_conversion_invalid`,
`manifest_conversion_endpoint_mismatch`).

Diagnostics a package writes stay local and bounded. `stdout` carries the program
protocol and nothing else, diagnostics go to `stderr`
(`crates/licoup-extension-contracts/src/transport.rs`), and a diagnostic line over
the published bound is truncated rather than allowed to stop the client from
responding. An instance identity token is not diagnostics: an identity prints its
label and never its token (`platform/extension_host/identity.rs`).

## 12. What this milestone implements

Stated plainly, because a contract document is not evidence:

| Part | State |
|:---|:---|
| Profile, transport, Agent, provider, usage, manifest, deployment and UI contracts | Implemented in `licoup-extension-contracts`, with the schemas under `schemas/extensions/` pinned to the crate by the agreement test |
| Kernel ownership table and the first-party package rule | Implemented in the contract crate: the Assistant and the workflow/flywheel are kernel capabilities, and the optional capabilities stay outside the kernel's dependency closure |
| Compatibility list | Implemented in the manifest contract and enforced by the package store at install and at activation |
| Data-package category and typed resources | Implemented in the manifest contract and admitted by the package store: a `data` runtime carries no program, and theme, layout, style, font, language and composition resources are typed declarations that name the host primitives and host actions the manifest declares. An executable declaration on a data package, a resource kind or shape this client does not publish, and a binding outside the declared set are refused with a stable reason before anything is published |
| Package lifecycle transactions | Implemented in `extension_packages`: offline local import, staged install with an install journal, crash recovery, the package and instance state machines, storage accounting, garbage collection and the uninstall transaction |
| Package program execution, generation management, the package center and first-launch recommendation | Implemented since: the isolation carrier starts an installed package's own declared entry (`platform/extension_host/runtime.rs`, `platform/extension_host/isolation/package_program.rs`), the host owns package generations and the maintenance admission (`platform/extension_packages`, `PackageGenerationAdmission` in `src/lib.rs`), the package center matches small declarative rules and records a recommendation without installing, downloading or starting anything (`platform/extension_packages/discovery.rs`), and the client surfaces are under `apps/desktop/lib/src/application/features/plugin_management/`. `tests/integration/extension_isolation/package_execution.rs` and `codex_package_turn.rs` execute the real carrier |

Nothing above says a package was downloaded, signed or published: the package
center reads cached catalogue metadata and the probe locations the user allowed,
and a package reaches the store through local import. The schemas and the crate
are the contract; the store enforces what section 9 and section 10 describe.

## 13. Not claimed here

This document does not install, download, execute or verify any package, and it
does not attest that any third-party extension exists or works. It publishes the
contract, the schemas and the rules that the host applies. Install sizes, artifact
inventories and per-platform evidence are separate, measured facts recorded in
release evidence.
