# Conversation Vertical Contract — Reactive State Binding Specification

[Architecture](README.md) · [Architecture (zh-CN)](README.zh-CN.md)

This document defines the precise reactive state binding contract for the full conversation
vertical pipeline: from user input on the Flutter display shell to backend execution and
state projection.

**Authority**: This specification owns the data-flow contract between layers. Each layer's
internal implementation is owned by its respective code module.

---

## Design Principles

1. **CLI is the product, Flutter is a display adapter** — The Rust native host (licoup-cli) is a
   complete semantic client. It runs independently of any UI. Flutter's sole job is to send
   user commands and render `UI = f(State)`.

2. **Reactive State Binding, not Pub/Sub** — Rust owns canonical state. Flutter holds a reactive
   mirror. State changes (from any source) propagate as structured deltas. Flutter renders current
   state — no protocol response correlation, no round-trip management, no callback chains. A
   local optional presentation trace may measure projection-to-frame latency; it never enters
   conversation content, persistence, native frames, or network traffic.

3. **Events are source-agnostic** — The Conversation Domain processes typed commands regardless
   of whether they originate from local Flutter UI, a remote peer via Lico Arc, or a local
   CLI command. This ensures the architecture supports IM group chat without core changes.

4. **Conversation ID + Membership ID are mandatory routing keys** — Every command and every
   state delta carries these identifiers from day one, even for 1:1 agent conversations.

5. **Flutter fabricates nothing** — Zero local state inference. UI reads projected state only.

6. **Conversation concurrency is Agent-owned** — LicoUp does not decide whether an Agent can
   handle concurrent turns. The Agent that actually runs the conversation holds its execution
   state. LicoUp projects whatever the Agent reports. This rule applies to all concurrency
   decisions: the runtime owner decides, LicoUp reflects.

7. **Commands are fire-and-forget mutations** — Flutter sends a command and does NOT await a
   correlated response. The result arrives as a state delta (which may be success or error).
   The "round trip" closes automatically because UI is bound to state.

8. **Presentation bindings preserve authority** — Flutter feature trees consume immutable named
   Bindings. Native-catalog sessions, Canonical Conversation Events/Parts, and Membership
   PersistentTurns remain distinct projection sources inside `ConversationBinding`; sharing a
   renderer does not merge their ownership.

9. **Conversation rendering is frontend-owned** — the shared Canonical Group pane lives under
   `frontend/features/agents/ui/conversation/` and consumes only Conversation semantic values,
   intents, effects, and renderer-local adapters. It does not import Application or behavior
   reader/source contracts.

---

## Reactive State Binding Model

```
        Commands (fire-and-forget)           State Deltas (reactive)
Flutter ────────────────────────→ Rust Host ────────────────────────→ Flutter
                                  (state owner)
                                       │
                                       │ state mutates for ANY reason:
                                       │   - local user command
                                       │   - agent output arrives
                                       │   - remote peer message (future)
                                       │   - internal process (timeout, recovery)
                                       │
                                       ↓
                                  emit StateDelta → Flutter applies → UI re-renders
```

**UI = f(State)**. Flutter never relies on causality for state correctness. State changes → UI reflects.

At the implemented M3–M6 presentation boundary, this becomes:

```text
Application owner → scoped Projection producer → ConversationBinding → Flutter renderer
       ↑                     optional local trace             │
       └──────── semantic Intent adapter ← user gesture ─────┘
```

The trace is diagnostic timing metadata only. Intent dispatch remains fire-and-forget and no
trace identifier crosses the Dart/native or network boundary. Equal semantic projections are
suppressed, and each source owns its subscription and disposal independently.

---

## Layer Overview

```
┌─────────────────────────────────────────────────────────────────┐
│ L1  Flutter Command Emitter (apps/desktop/lib/src/)             │
│     User gesture → typed Command (fire-and-forget)              │
├─────────────────────────────────────────────────────────────────┤
│ L2  Protocol Frame Transport (stdio JSON-RPC + codegen)         │
│     Commands ↑ StateDelta ↓ (structured enum, not JSON Patch)   │
├─────────────────────────────────────────────────────────────────┤
│ L3  Conversation Domain (crates/licoup-native/src/domain/)      │
│     Canonical state owner, mutation → delta emission            │
├─────────────────────────────────────────────────────────────────┤
│ L4  Agent Runtime (crates/licoup-native/src/platform/)          │
│     Agent adapter dispatch, protocol translation, I/O           │
├─────────────────────────────────────────────────────────────────┤
│ L5  Settlement Arbiter (crates/licoup-native/src/platform/)     │
│     Turn terminal verdict from protocol signals                 │
├─────────────────────────────────────────────────────────────────┤
│ L6  Flutter State Mirror (apps/desktop/lib/src/)                │
│     Apply deltas → hold local state → UI = f(state)            │
└─────────────────────────────────────────────────────────────────┘
```

---

## Canonical State Definition

This is the state that Rust owns and Flutter mirrors. It is the **single source of truth**
for everything the UI displays.

```rust
/// Per-conversation state. Each conversation has its own independent StateHolder.
/// Conversation concurrency (can Agent handle multiple turns?) is Agent-owned, not LicoUp-owned.
struct ConversationState {
    id: ConversationId,
    memberships: Vec<Membership>,
    messages: Vec<Message>,
    active_turn: Option<TurnState>,
    pending_approval: Option<ApprovalRequest>,
    input_enabled: bool,
    last_error: Option<ConversationError>,
}

struct Message {
    id: MessageId,
    membership_id: MembershipId,    // who sent this
    content: MessageContent,
    timestamp: Timestamp,
    send_state: SendState,          // the lifecycle of this message
}

/// Message send lifecycle — the critical state machine
enum SendState {
    /// Event generated locally, confirmed by Flutter.
    /// Now in transit: LicoUp internal (Flutter→Rust) + external (Rust→Agent).
    Sending,

    /// Agent confirmed receipt. Terminal success.
    Delivered,

    /// Send failed at any point in the pipeline. Terminal failure.
    /// Branches from Sending into error state.
    Failed { reason: SendFailureReason },
}

enum SendFailureReason {
    /// LicoUp internal: Flutter→Rust frame delivery failed
    InternalTransportError,
    /// LicoUp→Agent: adapter could not dispatch
    DispatchFailed { details: String },
    /// LicoUp→Agent: agent rejected the message
    AgentRejected { details: String },
    /// LicoUp→Agent: transport to agent lost during send
    AgentTransportLost,
}

struct TurnState {
    turn_id: TurnId,
    target: MembershipId,            // which agent is executing
    phase: TurnPhase,
    stream_content: String,          // accumulated streaming output
    tool_calls: Vec<ToolCall>,
    usage: Option<Usage>,
}

enum TurnPhase {
    Dispatching,        // Rust is setting up the adapter session
    Running,            // Agent is executing
    Streaming,          // Agent is producing output tokens
    WaitingForHuman,    // Agent requested approval/input
    Completed,          // Terminal: agent finished successfully
    Failed { reason: String },  // Terminal: execution error
    Cancelled,          // Terminal: user cancelled
}
```

### SendState Lifecycle Diagram

```
User composes message → clicks Send
     │
     ▼
  [Event Generated] ← Flutter confirms the message object is created
     │
     ▼
  [Sending] ← In transit through LicoUp (Flutter→Rust) and to external Agent
     │
     ├──── success ───→ [Delivered] ← Agent confirmed receipt
     │
     └──── failure at ANY point ───→ [Failed { reason }]
                                      (branches from Sending node into error path)
```

**Key rule**: The `Sending` state covers the ENTIRE pipeline from local event generation to
Agent receipt confirmation. It does not distinguish "internal transport" from "external delivery"
in the state model — that distinction is only in the `SendFailureReason` if it fails.

---

## Structured Delta Enum (L2 wire format)

State changes are communicated as **structured enum deltas**, not generic JSON Patch.
Generated by codegen from a shared schema.

```rust
/// Every delta targets a specific conversation
enum ConversationDelta {
    // Full state (reconnection, first load)
    FullSnapshot(ConversationState),

    // Message lifecycle
    MessageCreated { message: Message },                     // new message with SendState::Sending
    MessageSendStateChanged { message_id: MessageId, new_state: SendState },

    // Turn lifecycle
    TurnStarted { turn: TurnState },
    TurnPhaseChanged { phase: TurnPhase },
    StreamContentAppend { text: String },                    // append, not replace
    ToolCallAppended { tool_call: ToolCall },
    TurnEnded { phase: TurnPhase, usage: Option<Usage> },   // terminal phase
    TurnCleared,                                            // active_turn = None

    // Interaction
    ApprovalRequested { request: ApprovalRequest },
    ApprovalCleared,

    // Input control
    InputEnabledChanged { enabled: bool },

    // Error
    ErrorOccurred { error: ConversationError },
    ErrorCleared,

    // Membership (for future IM: member joins/leaves)
    MembershipAdded { membership: Membership },
    MembershipRemoved { membership_id: MembershipId },
}
```

### Delta Rules

1. Each state mutation in Rust emits exactly one delta (or a minimal batch for atomic changes)
2. `StreamContentAppend` is batched by Rust: accumulate tokens, flush every 16ms as one delta
3. `FullSnapshot` is sent on first connection or when reconnection gap is too large
4. Deltas are ordered: Flutter applies them in sequence, no reordering needed
5. Delta enum is generated by codegen — adding a new variant requires schema update on both sides

---

## L1: Flutter Command Emitter

**Location**: `apps/desktop/lib/src/` (event dispatch code)

**Responsibility**: Map user gestures to typed conversation commands. Fire-and-forget. No business logic.

### Input

User gestures: tap send button, press enter, tap cancel, tap steer, drag attachment, etc.

### Output

```dart
/// Every command targets a specific conversation and membership.
/// Commands are fire-and-forget — results arrive as state deltas.
sealed class ConversationCommand {
  final String conversationId;
  final String membershipId;
}

class SendMessage extends ConversationCommand {
  final String text;
  final List<Attachment> attachments;
}

class SteerTurn extends ConversationCommand {
  final String steerText;
}

class CancelTurn extends ConversationCommand {
  final String turnId;
}

class ApprovalResponse extends ConversationCommand {
  final String requestId;
  final ApprovalDecision decision;
}
```

### Command Dispatch

```dart
/// Fire-and-forget. No await, no callback, no correlation.
/// The state delta stream will reflect the result.
void send(ConversationCommand cmd) {
  channel.writeFrame(cmd.toFrame());
}
```

### Rules

- One gesture produces exactly one command. No batching, no merging, no inference.
- `conversationId` and `membershipId` are always explicit.
- Commands are fire-and-forget. Flutter does NOT await a response.
- The "result" of a command arrives as a state delta (success path or error path).
- Flutter MAY optimistically show the message as `Sending` immediately after dispatch.
  This is reconciled by the next delta from Rust (either confirms or overrides with error).

### Errors

L1 has no domain errors. If the stdio frame cannot be written (connection lost),
the `ConnectionState` delta will inform the UI.

---

## L2: Protocol Frame Transport

**Location**: `apps/desktop/lib/src/platform/native_client/` (Dart) ↔
`crates/licoup-native/src/bin/licoup/stdio_rpc/` (Rust)

**Responsibility**: Bidirectional transport of Commands (↑) and StateDelta (↓) over stdio.
Type safety enforced by **codegen from a shared schema** using **structured enums**.

### Upward (Flutter → Rust): Command Frames

```
Frame {
  jsonrpc: "2.0",
  method: "conversation.command",
  params: {
    conversation_id: String,
    membership_id: String,
    command: CommandPayload  // generated structured enum
  }
}
```

### Downward (Rust → Flutter): State Delta Frames

```
Frame {
  jsonrpc: "2.0",
  method: "conversation.delta",
  params: {
    conversation_id: String,
    delta: ConversationDelta  // generated structured enum (see above)
  }
}
```

### Codegen Contract

- Schema definition location: `schemas/conversation_protocol/`
- Generates Dart: typed Command builders + ConversationDelta decoders
- Generates Rust: typed Command decoders + ConversationDelta builders
- Schema changes require version bump; mismatched frames rejected at L2
- **Zero hand-written serialization** after codegen is established
- Delta format is **structured enum**, NOT generic JSON Patch

### Parser Layered Architecture

The protocol layer shares a layered parser architecture with the Agent adapter
parsers (L4). Two layers, reused across both directions:

```
┌─────────────────────────────────────────────────────────┐
│ Frame Layer (shared infrastructure)                      │
│   raw bytes → length-delimited frames → typed envelope  │
│   Streaming partial parse, error recovery, backpressure │
│   Used by: L2 (Flutter↔Rust) AND L4 (Rust↔Agent)       │
├─────────────────────────────────────────────────────────┤
│ Payload Layer (direction-specific)                       │
│                                                         │
│   L2 upward:  envelope → ConversationCommand (decode)   │
│   L2 downward: ConversationDelta → envelope (encode)    │
│   L4 downward: agent wire bytes → AdapterEvent (decode) │
│                                                         │
│   L2 instances are codegen-generated (controlled proto) │
│   L4 instances are per-agent manual parsers (varied)    │
└─────────────────────────────────────────────────────────┘
```

**Why this matters**: The existing `native_agent_parser/` is already a Payload Layer
implementation for L4. The protocol codegen produces Payload Layer implementations
for L2. Both share Frame Layer infrastructure: streaming decode, partial frame
handling, error boundaries, and backpressure propagation. This avoids duplicating
low-level byte handling logic across two directions.

### Connection State

L2 projects its own transport state as a special non-conversation delta:

```dart
enum ConnectionState { connecting, connected, disconnected, reconnecting }
```

### Reconnection

On reconnect, Rust pushes `ConversationDelta::FullSnapshot` for each conversation that
Flutter was previously observing. Flutter replaces its local state entirely. No "replay
missed deltas" logic needed — snapshot is always consistent.

### Streaming Performance

`StreamContentAppend` deltas are batched by Rust (16ms accumulation window) to stay within
one frame budget. Flutter applies the append and re-renders only the new text portion.

---

## L3: Conversation Domain (State Owner)

**Location**: `crates/licoup-native/src/domain/client_conversation/`

**Responsibility**: Owns canonical `ConversationState`. Processes commands → mutates state →
emits deltas. Source-agnostic (same entry for local Flutter, future remote peers, CLI commands).

### Core Rule: Every Mutation Emits a Delta

```rust
impl ConversationDomain {
    fn process_command(&mut self, cmd: Command) {
        // mutate state
        self.state.apply(cmd);
        // emit delta (MANDATORY — no silent mutations)
        self.emit_delta(derive_delta_from_mutation());
    }
}
```

### Command Processing → Delta Emission

| Command | State Mutation | Delta Emitted |
|:---|:---|:---|
| SendMessage | create Message(Sending), set input_enabled=false, create TurnState(Dispatching) | `MessageCreated` + `InputEnabledChanged(false)` + `TurnStarted` |
| SteerTurn | forward to L4 adapter | (delta comes when adapter responds) |
| CancelTurn | signal L4, set phase=Cancelled | `TurnPhaseChanged(Cancelled)` + `TurnEnded` |
| ApprovalResponse | forward to L4, clear pending | `ApprovalCleared` |

### Message Send Lifecycle (L3 manages the full pipeline)

```
SendMessage command arrives at L3:
  1. Create Message object with SendState::Sending
  2. Emit delta: MessageCreated { message }  ← Flutter sees it immediately
  3. Dispatch to L4 (adapter)
  4. ... adapter + agent processing ...

  SUCCESS path:
    5a. Agent confirms receipt
    6a. Emit delta: MessageSendStateChanged { Delivered }

  FAILURE path (at ANY point after step 3):
    5b. Failure detected (internal transport, dispatch, agent reject, transport lost)
    6b. Emit delta: MessageSendStateChanged { Failed { reason } }
```

**The Sending state is a single unified state** covering the entire pipeline from event
generation through LicoUp internals to external Agent receipt. Failure branches from
Sending regardless of where in the pipeline it occurred.

### Concurrency Rule

**Conversation concurrency is Agent-owned, not LicoUp-owned.**

L3 does NOT enforce "one active turn per conversation." Whether an Agent can handle
concurrent turns is determined by the Agent's own capability and runtime. L3 forwards
commands to L4; if the Agent cannot handle it, the Agent (via L4) reports failure, and
L3 emits the corresponding error delta.

This means:
- L3 never rejects a SendMessage because "a turn is already active"
- L3 lets L4/Agent decide if concurrent execution is possible
- Flutter's `input_enabled` is set by Rust based on the Agent's reported capabilities

### Invariants

1. Every state mutation emits exactly one delta (or atomic batch for multi-field changes)
2. `messages[]` is append-only except for an explicit local-owner delete command. That
   command atomically removes the selected user message and its derived turn subtree, and
   rejects while any derived turn is active.
3. State mutations are atomic with persistence (same SQLite transaction)
4. Commands from unknown conversation_id → `ErrorOccurred` delta
5. `ConversationState` per conversation is fully independent — zero shared mutable state

---

## L4: Agent Runtime

**Location**: `crates/licoup-native/src/platform/` (adapter drivers)

**Responsibility**: Dispatch commands to the appropriate agent adapter, manage subprocess/RPC
lifecycle, translate agent-specific protocols into a normalized event stream.

### Input (from L3)

```rust
struct DispatchCommand {
    conversation_id: ConversationId,
    membership_id: MembershipId,
    turn_id: TurnId,
    agent_id: AgentId,
    adapter: AdapterKind,  // ACP, AppServer, CLI, RPC, etc.
    payload: DispatchPayload,  // NewTurn { message, attachments } | Steer { text } | Cancel
}
```

### Output (back to L3 and directly to L2 for streaming)

```rust
enum AdapterEvent {
    // Session lifecycle
    SessionBound { session_id },
    SessionLost { reason },

    // Content stream (high-frequency, bypasses L3 for low latency)
    StreamChunk { kind: ChunkKind, content: String },

    // Structured transitions
    ToolCallStarted { tool_name, arguments },
    ToolCallCompleted { tool_name, result },
    ApprovalRequested { request_id, tool_name, description },
    UsageReported { input_tokens, output_tokens },

    // Terminal signals
    ProtocolFinish,      // agent signaled explicit completion
    TransportEOF,        // connection/process closed
    TransportError { error },
    CancelConfirmed,
}
```

### Streaming Path (Low Latency)

`StreamChunk` events bypass L3's state machine and go directly from L4 → L2 → Flutter
for minimal latency. L3 is notified of TurnState::Streaming but does not buffer individual
chunks. Final committed content is assembled by L5 and stored as a single Event in L3.

### Adapter Responsibilities (what each adapter MUST do)

1. Start/connect to the agent process or server
2. Translate `DispatchPayload` into agent-specific protocol
3. Emit `AdapterEvent` for every observable behavior from the agent
4. Never make terminal decisions — only report protocol signals; L5 decides

### Adapter Responsibilities (what each adapter MUST NOT do)

1. Never decide if a turn is "complete" (that's L5)
2. Never timeout a turn (timeouts are L5's domain)
3. Never hide agent output (every byte must be emitted as StreamChunk or structured event)
4. Never synthesize completion signals that didn't come from the agent protocol

---

## L5: Settlement Arbiter

**Location**: `crates/licoup-native/src/platform/` (settlement module)

**Responsibility**: Determine the terminal verdict for a turn based solely on protocol-level
signals from L4. This is the ONLY place that decides TurnOutcome.

### Input

`AdapterEvent` stream from L4 for one turn.

### Output

```rust
enum TurnOutcome {
    Completed {
        final_content: CommittedContent,
        usage: Option<Usage>,
    },
    Failed {
        reason: FailureReason,
        partial_content: Option<CommittedContent>,
    },
    Cancelled {
        partial_content: Option<CommittedContent>,
    },
}

enum FailureReason {
    AdapterError { message: String },
    TransportLost,
    ProcessCrashed { exit_code: Option<i32> },
    ProtocolViolation { details: String },
}
```

### Settlement Rules

| Signal | Verdict |
|:---|:---|
| `ProtocolFinish` (explicit agent completion signal) | **Completed** |
| `TransportEOF` after receiving valid content | **Completed** (EOF is normal termination for many agents) |
| `TransportEOF` with zero content | **Failed**(TransportLost) — not "empty completion" |
| `TransportError` | **Failed**(AdapterError) |
| `CancelConfirmed` | **Cancelled** |
| User `CancelTurn` + no `CancelConfirmed` within grace period | **Cancelled** (forced) |

### What DOES NOT determine settlement

| Non-signal | Why it's NOT a settlement trigger |
|:---|:---|
| Silence / time elapsed | Agents can think for minutes; silence is not failure |
| Empty text field | Pure tool-call turns produce no text; this is valid completion |
| Partial text received | Streaming content ≠ completion |
| Flutter observer disconnect | GUI detach is not cancellation |
| 120s timeout | **REMOVED** — no hardcoded timeouts exist in this architecture |

### Process Supervision (for subprocess-based agents)

When cancellation requires process termination:
```
Cancel requested
  → Send protocol-level cancel signal (adapter-specific)
  → Grace period (configurable per adapter, default 5s)
  → SIGTERM
  → Kill period (3s)
  → SIGKILL
  → Verdict: Cancelled
```

---

## L6: Semantic Binding & Flutter Display

**Location**: `apps/desktop/lib/src/presentation/conversation/` and
`apps/desktop/lib/src/frontend/`

**Responsibility**: Render `UI = f(semantic projections)` through one immutable
Conversation Binding. Application owns mutable client state; feature-local producers map
that state to equal-suppressed projections. Flutter owns no Conversation authority and
contains zero business logic.

### Binding Surface

```dart
final class ConversationBinding {
  final ProjectionSource<ConversationProjection> projection;
  final ProjectionSource<NativeConversationCatalogProjection> nativeCatalog;
  final ProjectionSource<CanonicalConversationProjection> canonicalEvents;
  final ProjectionSource<PersistentTurnProjection> persistentTurns;
  final ProjectionSource<ComposerProjection> composer;
  final ProjectionSource<ConversationAttachmentsProjection> attachments;
  final ProjectionSource<ConversationTabActivityProjection> tabActivity;
  final ProjectionSource<ConversationNotificationsProjection> notifications;
  final ProjectionSource<ConversationArchiveProjection> archive;
  final IntentSink<ConversationIntent> intents;
  final EffectSource<ConversationEffect> effects;
}
```

The native history catalog remains the authority for local 1:1 panes. Canonical Events
and Parts remain the authority for group panes. Both surfaces attach the same
Membership-scoped PersistentTurn observer; no producer merges those stores or creates a
second transcript.

### Display Rules

```dart
Widget build(BuildContext context) {
  return ProjectionBuilder<ComposerProjection, ComposerProjection>(
    source: binding.composer,
    select: (projection) => projection,
    builder: (context, composer) => Composer(
      enabled: composer.inputEnabled,
      onSend: (text) => binding.intents.send(
        PostConversationMessage(
          conversationId: composer.conversationId,
          content: text,
          addressedMembershipIds: const [],
          dispatchCanonical: false,
        ),
      ),
    ),
  );
}
```

### Rendering Rules

1. **Display what state says, nothing more, nothing less**:
   - `message.sendState == Sending` → show sending indicator on that message
   - `message.sendState == Failed` → show error badge on that message
   - `activeTurn.phase == WaitingForHuman` → show approval UI
   - `activeTurn.phase == Streaming` → show growing text from `streamContent`
   - `inputEnabled == false` → disable composer

2. **Never synthesize state**:
   - DO NOT infer send success from absence of error
   - DO NOT fabricate turn lifecycle events
   - DO NOT hide any state that Rust projects (including errors)

3. **Performance**:
   - Message list in RepaintBoundary (isolate from turn state changes)
   - Streaming text: only rebuild the text widget, not the message list
   - Use narrow `ProjectionBuilder` selectors for fine-grained rebuilds
   - Runtime deltas update Application state, then one equal-suppressed semantic source;
     Flutter never applies protocol deltas itself

4. **Assistant identity test authority**:
   - Tests derive expected Agent, model, and reasoning labels from a mutable
     backend Membership Profile fixture.
   - Tests must not hard-code a real Agent + model pairing as the asserted UI
     result. Catalog choices can change without changing this projection
     contract; the UI must always equal the backend Profile currently read.

5. **Failed-message composition**:
   - A failed user message continues to use the ordinary user-message bubble.
   - A transport failure after the user Event commits writes a causal diagnostic,
     so retry state survives refresh and relaunch.
   - Retry is an adjacent child affordance of that message row, never a replacement
     failure bubble or a duplicate transcript item.
   - Right-click delete targets the canonical message id and is available only for
     local-owner messages.

6. **Terminal failure projection**:
   - The runtime parser owns the first stable failure code. A recognized provider
     quota failure must not be collapsed into a generic incomplete-turn code.
   - Canonical diagnostics retain only the safe code, stage, component,
     retryability, recovery token, and closed turn-status class. Raw provider
     messages, reset timestamps, paths, and runtime identities are never stored.
   - The group-chat banner localizes that contract into a concise cause and an
     actionable recovery. Tests assert the typed class without fixing a real
     Agent + model pairing.

---

## Cross-Layer Invariants

1. **Delta completeness**: For any conversation state S reachable from initial state S₀,
   the delta stream is sufficient for Flutter to reconstruct S exactly. No side-channel
   queries needed.

2. **Snapshot consistency**: `FullSnapshot` delta always represents a valid, consistent state.
   Flutter can replace its entire local mirror with a snapshot at any time and display
   correctly.

3. **Conversation isolation**: Deltas for conversation A never affect conversation B.
   `conversation_id` is an absolute partition key. Each conversation has its own independent
   `ConversationStateHolder`.

4. **Conversation concurrency is Agent-owned**: LicoUp does not decide whether concurrent
   turns are allowed. The Agent runtime owner decides. LicoUp reflects whatever the Agent
   reports. If an Agent reports two concurrent turns, LicoUp displays both.

5. **Membership routing**: Every command and delta carries `membership_id`. This supports
   group chat (N members) without architectural change.

6. **Terminal finality**: Once a TurnPhase reaches terminal (Completed/Failed/Cancelled),
   it never transitions again. Once SendState reaches terminal (Delivered/Failed), it
   never changes.

7. **Send lifecycle completeness**: A Message in `Sending` state WILL eventually transition
   to either `Delivered` or `Failed`. There is no state where a message stays `Sending`
   indefinitely without resolution. The path: Event generated → Sending → {Delivered | Failed}.

8. **Fire-and-forget commands**: Flutter never awaits a command response. The "response" is
   a state delta. Flutter renders current state continuously — the delta arrival IS the
   response, visible as a state change in the UI.

---

## IM Extensibility Points (designed now, implemented later)

| Extension | What's already in place | What's added later |
|:---|:---|:---|
| Group chat (N humans + M agents) | `memberships[]`, `membership_id` on all commands/deltas | Multi-member UI, typing indicators |
| Remote peer messages | L3 is source-agnostic; delta model covers any message source | Lico Arc ingress feeds L3 same as local commands |
| Multi-device sync | `FullSnapshot` on connect; state is replayable | Cross-device delta relay |
| Offline messages | Messages have explicit `SendState` lifecycle | Queue + delivery receipts |
| Typing indicators | Delta enum is extensible (add new variant) | `TypingStateChanged { membership_id, typing: bool }` |
| Read receipts | Message model supports metadata | `MessageReadStateChanged { message_id, read_by }` |
| Presence | Per-membership state fields | `MemberPresenceChanged { membership_id, presence }` |
| Concurrent turns | **Concurrency is Agent-owned** — model already supports multiple active turns | UI for parallel turn display |

### Why Agent-Owned Concurrency Enables IM

In a group chat with multiple Agents:
- Agent A may be executing a turn while Agent B is idle
- Agent C may support concurrent turns (e.g., a stateless tool agent)
- LicoUp does NOT decide who can run — each Agent's runtime decides

This means the state model naturally supports:
```rust
// Future: multiple active turns in one conversation (group chat)
struct ConversationState {
    active_turns: Vec<TurnState>,  // plural, not Option<TurnState>
    // ...
}
```

The current `Option<TurnState>` is a simplification for 1:1 agent chat.
Extending to `Vec<TurnState>` requires only a delta enum addition, not an architecture change.

---

## Implementation Priority

1. **Schema + Codegen** — Define `schemas/conversation_protocol/` and generate both sides
2. **L3 Projection completeness** — Ensure every Rust state change emits projection
3. **L5 Settlement rules** — Remove false-completion/false-failure logic
4. **L6 Faithful rendering** — Delete Flutter state fabrication
5. **L4 Adapter compliance** — Ensure adapters report all signals, decide nothing
6. **L2 Streaming** — Ensure low-latency stream path works with backpressure
