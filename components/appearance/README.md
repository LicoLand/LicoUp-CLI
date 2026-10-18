# Appearance resource converter (`org.licoland.converter.appearance`)

The native owner of the appearance slice of the persisted client-state format. It
replaces the retired appearance JavaScript codec: the package declares the formats it
converts, and it carries the Rust program that runs the conversion. No Node, Python or
other interpreter lane remains, and nothing here needs a runtime the package does not
carry.

| Module | What it owns |
|:---|:---|
| `declaration` | The published conversion declaration: the source/target pair read from the client's own embedded catalogue, and the manifest document the packaging carrier publishes |
| `converter` | The driver: read the client's projection, ask the client's admission to convert the root, report the appearance domain's outcome as one JSON report with a stable code and an exit code |

## One coordinator, one writer

The conversion is **not** implemented here. The client's migration owner is the single
implementation of every domain move, and this package drives it:

1. the client's read-only projection answers for `appearance-presentation` — the store at
   `client-state/appearance-preferences.json` — and an unsupported shape is refused there,
   before anything is opened for writing;
2. the client's own admission converts the root;
3. the appearance domain's outcome is reported: `converted`, `already-current`, or
   `refused` with the client's stable code.

A second implementation in this package would be a second writer, would duplicate the
ledger and marker discipline, and would drift from the client on the first schema change.

## Declared formats

The pair is read from `licoup_native::domain::client_state_migration::conversion_endpoints`,
the client's own projection of its embedded frontier. The committed
[`package/manifest.json`](package/manifest.json) is that declaration as the carrier
publishes it, and the suite fails when it goes stale:

```bash
cargo run --manifest-path components/appearance/Cargo.toml -- --manifest > components/appearance/package/manifest.json
```

A package that declares no conversion, or a pair other than the required one, is refused
by the standalone tool with the contract's own code; identification is a read of this
document, never a table of package names or format aliases.

## The entry

`licoup-appearance-convert` is the `native-executable` entry the manifest names at
`bin/licoup-appearance-convert` inside the package payload.

```bash
licoup-appearance-convert --data-root <path>   # one JSON report; 0 settled, 3 refused, 4 unavailable, 2 usage
licoup-appearance-convert --manifest           # the published manifest document
```

The caller is the migration coordinator: it has already stopped the writers and holds the
admission guarantee, so the entry adds no second lock and no second journal. The entry
never truncates or rewrites a source document itself; a refused run leaves the root
byte-identical, and an interrupted run resumes against the store that committed.

## Tests

```bash
cargo test --locked --offline --manifest-path components/appearance/Cargo.toml
```

`tests/conversion.rs` drives the real binary over synthetic roots — a supported source
that converts and preserves its user content, an interrupted conversion whose resume
applies the move exactly once, an unsupported store shape refused with a stable code
before any write, and a root whose declared format is not a published endpoint. The
standalone tool's suite (`crates/licoup-migrate/tests/resource_converter.rs`) holds the
other half: the committed manifest is identified for the pair the catalogue freezes, and a
synthetic source converts through the tool's own conversion entry.

## Wiring this package needs from its owners

- **Packaging**: the carrier that lays this manifest and the built
  `licoup-appearance-convert` binary into one package payload at the declared entry now
  exists. The package is declared in `tools/client-release-package-set.json` under the
  `appearance-converter-package-payload` role, its `package/package-release.json` is the
  release declaration the index republishes, and `bin/licoup-appearance-convert` is the
  staged entry the release stage replaces with the compiled program. The manifest is
  generated from this crate, so the carrier transcribes no format name.
- **Migration coordination (PACKAGE-MIGRATION-COORDINATOR)**: acquisition, verification and
  execution of a package converter entry. This package declares the seam and provides the
  program; it does not acquire, verify or run itself, and it holds no second coordinator.
