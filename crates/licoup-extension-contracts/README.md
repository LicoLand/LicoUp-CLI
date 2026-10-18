# licoup-extension-contracts

The extension SDK contract: the five narrow profiles an extension can serve, the
carrier that frames them, and the package facts that decide whether a capability
can be served at all.

This crate holds **rules and catalogs**, not a runtime. It owns no registry, no
scheduler, no process supervision and no authority; the host owns all of that, and
identity and version negotiation belong to `licoup-application::extension`, which
this crate builds on rather than duplicating.

| Module | Contract | What it publishes |
|:---|:---|:---|
| `profile` | C09 | The five profile ids, their required and optional method catalogs, method coverage per profile |
| `transport` | C09 | Carriers, the negotiated frame bound, the control reservation and blob handles |
| `agent` | C09 | Event kinds, the admission receipt, cancel outcomes, describe and resume shapes |
| `provider` | C10 | Provider configuration, catalog keys, credential handles and their endpoint scope |
| `usage` | C11 | Observations, metric semantics, exact decimals and cumulative series rules |
| `manifest` | C09, C12 | The package manifest: identity, category, profiles, dependencies, permissions and the typed data resources a data package contributes |
| `deployment` | C12 | Install closure, package facts, capability availability and per-capability ownership |
| `ui` | C13 | Contribution kinds, host primitives, mount decisions and generation checks |

The JSON Schema form of every shape is published under
[`schemas/extensions/`](../../schemas/extensions), and `tests/schema_agreement.rs`
pins the two together: an edit to one without the other fails the test rather than
silently shipping two contracts.

## Building this crate

This crate is a member of the repository workspace, so it builds with the same
toolchain, lockfile and target directory lifecycle as every other crate. From the
repository root:

```
node tools/scripts/cargo-client.mjs test -p licoup-extension-contracts
```

That wrapper holds the shared test-artifact lease, so it does not race another
crate's build inside the same target directory; plain
`cargo test -p licoup-extension-contracts` works as well. A `Cargo.lock` inside
this directory is never read — the workspace root lockfile is authoritative.
Package metadata uses workspace inheritance, so no local `[workspace]` table and
no version literals belong here.

## The samples

`samples/echo-agent/` is the smallest complete extension: a Python program that
implements exactly the six methods `agent-execution` requires, asks for no
permission, needs no registry, and is installed from a directory the user already
has. It opens no socket, reads no file and calls no model. Its recorded session is
checked by `tests/minimal_agent_sample.rs`, so the sample cannot drift from the
contract it demonstrates.

`samples/state-converter/` is the smallest complete converter package: a manifest
whose `conversion` block declares the persisted formats it reads, the one it
produces and the entry inside its own payload that performs the move. The entry
is native — the payload carries the program and borrows no interpreter — so the
example is compiled by the checks that import it rather than committed as a
binary. `tests/state_converter_sample.rs` holds the two declarations to the
contract and to the published JSON Schema, and
`crates/licoup-migrate/tests/state_converter_sample.rs` builds the entry,
assembles the payload, imports it through the host's own path and drives the
stop-and-resume contract over it. See
[its README](samples/state-converter/README.md) for the payload layout, the
import path and the refusals an import can produce.
