# Analytics and usage sources (`org.licoland.feature.analytics`)

The optional M29 package: statistics sources, the query index, the dashboard
definitions and specialist metrics. It provides the `analytics.v1` capability
and is not part of the core; a client without it has no index, no panel registry
and no scraper, because there is nothing to construct.

| Module | What it owns |
|:---|:---|
| `facts` | The narrow port over the core's retained metering facts (the M33 slice): record, retract, read, pending. Borrowed, never owned |
| `policy` | The host's source grants: who is admitted, and who may settle. A producer's reported quality never appears here |
| `index` | Observation identity, revision supersession, retraction tombstones and cumulative series differencing |
| `correlation` | One call reported by several sources: settle once, or report `Ambiguous` |
| `metrics` | The professional metric catalog: declared unit, temporality and aggregation, exact addition, subset and unit refusals |
| `panels` | Declarative `metric-panel` contributions and their prepared values |
| `package` | Admission, reconciliation, panel preparation, activation, drain and uninstall |

## The three separations

- **The trusted account is the core's.** This package records eligible facts
  through `CoreUsageFacts` and never keeps a second ledger. Uninstalling it
  releases its panels, prepared values, source bindings and scrapers; the facts
  are exactly where they were, and a reinstall reads them without duplicating.
- **A producer's quality is not authority.** `AuthorityPolicy` is a host grant
  table. A source that reports `reported` gains nothing by saying so; an
  admitted source without the settlement grant is displayed with its provenance
  and can never change a budget.
- **Display is prepared data.** A panel declares namespaced series and receives
  prepared values. An unreported metric is `unknown` with no number, an
  ambiguous measurement is not drawn as a definite amount, and a redraw reads
  prepared values instead of re-scanning a source.

## One call, several reports

An Agent adapter, a model gateway and the execution graph may all report one
invocation. Reports that share a host-issued measurement settle once; when they
do not, an explicit host priority decides; when neither exists the outcome is
`Ambiguous` and nothing is summed. A conflicting report never erases a charge
that was already settled — only a retraction withdraws its own observation.

## Activation and uninstall

- `AnalyticsPackage::activate` refuses when the capability is not installed or
  not enabled, using the contract's unavailable-capability vocabulary with an
  actionable recovery.
- `begin_uninstall` stops new admission and reads and releases panels, prepared
  values and scrapers. `complete_uninstall` drops the index and the bindings
  once in-flight reads have drained, and reports exactly what was released and
  that the facts were preserved.

## Tests

```bash
cargo test --locked --manifest-path components/analytics/Cargo.toml
```

That runs the package's unit tests and the V7-U6 component-integration suite in
`tests/integration/usage_sources/` (A34 and the component-level part of
A31). Everything is synthetic; no account, ledger, usage file or network is
involved.

## Wiring this package needs from its owners

- **M33 / M07**: an adapter implementing `CoreUsageFacts` over the existing
  usage store, and the host composition that lends it to this package. The port
  is defined here because this package is its consumer; the ledger stays with
  its current owner.
- **Packaging (U10/U11/U12)**: the deployment carrier and manifest for
  `org.licoland.feature.analytics`. The distribution graph already declares the
  package, its `requires_package` and its `analytics.v1` capability; no manifest
  is invented here because the carrier shape belongs to packaging.
- **Regression catalog**: the exact commands above, registered by the
  coordinator in the existing catalog.
