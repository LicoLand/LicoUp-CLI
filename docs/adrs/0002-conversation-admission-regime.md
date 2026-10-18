# ADR 0002: Conversation admission regime for running Agent dialogs

> **Governing principle: leverage the Agent framework's native capabilities
> to the maximum, and have LicoUp continue onto existing dialogs.**
>
> The client must always prefer the native, framework-provided way of
> attaching to a conversation that already exists — including a conversation
> another process is running — over launching a second process of its own.
> Every Agent section below states what that framework provides natively and
> how LicoUp uses it to continue onto existing dialogs. Where the framework
> does not natively resolve the concurrency of multiple joiners on one
> dialog, LicoUp waits for the running dialog to finish and then continues it
> with the framework's native resume mechanism.

- `context` — LicoUp observes and joins Agent conversations through official
  lanes. Joining a conversation that another process is actively running has
  real concurrency hazards when the lane is a per-process stdio transport:
  two processes would share one thread/session store, the lane reports
  another process's threads as `notLoaded` (unable to confirm whether a turn
  is still running), steer only reaches turns owned by the local process, and
  browse reads of live rollout files can observe half-written state. Some
  Agent frameworks natively expose a single long-lived service (loopback
  HTTP serve, vendor gateway) that multiple clients attach to; for those, the
  "second process" hazard does not exist because the conversation state lives
  inside one service instance.
- `decision` — Default admission regime for every Agent:

  1. When a conversation is observed, LicoUp first detects whether that
     conversation is currently running (an active turn or active runtime
     task).
  2. Running conversations are reflected in the conversation list with their
     running state.
  3. For a running conversation, the composer enters a loading mode: it only
     displays the ongoing dialog and does not accept input; interrupting the
     running turn (steer, cancel, or sending a new message) is not supported.
  4. Only after the running conversation finishes can LicoUp continue it,
     always through the framework's native resume capability described in
     the Agent's section below.
  5. Stopped conversations can be resumed directly by launching the long-lived
     process.

  Per-Agent continuation mechanics are defined in the sections below. The
  governing principle applies to all of them: use the framework's native
  capability first, and never invent a second-process scheme where the
  framework already provides a continuation mechanism.

- `rationale` —
  - Joining a live turn from a second process is unverified for per-process
    stdio lanes and can corrupt shared state or produce two active turns on
    one thread.
  - A state blind spot (the lane cannot confirm whether the other process is
    still running) makes optimistic joining unsafe.
  - Steer/cancel reach only turns owned by the local process; admitting input
    on a foreign running turn would promise control that cannot be delivered.
  - The governing principle keeps the safe default while using every native
    attach and resume mechanism the frameworks provide.
- `alternatives` —
  - Join every conversation optimistically and let the Agent backend resolve
    conflicts: rejected because the shared store and `notLoaded` blind spot
    give no conflict guarantee.
  - Reject all running conversations entirely: rejected because it discards
    the framework-native attach capability that several lanes already have.
  - Treat every lane as concurrency-capable until proven otherwise: rejected
    because it inverts the risk direction.

## Agent sections

### OpenCode

- Native capability: `opencode serve` exposes a loopback HTTP/SSE service
  (`serve-http` lane, `sessionScope: persistent`, contract fixture
  `opencode.json`). The serve instance owns the conversation state, and
  multiple clients attach to the same instance.
- Continuation: attach to the already-running serve instance instead of
  starting a second one. `local_service/serve.rs` probes the default port
  first and reuses a healthy service (`persist_reused_endpoint`, `adopted:
  true` when healthy but not LicoUp-started); a new process is launched only
  when no healthy service exists. Continue onto an existing dialog by
  resuming its native session id on the attached instance. While a turn is
  actively running inside that instance, the composer loading mode applies;
  the service itself resolves the concurrency of multiple clients.

### Kilo Code

- Native capability: `kilo serve` exposes the same loopback HTTP/SSE service
  shape (`serve-http` lane, `sessionScope: persistent`, contract fixture
  `kilo-code.json`).
- Continuation: identical to OpenCode — attach to the existing healthy serve
  instance and resume the native session id; composer loading mode while a
  turn is running inside the instance.

### OpenClaw

- Native capability: the vendor Gateway runs as a long-lived service on its
  official port; the lane states "Prefer vendor Gateway attach/reuse; never
  steal that port" (`conversation_lane.rs`), and `session/load` resumes a
  session with a caller-supplied native id (contract fixture `openclaw.json`,
  supervised as `attached-service`).
- Continuation: attach to and reuse the vendor Gateway instance that is
  already running on its official port, then continue onto the existing
  dialog with `session/load`. Composer loading mode still applies while the
  Gateway reports an active turn.

### Codex

- Native capability: the official stdio app-server (`codex app-server
  --stdio`) owns thread continuity through `thread.id`; `thread/resume`
  continues a thread with a caller-supplied thread id (contract fixture
  `codex.json`; `protocol/session.rs`). Thread state is shared on disk
  (`~/.codex/session_index.jsonl`, `~/.codex/sessions/`).
- Continuation: run the official stdio app-server, and continue onto an
  existing dialog with `thread/resume` using the observed thread id — after
  the dialog is no longer running. The lane cannot confirm whether another
  instance is still running a thread (`notLoaded`, `codex_runtime_observation.rs`)
  and steer only reaches turns owned by the local process
  (`active_control.rs`), so the running dialog is shown in loading mode and
  resumed only once it has finished. `thread/resume` is the native
  continuation mechanism; it must be used for continuing existing dialogs.

### Claude Code

- Native capability: the `claude` CLI lane (`stream-json`) resumes a native
  session with the caller-supplied native session id (`exactResume`, contract
  fixture `claude-code.json`).
- Continuation: run the CLI against the existing native session id once the
  dialog is not running; composer loading mode while it is.

### Cursor

- Native capability: the `cursor-agent` CLI lane (`cli`) resumes an exact
  native session with a caller-supplied native id (`exactResume`, contract
  fixture `cursor.json`).
- Continuation: same pattern — resume the exact native session once the
  dialog is not running; composer loading mode while it is.

### Antigravity

- Native capability: the Antigravity CLI lane (`cli`, argv-hook protocol)
  resumes an exact native session (`exactResume`, contract fixture
  `antigravity.json`).
- Continuation: resume the exact native session once the dialog is not
  running; composer loading mode while it is.

### Hermes

- Native capability: the ACP lane (`session/load` with a caller-supplied
  native id, contract fixture `hermes.json`, supervised as `process-tree`).
  The VM/SSH gateway form is a remote transport to one Hermes runtime.
- Continuation: resume the native session with `session/load` once the dialog
  is not running; composer loading mode while it is.

### GitHub Copilot

- Native capability: the Copilot ACP lane resumes a session with a
  caller-supplied native id (`session/load`, contract fixture `copilot.json`).
- Continuation: resume the native session once the dialog is not running;
  composer loading mode while it is.

### Kimi Code

- Native capability: the Kimi Code ACP lane resumes a session with a
  caller-supplied native id (`session/load`, contract fixture `kimi-code.json`).
- Continuation: resume the native session once the dialog is not running;
  composer loading mode while it is.

### Pi Agent

- Native capability: the Pi RPC lane resumes a native session with a
  caller-supplied native id (contract fixture `pi.json`). The lane does not
  expose a durable cancel handle (`cancel: false`), so interruption is not
  part of its native surface.
- Continuation: resume the native session once the dialog is not running;
  composer loading mode while it is.

### Lico Agent

- Native capability: the Lico Agent RPC lane's transcript is parent-owned and
  reinjected by the driver ("parent-owned transcript reinjected by the
  driver", contract fixture `lico-agent.json`). Conversation authority
  belongs to the LicoUp client itself, so continuation does not depend on an
  external framework store.
- Continuation: continue the dialog by reinjecting the owned transcript into
  a fresh RPC process; the default observation and composer-loading rules
  still apply.

- `consequences` —
  - Users of a running dialog wait for it to finish before continuing; the
    composer is read-only while a conversation is running.
  - The regime needs a per-conversation running-state projection in the
    conversation list and a composer loading mode.
  - Each lane's native continuation mechanism (serve attach, gateway reuse,
    session/thread resume, transcript reinjection) is the implementation
    target; the sections above are the reference for it.
- `status` — implemented and verified, 2026-08-23.
