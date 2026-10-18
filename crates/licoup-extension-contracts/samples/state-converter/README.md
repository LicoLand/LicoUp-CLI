# Example state converter (`org.licoland.example.state-converter`)

The smallest complete **converter package** an extension developer can copy: one
manifest that declares the persisted format it owns, and one native program,
inside the package payload, that performs the move.

It is a developer example, not a published package. It is not listed in
`tools/client-release-package-set.json`, it is not signed, and nothing in the
client imports it at startup. It exists so that "write a converter" has an
answer that is executed by the repository's own checks rather than described.

The sibling [`echo-agent`](../echo-agent/manifest.json) sample covers the other
half of the platform: a package that serves the `agent-execution` profile. This
one serves no published profile at all. A converter is a maintenance program the
host runs over a data root; it is never started as an agent, and it declares
`native-converter` — a profile id this client does not publish, which the
manifest contract preserves and refuses for nothing.

## What a converter package is

| File | What it is |
|:---|:---|
| [`manifest.json`](manifest.json) | The package manifest, in the published `licoup.extension-package.v1` shape, whose `conversion` block is the package's own claim about the format it owns |
| [`package-release.json`](package-release.json) | The release declaration the release index republishes for this package, carrying the same pair in the index's own vocabulary |
| [`converter/main.rs`](converter/main.rs) | The whole source of the `native-executable` entry the manifest names |

The two declarations are separate facts about the same program and both are
checked: the manifest is what the host reads before it starts anything, and the
release declaration is what a signed index binds to a payload digest. A package
whose release declaration names a pair its manifest does not is refused by the
index reader.

## Building the entry

A payload carries a program, not a build system. The entry is compiled by the
verification entry that imports this sample, from the source in this directory
and with nothing but the Rust toolchain the repository already requires:

```sh
rustc --edition=2021 -C debuginfo=0 -o <payload>/bin/licoup-sample-state-converter converter/main.rs
```

No interpreter, no `runtimeRef`, no workspace member and no dependency: the
sample is standard-library Rust, so the same file compiles on every platform the
client builds on.

## The payload, and where it is imported

A payload is a ZIP archive. Two of its entries are the contract:

```text
manifest.json                          the manifest above, at the archive root
bin/licoup-sample-state-converter      the compiled entry, at runtime.entry
```

The import path is the host's own, and it is the same one a package downloaded
from an index takes after the download:

```rust
PackageStore::open(store_root)?.install_local_import(
    "org.licoland.example.state-converter",
    "1.0.0",
    TrustRecord::local_approved(content_digest(&payload), Vec::new())?,
    &payload,
)
```

`crates/licoup-migrate` reaches the same path when a caller names an offline
payload, so the two ways in agree by construction rather than by convention.

Before the version is published, the import refuses a payload that:

| Refusal | What it read |
|:---|:---|
| `package_artifact_invalid` | The archive is not a readable ZIP, or the manifest inside it is not JSON |
| `package_manifest_missing` | No `manifest.json` at the archive root |
| `package_artifact_limit_exceeded` | An entry, the entry count or the expansion is over the published bound |
| `package_manifest_invalid` | The manifest does not parse as `licoup.extension-package.v1` — including a `conversion` block that is not a `native-executable` entry inside the payload |
| `package_manifest_mismatch` | The manifest's `id`/`version` are not the ones the caller is installing |
| `package_entry_missing` | `runtime.entry` names a file the payload does not carry |
| `package_entry_unsafe` | `runtime.entry` is absolute, climbs out of the payload, or carries a platform separator |
| `package_trust_not_bound_to_content` | The trust record is not bound to the digest of these bytes |
| `package_client_incompatible` | `compatibility.clientVersions` does not cover the running client |

The three checks above are the ones a package author gets wrong, and each one
names the field it read: see
[`artifact.rs`](../../../../crates/licoup-native/src/platform/extension_packages/artifact.rs)
and
[`install.rs`](../../../../crates/licoup-native/src/platform/extension_packages/install.rs).

## The stop contract

A conversion runs over a data root that other owners write to. Two people have to
have stopped writing before this program is allowed to start:

- **The operator.** `licoup-migrate package-convert --writers-stopped` is the
  statement that every writer is stopped against that data root. It is a
  parameter, not a gate the tool can check for the caller: the tool's own lock
  cannot stop a writer that never heard of it. Without the statement the caller
  is refused with `writers_running` before the data root or the store is opened,
  and no staging directory, journal or target is created.
- **The caller.** Once the converter is running, the caller may decide it no
  longer wants the run. It answers a stop probe, the tool signals the converter's
  whole process group, gives it a bounded grace window, and then kills it.

The instant stop is answered, the run stops being a conversion. The caller
receives a `ConverterOutcome` with `stopped: true`, `complete: false` and
`reason: package_conversion_stopped`; a converter that was stopped reports no
result document, and a missing result document is never read as completion. A
stop is not a refusal: the run is recorded as unfinished and stays resumable.
This sample makes the window observable rather than racy — a first attempt holds
before it writes its result for as long as
`client-state/sample-conversion-hold-millis.txt` states in the source root, and
that control document is sample scope, not part of the contract. A run that was
asked to continue does not hold again.

## The update contract

A run is **continued, never restarted**. That is the whole of the update story: a
client update replaces the installed bytes, and a conversion that was interrupted
half-way must continue the target it already produced rather than begin again
over it.

| Fact | Who decides it |
|:---|:---|
| The run is resumable at all | The caller: an unsettled working root is resumed with `package-resume`, under the same declared package, pair, payload digest and source digest |
| A plain `package-convert` will not start a second run over an unsettled one | The caller: it is refused with `package_conversion_unfinished` until the run is explicitly resumed |
| The converter continues instead of clearing | The package: the tool passes `--resume`, and the entry appends to the target it already has |

The entry is told `--resume` and nothing else; the tool never deletes the target.
A converter that clears it on a resumed run has destroyed the work the resume was
for.

## The receipt

The exit status is not the outcome. The outcome is one document the converter
writes at `--result`:

```json
{
  "schema": "licoup.package-conversion-result.v1",
  "sourceFormat": "licoup-state-0.1.1",
  "targetFormat": "licoup-state-0.3.0",
  "complete": true,
  "convertedRecords": 2
}
```

A run counts as complete only when the process exited zero **and** this document
names the schema, names the required pair, says `complete`, and left a non-empty
target. Anything else — a non-zero exit, no document, another pair, `complete:
false` — leaves the run unfinished and the caller's exit status non-zero.

## The invocation, in full

```text
<entry> --source <dir> --target <dir> \
        --source-format <identity> --target-format <identity> \
        --result <file> [--resume]
```

`--source` is the caller's staged copy of the data root, and the entry reads it
and must not write inside it: the caller digests the data root after the run and
refuses a converter that touched it. `--target` is both the output root and the
process's working directory. The environment is reduced to the few variables a
native program needs to start, so a run is reproducible from its arguments
alone.

## What is checked, and by which entry

```sh
cargo test --manifest-path crates/licoup-extension-contracts/Cargo.toml
node tools/scripts/migration-crate-tests.mjs
node --test crates/licoup-extension-contracts/tests/ui_schema.test.mjs
```

The middle entry is the migration crate's own runner, and it is the one that
matters here: this package declares the client line it supports
(`>=0.3.0, <1.0.0`), the store refuses a package whose list does not cover the
client it is installed on, and the runner supplies the planned candidate
identity through `LICO_CLIENT_PRODUCT_VERSION`. Running the suite with a
development identity instead reports `0.0.1-alpha` and the import is refused
with `package_client_incompatible` — correctly, and that refusal is the
compatibility rule doing its job rather than a defect in the sample.

- [`tests/state_converter_sample.rs`](../../tests/state_converter_sample.rs)
  reads this directory's manifest and release declaration and holds them to the
  contract: the declaration parses, the entry is inside the payload, the required
  pair is one the declaration reads and produces, and the two documents agree.
- `crates/licoup-migrate/tests/state_converter_sample.rs` compiles
  [`converter/main.rs`](converter/main.rs), assembles the payload above, imports
  it through `PackageStore::install_local_import` and through the coordinator's
  own offline-payload path, selects it by declaration, and drives the stop,
  resume and receipt contract over it.
- `tests/ui_schema.test.mjs` validates the committed manifest against the
  published JSON Schema, and refuses the same document once a required field is
  taken out of the shape the schema publishes.
