# Architecture and development graph (module M24)

Reads the LicoUp architecture inventory and the private execution inventory, checks them, resolves
source-file identity, and projects views. It is the single owner of relation meaning for contract
C08, so the Mermaid/SVG/HTML/JSON views are outputs that nobody edits by hand.

It is not a scheduler, an agent launcher, a plugin host, a gate platform, or an acceptance
authority. It never runs the regression commands it selects, never writes the development ledger,
and never marks work complete.

## Commands

```bash
node tools/architecture-graph/cli.mjs validate            # structure + semantics + relation checks
node tools/architecture-graph/cli.mjs resolve             # resolved typed graph with source identity
node tools/architecture-graph/cli.mjs relations           # classes, edges, what may schedule
node tools/architecture-graph/cli.mjs trace               # legacy task mapping + directed checks
node tools/architecture-graph/cli.mjs impact --contract C08
node tools/architecture-graph/cli.mjs impact --path crates/licoup-workflow/src/machine.rs
node tools/architecture-graph/cli.mjs select --changed-from HEAD~1
node tools/architecture-graph/cli.mjs export --out docs/plans/v7/graph/generated/work-items.json
node tools/architecture-graph/cli.mjs render              # Mermaid/SVG/offline HTML
node tools/architecture-graph/cli.mjs publish             # write docs/architecture/architecture-map.json
node tools/architecture-graph/cli.mjs publish --check      # fail if that file is stale or hand edited
node tools/architecture-graph/cli.mjs bind --evidence receipt.json [--claim claim.json]
```

Sources are located through `docs/plans/v7/graph/project.json` (override with `--project`), always as
repository-relative paths: no machine-local absolute path, environment variable or home directory is
consulted. A graph document without a sibling `*.schema.json` is an error rather than a silent skip.

## Relation classes

Only `development-order` may schedule work. The other classes explain, verify and trace.

| class | direction | role |
|---|---|---|
| `goal-reference` | consumer module → provider module (`depends_on`) | direction and impact scope; never a wait |
| `port-call` | caller → callee through a named contract (`runtime_calls`) | assembly feedback; loops are allowed and do not enter the DAG |
| `development-order` | prerequisite task → dependent task (`precedes`) | **the only class in the development DAG**, lowered from `task.depends_on` |
| `impact-traceability` | task → module/contract/scenario/write scope, milestone → task | selective invalidation and traceability |
| `deployment-delivery` | package → required package, package → module/task, profile → package, source → package | install closure and delivery responsibility only; never a wait |

The deployment family is the fourth class required by C08. `requires_package` decides the install
closure, `task.packages` records delivery responsibility, and a package may not link statically to a
module outside its own closure. None of it becomes a task prerequisite.

## Checking

Structural checking uses the repository's existing JSON Schema library (`ajv` + `ajv-formats`),
already a devDependency. There is deliberately no second schema engine: shape comes from the
`*.schema.json` documents, while ID resolution, reference direction, closure containment, DAG
acyclicity and evidence-level rules are semantic checks in `lib/graph-model.mjs`. Neither replaces
the other.

## Identity and claim binding

Graph digest and per-task fingerprints are byte-identical to the existing Python plan tool's, so a
claim made against one tool names the same graph in the other; `test/graph-model.test.mjs` asserts
that parity. `bind` compares a receipt against the resolved graph and rejects a stale
`task_digest`/`graph_digest`, a moved contract revision, an unpermitted evidence level, an
assertion-free check, missing required receipt fields, malformed binding identity, or a claim record
from another generation. It reports consistency only: it
does not authenticate a log, prove test semantics, accept work or mutate any ledger, and the
existing ledger stays the single owner of claims and status.

## Regression selection

`select` and the regression block of every work item call the repository's existing
`tools/regression/client-module-selection.mjs`. A changed path with no observing catalog entry is
reported as a gap instead of being dropped, and no aggregate gate is added or invoked here. The
public catalog intentionally watches only public paths: a local `docs/plans/` edit is reported
through this tool's own impact output rather than through a public regression module.

## Public projection

`publish` writes `docs/architecture/architecture-map.json`, a projection built from an explicit
allow-list of architecture fields: no task ids, task prose, scenarios, write scopes or ledger
digests can reach a public directory. The payload is scanned for absolute paths, home aliases,
account names, hostnames, credentials and private identifiers before writing; findings are reported
by rule name only, so a failure message cannot itself leak the value. `publish --check` fails when
the published file differs from a fresh generation, which is how a hand edit is detected.

## Tests

```bash
node --test tools/architecture-graph/test/
```

The suite covers structure and semantic rejection (illegal edge, cycle, duplicate ID, unsafe path,
unpermitted level), the four relation classes, install-closure containment, source identity
resolution, impact, catalog selection, receipt binding and tampering, legacy mapping, projection
allow-listing, offline rendering and byte-reproducible generation.

Behaviour tests use in-memory graph fixtures and run in any checkout. Comparisons that need the
local plan under `docs/plans/v7` — schema conformance of the real documents, Python identity parity,
the published projection, and the repository graph's own relation checks — are separate optional
local checks that report as skipped when that plan is absent. They are not part of the public
regression path.

The identity-parity assertion executes the native reference tool, which declares "Python 3.10+
standard library only". The harness therefore resolves a supported interpreter (`python3` first,
then versioned names, ignoring anything older than 3.10) and skips with an explicit reason when the
machine has none, rather than reporting a false failure from an interpreter that cannot parse the
reference source.
