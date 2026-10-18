# ADR 0008: Native Agent parser and conversation integrity

Status: Implemented

## Context

Packaged Agent runtimes use thirteen different CLI, JSONL, JSON-RPC, ACP,
HTTP, and SSE frame shapes. Parsing those shapes inside execution loops made
text reconciliation, interactive requests, terminal errors, privacy, and
lifecycle evidence disagree.

## Decision

Use an Adapter at the returned-frame boundary. Every packaged runtime has one
registered parser component under `native_agent_parser/adapters/`; downstream
conversation code receives only closed typed transitions. Transport remains
responsible for process and HTTP I/O.

Text units distinguish delta from cumulative snapshots and emit each suffix
once. The first exact terminal failure is write-once. Successful reply-backed
turns persist the explicit `submitted → accepted → processing → responding →
completed` prefix in Rust. Flutter renders that canonical evidence and an
observer disconnect cannot replace a persisted native failure.

Assistant user text remains exact. Bounded workflow guidance travels as a
private, non-durable request field. Answerable native interactions use one
opaque, process-local, single-use callback route with no elapsed-time expiry;
unsupported shapes fail closed.

## Alternatives

- Driver-local parsing was rejected because it duplicated wire and terminal
  rules.
- A compatibility parser or heuristic JSON scraping was rejected because it
  could reinterpret arbitrary output.
- A plugin Facade was rejected because transport and session ownership remain
  concrete runtime concerns.

## Consequences

Adding a packaged adapter requires one parser registration and focused
synthetic protocol fixtures. Raw frames are not durable conversation facts,
agent output is not truncated by default, and approvals are never auto-allowed.

## Implementation status

All thirteen packaged conversation adapters now enter their isolated parser at
the raw returned-frame boundary. Runtime normalization serializes only typed
parser transitions, the scoped interaction route resumes parked native turns,
and Flutter renders the explicit Rust lifecycle prefix and terminal transition.
Focused Rust, Node, and rendered Flutter widget evidence covers the cutover;
the temporary retired-parser residue scan reported zero retained paths or
fallback symbols and was not kept as a permanent gate.
