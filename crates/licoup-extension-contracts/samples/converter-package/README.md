# Sample: a package that owns one format conversion

This directory is the second contract sample, next to
[`../echo-agent/`](../echo-agent). Where that one is the smallest complete
`agent-execution` extension, this one is the smallest complete **converter**
package: a manifest whose `conversion` object names a program the package itself
carries, the published source formats that program reads and the one format it
produces.

Nothing here is started by the sample tests. The manifest is read, validated
against `crates/licoup-extension-contracts/src/manifest.rs`, and checked against
the refused mutations in `tests/minimal_agent_sample.rs`, so a package author can
see the accepted shape and every rule it can break without installing an Agent, a
Node.js runtime, a Python runtime or a developer toolchain on the target device.

## What the manifest declares

| Field | Value here | What it means |
|:---|:---|:---|
| `profiles` | one `native-converter` profile, major 1 | The package serves no method profile. An id this host does not publish is preserved and acted on by nothing, so a converter profile is a declaration and not a call |
| `runtime` | `process`, entry `bin/example-converter` | The host runs a program. The entry is inside the package payload and is started directly; it is not named through an interpreter, so a converter cannot borrow a runtime the package does not carry |
| `conversion.kind` | `native-executable` | The only converter kind the contract publishes |
| `conversion.entry` | `bin/example-converter` | The program that performs the conversion, relative to the package root |
| `conversion.sourceFormats` | `licoup-state-0.1.1` | The published format identities this converter reads. They are identities, never client versions, and at most eight of them |
| `conversion.targetFormat` | `licoup-state-0.3.0` | The one published format identity it produces. It may not be one of its own sources |
| `compatibility.clientVersions` | `>=0.3.0, <1.0.0` | Which client builds may load the package. A package that declares none is admitted by nothing |
| `activation` | `on-demand` | The host starts the program when a conversion needs it, not at install |

The entry is a path with at least one directory component. `manifest.json` at the
archive root, the entry file and the optional `contributions/` documents are the
whole payload: the store reports any install script it finds (`install.sh`,
`postinstall.sh`, `setup.py` and the rest of the published list in
`platform/extension_packages/artifact.rs`) as a fact about the archive, never puts
one on an install path and never executes one.

## How it reaches the host

A package is a ZIP archive whose root carries `manifest.json`. The store expands
it in a private staging directory, reads the manifest, checks that the declared
entry is a real file inside the payload, and installs it through
`PackageStore::install_local_import(package_id, version, trust, bytes)` — the
local-import path in `crates/licoup-native/src/platform/extension_packages/`. No
network, no marketplace and no signature is involved: the trust record binds the
specific bytes the user supplied.

After install, the conversion this package owns is answered from the manifest:
`PackageManifest::conversion_owner` returns this package for the declared
`licoup-state-0.1.1` → `licoup-state-0.3.0` pair, and refuses a required pair
this package does not declare with `manifest_conversion_endpoint_mismatch`. A
package that declares no conversion at all is refused only when a conversion was
*required* of it (`manifest_conversion_missing`).

## The stop contract a converter must respect

A converter is a package program like any other, so the host's stop rules apply
to it unchanged:

- `agent.cancel` is an optional method. A package that implements no cancel is
  complete, and the host reports `unsupported` rather than pretending the work
  stopped (`CancelOutcome` in `crates/licoup-extension-contracts/src/agent.rs`).
- The four cancel outcomes stay distinct: `requested` (the request left, no answer
  yet), `acknowledged` (the extension confirmed it stopped its own work),
  `unsupported` and `unknown`. Only `acknowledged` means the work demonstrably
  stopped, and no outcome settles an external effect.
- Manual stop resolves the durable owner of the admitted work and routes the
  request to that owner; force stop may terminate only a LicoUp-owned process
  group whose ownership record is re-verified at execution time
  (`crates/licoup-native/src/platform/stop_control.rs`). A request is not proof of
  exit, and an unconfirmed stop stays unconfirmed.

## The update contract a converter must respect

Installing, replacing or activating a version changes state that running work may
be reading, so both mutating operations pass one idle-guard seam
(`platform/extension_packages/maintenance.rs`):

- The verdict that no local work is in flight is read as data for the data root
  the operation would change. A verdict nobody read is a refusal, never an assumed
  idle host.
- Read-only work — checking the catalogue, comparing versions — is not gated.
- No package program, converter included, is asked whether it *wants* to be
  replaced: the host decides admission, and the package is never the authority on
  whether its own replacement is safe.

`crates/licoup-native/tests/extension_contract/a30_generation.rs` and the
`maintenance.rs` unit tests prove the admission and generation rules against
synthetic packages, without installing a real conversion.
