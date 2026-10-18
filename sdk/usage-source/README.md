# Usage source SDK (C11)

A usage source produces **observations**, not charges. This crate is the SDK for
the C11 `usage-metric` profile: it makes the three ways a source is collected
converge on one mapped value, and it refuses the things a payload may not decide
about itself.

| Module | What it owns |
|:---|:---|
| `collection` | `usage.publish` (bounded push batch) and `usage.query(cursor, limit, scopeRef)` (bounded pull and cursor resume), with the count, frame and cursor bounds |
| `binding` | The host-side binding: source identity, instance, generation, source epoch and authorized scopes. Every direction is mapped through it to one `BoundObservation` |
| `normalize` | Boundary adapters: an explicit field-mapped JSON record, a `key=value` log line, and the sum-and-gauge subset of an OpenTelemetry-shaped metric |
| `describe` | `usage.describe`: the fields and series a source declares, with unit, temporality and aggregation |
| `metrics` | The general metric ids of C11 and their subset relations (`licoup.tokens.total` contains `licoup.tokens.input`, which contains `licoup.tokens.cached-input`) |

## The refusals that matter

- A payload carrying `source`, `extensionId`, `instanceId`, a generation, a
  registry epoch, an authority field or a measurement identity is refused
  before it is parsed. The transport binds the source; the payload never names
  it.
- A payload whose `sourceEpoch` is not the bound epoch, or whose `scopeRef` is
  not a scope the host issued, is refused. Naming an old epoch would evade
  deduplication; naming another scope would attach an observation to work that
  is not the user's.
- A cursor belongs to one epoch. Resuming an old cursor is
  `usage_cursor_epoch_stale` with a reconcile recovery, not an empty page.
- A missing metric is absent, never zero. Tokens, cost and model are optional
  beyond the observation's identity.

## What it does not do

No ledger, no settlement, no storage, no network, no process supervision: an
observation is a claim about what a source measured, and where a claim becomes
durable is the host's decision. The kernel's base usage journal depends on this
crate downward for the shapes it admits, so that decision is structural there. A
delivery is keyed by the bound source, its epoch and the observation id; a higher
revision corrects it and a retraction withdraws only its own observation; and one
host-issued measurement settles once however many sources report it. Cumulative
differencing, correlation and presentation stay with the consumer — the optional
analytics package — which reads the core's facts through the port the kernel
composes.

## Tests

```bash
cargo test --locked --manifest-path sdk/usage-source/Cargo.toml
```

The crate is excluded from the repository root workspace on purpose: it is the
third-party boundary, so it keeps its own manifest and lock while the kernel takes
it as a path dependency. The optional analytics package depends on the same crate;
the kernel never depends on the package, and the repository root manifests stay
owned by the workspace owner.
