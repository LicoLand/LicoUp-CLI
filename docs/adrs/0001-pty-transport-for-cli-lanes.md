# ADR-0001: PTY transport for the Antigravity and Cursor CLI lanes

Status: implemented and verified (2026-08-07) · Authority: this record links to code and the driver registry, which own the current facts

## Context

LicoUp converses with agent CLIs through per-agent drivers that all share one
shape (`execute` in `crates/licoup-native/src/platform/*_driver/`). Two lanes
relied on argv-passed prompts and pipes:

- **Antigravity** (`antigravity_driver/`) launched `agy --print=<prompt>` with
  piped stdio and returned only the final stdout after process exit. The driver
  explicitly skipped progressive chunk events because "Antigravity exposes only
  the final stdout after the CLI process exits" — real-time response evidence
  did not exist for this lane.
- **Cursor** (`cursor_driver/`) already streamed NDJSON, but its turn
  subprocess ran on pipes with `isatty=false` and a null stdin.

Both lanes lacked real terminal semantics. The product need: run the agent CLI
attached to a pseudo-terminal in the background, read its output incrementally,
and bind the stream into the conversation surface — the same approach the
platform affords on Linux (and macOS; Windows is a separate lane).

## Decision

Introduce a shared unix-only PTY foundation `pty_transport.rs` and use it for
the Antigravity and Cursor turn lanes:

1. **Zero new dependencies**: raw `libc` FFI (`openpty`, `cfmakeraw`,
   `tcgetattr`/`tcsetattr`, `TIOCSWINSZ`) — `libc 0.2` is already a dependency.
2. **stdin+stdout on the pty slave, stderr stays a real pipe**. The slave runs
   in raw mode (OPOST off), so `\n` is not translated to `\r\n` and structured
   line-based protocols parse byte-identically to pipes. A piped stderr keeps
   driver stderr-counting semantics intact and keeps stderr noise out of the
   stdout protocol stream.
3. **`spawn(command: Command)` takes the command by value and drops it before
   returning**: `std` keeps `Stdio::from(OwnedFd)` descriptors open in the
   parent until the `Command` drops, and a parent-held slave fd would keep the
   master from reaching EOF/EIO when the child exits (every turn would stall
   until timeout).
4. **`Master::read` translates Linux's EIO (all slaves closed) into a clean
   EOF** and retries EINTR, so natural child exits close the stream instead of
   surfacing as read errors (macOS returns EOF natively).
5. **Reader thread with a bounded event protocol**: `Data` / `Truncated` /
   `Closed`. On exceeding the byte cap the allowed prefix is delivered, then
   reads continue discarding until EOF — truncate-and-succeed without letting a
   chatty child block on the pty buffer. This strictly improves on the old
   pipe `read_bounded`, which degraded to a timeout failure when a chatty child
   outran the pipe buffer.
6. **Incremental ANSI stripper for the Antigravity lane**: CSI / OSC / DCS /
   single-char escapes and CR bytes are dropped; cursor movement is not
   interpreted (the `--print` output contract is sequential text). Escape
   sequences and multibyte UTF-8 may span read boundaries; the concatenated
   output is byte-exact.
7. **Antigravity** now emits `agent.turn.accepted` at start and
   `agent.message.chunk` progressively from the pty stream; `timeout_ms == 0`
   now means "no deadline", matching Cursor/Claude Code and the dispatch
   contract (previously 0 was an instant timeout). Auth gate, hook receipt, and
   session resume mechanics are unchanged.
8. **Cursor** keeps its NDJSON parser unchanged (`read_protocol_messages`);
   only the turn subprocess spawn moves to the pty. `create-chat` session
   bootstrap stays on pipes.
9. **Non-unix platforms keep the historical pipe implementation** (a `cfg`
   variant per driver); Windows ConPTY is a separate future lane.
10. **Registry**: `agent-conversation-drivers.json` flips Antigravity's
    `capabilityMatrix.streaming` to `true` and `lifecycleEvidence.accepted` /
    `responding` to `true`. `processing` intentionally stays `false` — every
    driver with `processing: true` emits `agent.turn.processing` evidence;
    Antigravity emits accepted + chunks + completed, so flipping it would be an
    unbacked claim. Readiness status remains `unverified`.

## Alternatives considered

- **`portable-pty` crate** — mature, cross-platform (Unix ptmx + Windows
  ConPTY), used by WezTerm. Rejected: new dependency, and its async API does
  not match the crate's synchronous dispatch threads.
- **`nix::pty` (`openpty`/`forkpty`)** — would require enabling the `nix`
  "term" feature. Rejected under the zero-new-dependencies rule when raw
  `libc` is already available.
- **Full terminal emulator** (cursor movement, scrollback, alternate screens) —
  unnecessary for sequential `--print` text; the stripper contract covers the
  current lanes.
- **Controlling terminal session (`setsid` + `TIOCSCTTY`)** — deferred to a
  future interactive-TUI lane. `isatty` plus streaming is all `--print` lanes
  need; the documented consequence is that pty-generated job-control signals do
  not reach the child, while cancellation still works through process-group
  SIGTERM (`control.rs`).
- **Windows ConPTY** — out of scope for this change, consistent with the
  platform-adaptation release positioning (each platform is released through
  its own channel).

## Rationale

The driver contract (`execute(...) -> RunResult` + the thread-local stream sink
in `turn_event_emit.rs`) required no signature change: the pty lane consumes
the master in a reader thread, emits progressive chunks on the dispatch thread,
and returns the same `RunResult`. The Dart conversation surface renders chunk
events by `participantAgentId` and tolerates empty `sessionId`, so new-session
chunks (which cannot carry a native session id until the hook receipt is
written at exit) render correctly. A unix-first foundation with a piped
fallback keeps the change bounded and reviewable.

## Consequences

- Antigravity conversations now stream progressively on unix instead of
  appearing only at completion.
- Chunk events carry the *requested* session id, which is empty for a new
  session; the terminal response carries the native session id.
- `agent.message.completed` is sink-emitted on unix in addition to the
  post-hoc events envelope; the Dart controller fills final text from
  `dispatch.turn.completed` only when no streamed text exists, so there is no
  double render.
- Truncate-and-succeed is now reliable on unix for output beyond the byte cap.
- `Master` write (`try_clone`) and `resize` are foundation API for the future
  interactive-TUI lane and are not yet wired into callers.
- Non-unix behavior is unchanged.

## Implementation evidence

- New module `crates/licoup-native/src/platform/pty_transport.rs` with 11 unit
  tests (stripper state machine, incremental streaming, byte-cap truncation,
  EOF/EIO semantics, terminate unblock, stdin write, winsize default).
- `antigravity_driver/` — 12 tests, including
  `execute_streams_pty_chunks_before_completion` (progressive chunks precede
  the completed event) and `execute_with_zero_timeout_runs_to_completion`
  (0-deadline regression).
- `cursor_driver/` — existing resume test now exercises the pty transport and
  asserts a parsed `agent.message.chunk` through the raw-mode pty.
- `runtime_adapters/tests/registry.rs` — profile test asserts
  `capabilityMatrix.streaming == true` for Antigravity and the registry JSON
  keeps `lifecycleEvidence.processing == false`.
- Gates: `npm run client:native:test` (1678 passed; the 6 failures in
  `core::secure_mesh_*` / `domain::mobile_relay` are pre-existing at HEAD —
  verified in a clean worktree — and unrelated to this change),
  `client:native:clippy` (`-D warnings`), `client:native:fmt:check`,
  `client:native:smoke` — all green for this change.

Current technical facts remain owned by code and the driver registry:
`resources/agent-conversation-drivers.json` (contract version CL-06) is the
authority for driver capabilities.
