# ADR 0006: Capability-aware parallel client regression

- `context` — The client regression catalog currently expands 782 leaf
  modules into a synchronous `spawnSync` loop and returns after the first
  failed command. This serializes independent frontend, backend, platform,
  and Agent checks, repeats toolchain startup and test-target discovery, hides
  later failures, and makes a correction rerun repeat unrelated successes.
  The existing report has no elapsed-time field. The only reliable legacy
  timing observation is an interrupted run that had reached 1,161,000 ms and
  was still incomplete; it is a lower bound, not a completed baseline.
- `decision` —
  - The regression is a staged dependency graph: incremental common
    preflight; shared foundation; parallel frontend and backend collections;
    interface integration; core scenarios; then one bounded compatibility
    frontier containing every locally eligible platform and Agent lane.
  - Independent work settles even after sibling failures. A failure blocks
    only success-dependent descendants. Reports aggregate every failure, and
    a report-derived retry selects failed or attribution-pending members plus
    newly eligible descendants without rerunning unrelated successes.
  - macOS, Android, Windows, Linux, and iOS each own a dedicated entry and run
    an incremental capability probe immediately before live work. Every Agent
    in the canonical driver inventory likewise owns a dedicated entry. Static
    validation is split into one shared inventory/schema contract and one
    independently scheduled contract per Agent. A failing Agent contract
    blocks only that Agent's live verifier. Static contracts may pass without
    promoting compatibility; a missing local SDK, device, host, or Agent
    runtime is `unverified` with a stable reason, never passed and never a core
    regression failure.
  - The outer scheduler uses asynchronous static-argument process spawning,
    `shell: false`, one global capacity derived from available parallelism,
    and separate Rust, Node, Flutter, and Gradle resource pools. Commands whose
    toolchain already uses internal parallelism consume higher resource
    weights so multiple Cargo, libtest, Node Test Runner, Flutter, or Gradle
    process trees do not oversubscribe the host. The shared Cargo target and
    Flutter cache are single-owner resources. Hybrid wrappers must claim every
    toolchain they launch; for example, Android native verification claims
    Cargo, Flutter, and Gradle resources rather than presenting itself as a
    pure Node command.
  - Compatible complete selections are batched before scheduling. Rust leaves
    sharing the same manifest, package, features, target kind/name,
    environment, and resources become one target-level `cargo test` invocation
    only when the complete registered target group is selected; libtest then
    owns test-level parallelism. Compatible Node test files become one
    `node --test` invocation with bounded file concurrency. Flutter paths and
    Gradle tasks merge only when project, setup, options, environment, and
    resource claims are equal. Focused or incompatible selections preserve
    their exact commands.
  - A merged Node Test Runner invocation uses a repository-owned reporter that
    emits only its input count and failed numeric input indexes. The parent
    maps those indexes back to catalog module IDs in memory, so a retry selects
    only failing members without persisting test paths, names, stacks, or raw
    output. An invocation that cannot produce a complete attribution receipt
    remains `attribution-pending` instead of guessing.
  - Each invocation records monotonic wall time. A process-tree metrics adapter
    records direct-process CPU, descendant CPU, and peak resident memory only
    where the host can measure them accurately; otherwise each metric is
    `unavailable` with a stable reason. Reports persist only allowlisted IDs,
    statuses, counts, timings, numeric resource measurements, concurrency
    peaks, failure codes, and compatibility rows. They never persist command
    output, arguments, environment, paths, PIDs, machine or user identity,
    credentials, devices, endpoints, prompts, or runtime payloads.
  - Toolchain-native statistics are collected independently of OS process
    metrics. Rust commands request stable Cargo timings and retain only stable
    anonymous libtest terminal aggregates; Cargo's HTML timing artifact remains
    human-readable and is never parsed as a machine contract. Flutter tests use
    the package:test JSON reporter with explicit bounded concurrency and reduce
    structured events to anonymous suite/test counts and durations. Native
    statistics never promote unsupported CPU or RSS fields to measured.
- `rationale` — Cargo compilation already defaults to logical-CPU parallelism,
  libtest runs tests within one test executable on multiple threads, and the
  Node Test Runner can run isolated test files in bounded parallel child
  processes. Reducing repeated Cargo/Node/toolchain invocations before adding
  outer concurrency therefore removes more overhead and avoids nested
  oversubscription. Reusing the existing module catalog, selection logic,
  toolchain runner, managed Cargo target and lease, Agent inventory, and live
  verifier keeps the cutover small and preserves established authorities.
- `alternatives` —
  - Only replace `spawnSync` with asynchronous `spawn`: rejected because it
    leaves hundreds of repeated Cargo, target-discovery, libtest-binary, and
    Node startup costs and can oversubscribe internally parallel tools.
  - Run all catalog leaves concurrently: rejected because 782 processes would
    compete for CPU, memory, Cargo artifacts, Flutter/Gradle caches, devices,
    and Agent runtimes.
  - Keep one cross-platform environment script: rejected because it delays
    unrelated work and conflates a missing optional capability with a core
    failure.
  - Maintain a serial compatibility mode: rejected because the migration is a
    complete cutover and a fallback would preserve the failed architecture.
- `consequences` — The catalog and entry registry gain explicit stage, lane,
  environment, resource, toolchain, batching, and internal-parallelism facts.
  Full regression becomes bounded and non-fail-fast; focused selection remains
  exact; retries become narrow. Compatibility matrices explicitly distinguish
  executed evidence from local unavailability. Reports can identify real
  bottlenecks using scheduler wall time plus Rust/Flutter native statistics,
  and compare a completed new run only against the legacy
  incomplete lower bound; no speedup percentage is valid for that baseline.
- `status` — implemented, 2026-08-23. Focused scheduler, batching,
  compatibility-isolation, native-statistics, attribution, and report-privacy
  contracts are green. The first complete terminal report also demonstrated
  non-fail-fast aggregation; product and locally eligible live-target failures
  remain ordinary reported evidence rather than scheduler exceptions.
- `evidence` — repository scheduler, batching, native-statistics, retry, and
  report-schema contracts listed in the implementation section above.
