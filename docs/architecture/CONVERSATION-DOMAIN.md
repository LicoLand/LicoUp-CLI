# Canonical Conversation Vertical Domain Architecture Specification

| Related Document | Language / Path | Authority |
|:---|:---|:---|
| **Normative Version** | English (Normative) | Authoritative technical specification |
| **Localization** | [简体中文](CONVERSATION-DOMAIN.zh-CN.md) | Localized Chinese projection |
| **Architecture Root** | [docs/architecture/README.md](README.md) | 4-tier client architecture overview |
| **Rust Infrastructure** | [RUST-INFRASTRUCTURE-LAYER.md](RUST-INFRASTRUCTURE-LAYER.md) | Database, config, secrets, network, and PTY |
| **Agent Adapters** | [AGENT-ADAPTERS-ARCHITECTURE.md](AGENT-ADAPTERS-ARCHITECTURE.md) | 13-agent taxonomy & protocol normalization |
| **Bridge Interaction** | [CLIENT-NATIVE-INTERACTION.md](CLIENT-NATIVE-INTERACTION.md) | Frontend-backend RPC / FFI frame contract |
| **Product Charter** | [PRODUCT.md](../../PRODUCT.md) | Durable one-conversation product destination |

In LicoUp, **Conversation is formally established as an End-to-End Vertical Architecture Slice**, rather than an isolated module tucked inside Rust. It cuts vertically across all four horizontal architectural tiers, connecting user presentation, bridging contracts, domain state machines, infrastructure subsystems, and the native operating system.

This document unifies the **four-tier vertical breakdown, bidirectional frontend-backend binding model, atomic direct chat base with group orchestration encapsulation, dedicated Profile abstraction, state machine driven execution, synchronized progress reflection, and end-to-end sequence lifecycles**.

---

## 1. Vertical Slice Architecture

```mermaid
flowchart TB
    %% Styling & visual definitions
    classDef t1 fill:#e8f4fd,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef t2 fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef t3_d fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef t4 fill:#f1f3f5,stroke:#495057,stroke-width:2px,color:#212529;
    classDef box fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;
    classDef coreBox fill:#ffffff,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;

    subgraph TIER1["[Tier 1] Flutter Presentation & Application Layer"]
        direction TB
        subgraph T1_VIEWS["UI Components & Views"]
            direction LR
            V_COMPOSER["Composer Input<br/>(Send Lock · Drafts)"]:::box
            V_PROFILE["Profile Edit & Echo<br/>(Role & Avatars)"]:::box
            V_APPROVAL["Approval Modal<br/>(Sanitized Summaries)"]:::box
            V_STREAM["Bubble Stream Workspace<br/>(Markdown · History)"]:::box
            V_BLACKBOARD["Process Blackboard & Progress<br/>(Reasoning · State Mirror)"]:::box
        end
        UI_CTRL["ClientConversationController (Debounce · Progress Sync)"]:::coreBox
        V_COMPOSER --> UI_CTRL
        V_PROFILE --> UI_CTRL
        V_APPROVAL --> UI_CTRL
        UI_CTRL --> V_STREAM
        UI_CTRL --> V_BLACKBOARD
    end

    subgraph TIER2["[Tier 2] Bridging Contract Layer (Contract Layer)"]
        direction LR
        RPC_DESK["Desktop JSON-RPC<br/>(licoup.stdio.v1 Frames)"]:::box
        RPC_MOBILE["Mobile C-ABI FFI<br/>(Platform Bridges Commands)"]:::box
        RPC_OBSERVER["Bidirectional Observer Stream<br/>(Streaming Parts · State Steps)"]:::box
    end

    subgraph TIER3["[Tier 3] Rust Domain Core & Infrastructure Layer"]
        direction TB
        subgraph T3_DOMAIN["3.1 Conversation Domain Core"]
            direction LR
            D_INGEST["Domain Ingestion & Preflight"]:::box
            D_DISPATCH["Dispatch Door"]:::box
            D_FSM["Session State Machine"]:::coreBox
            D_PARSER["L1 Parser & Settlement Arbiter<br/>(native_agent_parser)"]:::box
            D_INTERACT["L2 Interaction & Approval Gate"]:::box
            D_INGEST --> D_DISPATCH --> D_FSM
            D_FSM --> D_PARSER
            D_FSM --> D_INTERACT
        end

        subgraph T3_INFRA["3.2 Infrastructure & Boundary Gateway"]
            direction LR
            I_DB["Database Storage<br/>(SQLite WAL · Transactions)"]:::box
            I_PTY["PTY/TTY Console Module<br/>(Pseudo Terminal · Winsize)"]:::box
            I_NET["Network Transport<br/>(HTTP/SSE · SSH Tunnels)"]:::box
            I_CONF["Dynamic Config System<br/>(Agent Manifests · Env)"]:::box
            I_SEC["Secret Custody Facade<br/>(Key Derivation · Crypto)"]:::box
        end
        D_FSM -->|"Persistence Calls"| I_DB
        D_FSM -->|"Pipes & Pseudo Consoles"| I_PTY
        D_FSM -->|"Network Requests"| I_NET
        D_DISPATCH -->|"Env Resolution"| I_CONF
        D_FSM -->|"Key Operations"| I_SEC
        I_PTY -->|"Raw Frames"| D_PARSER
        I_NET -->|"Stream Frames"| D_PARSER
    end

    subgraph TIER4["[Tier 4] Native OS Adaptation Layer"]
        direction LR
        N_PTY["Terminal Subsystem<br/>(POSIX openpty / Win ConPTY)"]:::box
        N_PROC["OS Processes & Signals<br/>(SIGINT / SIGTERM / SIGKILL)"]:::box
        N_SEC["Native Hardware Keyrings<br/>(macOS Keychain / WinCred / Keystore / Enclave)"]:::box
    end

    %% Tier connections
    UI_CTRL ==>|"① Downlink Actions"| RPC_DESK
    UI_CTRL ==>|"① Mobile Invocation"| RPC_MOBILE
    RPC_DESK ==>|"② Unpack (Principal+Profile+Event)"| D_INGEST
    RPC_MOBILE ==>|"② Unpack (Principal+Profile+Event)"| D_INGEST

    I_PTY ==>|"③ System PTY Attach"| N_PTY
    I_PTY ==>|"③ Process Supervision"| N_PROC
    I_SEC ==>|"③ Hardware Keyring"| N_SEC

    N_PTY ==>|"④ Console Output"| I_PTY
    D_PARSER ==>|"⑤ Uplink Typed Stream"| RPC_OBSERVER
    RPC_OBSERVER ==>|"⑥ Sync Progress Mirror"| UI_CTRL

    class TIER1 t1;
    class TIER2 t2;
    class TIER3 t3_d;
    class TIER4 t4;
```

---

## 2. L1-L5 Five-Layer Target Architecture

Based on systematic audits of 16 reports covering 50+ cataloged defects, the root cause of conversation flow interruptions is **authority fragmentation — multiple independent synthesizers competing for write authority over the same product** (e.g., "is the turn complete?", "current progress prefix", "is the system waiting for human input?"). The target architecture introduces 5 dedicated layers, each with exclusive ownership of specific products:

```mermaid
flowchart TB
    classDef l5 fill:#e7f5ff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef l4 fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef l3 fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef l2 fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f;
    classDef l1 fill:#fff5f5,stroke:#c92a2a,stroke-width:2px,color:#7c2d12;
    classDef box fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;

    subgraph L5_LAYER["L5: Flutter Evidence Consumption Unification"]
        direction LR
        L5_P1["Eliminate dual enums<br/>(single ConversationTurnProcessState)"]:::box
        L5_P2["Single terminalTransition<br/>consumption point"]:::box
        L5_P3["Prohibition: Dart must not<br/>fabricate Dispatch events"]:::box
    end

    subgraph L4_LAYER["L4: Continuity & Session Identity"]
        direction LR
        L4_P1["Exact Resume verification<br/>(fail-closed on mismatch)"]:::box
        L4_P2["AwaitSession isolation<br/>(no premature active registration)"]:::box
        L4_P3["Honest in-flight declaration<br/>(crash = interrupted, not running)"]:::box
    end

    subgraph L3_LAYER["L3: Transport & Process Supervision"]
        direction LR
        L3_P1["Unified ControlDisposition<br/>(Accepted / NoActiveTurn /<br/>SessionUnavailable / Unsupported)"]:::box
        L3_P2["DispatchDeadlinePolicy<br/>(replaces hardcoded 120s)"]:::box
        L3_P3["Unified line/frame reader<br/>& supervision ladder"]:::box
    end

    subgraph L2_LAYER["L2: Interaction Routing & Settlement"]
        direction LR
        L2_P1["Unified Park-and-Wait<br/>(replaces 4 scattered loops)"]:::box
        L2_P2["Turn-terminal fail-closed<br/>settlement of pending interactions"]:::box
        L2_P3["Activate WaitingForHuman<br/>SQLite write path"]:::box
    end

    subgraph L1_LAYER["L1: Parser & Turn Settlement Arbiter"]
        direction LR
        L1_P1["TurnSettlementArbiter<br/>(sole completion authority)"]:::box
        L1_P2["Extended Transition vocabulary<br/>(ApprovalRequest / Usage / Progress)"]:::box
        L1_P3["Cancelled vs Failed<br/>terminal separation"]:::box
    end

    L5_LAYER ==>|"Consumes Canonical Evidence"| L4_LAYER
    L4_LAYER ==>|"Session Binding Proof"| L3_LAYER
    L3_LAYER ==>|"Raw Frames (sole boundary)"| L1_LAYER
    L1_LAYER ==>|"InteractionRequested"| L2_LAYER
    L2_LAYER -.->|"Approval response injection"| L3_LAYER

    class L5_LAYER l5;
    class L4_LAYER l4;
    class L3_LAYER l3;
    class L2_LAYER l2;
    class L1_LAYER l1;
```

| Layer | Exclusive Products | Design Principle |
|:---|:---|:---|
| **L5** Flutter Evidence Unification | Single `terminalTransition` consumption; unified state enum | The frontend must only consume canonical evidence produced by the backend; it must never derive or fabricate any lifecycle facts |
| **L4** Continuity & Session Identity | `RuntimeBinding` truth; `UnverifiedBinding` on unproven open | Session resumption must pass real native identity verification; unverified bindings must never be reported as bound |
| **L3** Transport & Process Supervision | `ControlDisposition`; `DeadlinePolicy`; supervision ladder | The transport layer is solely responsible for frame boundaries and process lifecycle; it must never embed business-semantic decisions |
| **L2** Interaction Routing & Settlement | `WaitingForHuman` + one-time Token; fail-closed settlement | On turn termination, all pending interactions must be settled fail-closed; abandoned pending states are forbidden |
| **L1** Parser & Settlement Arbiter | `TurnOutcome`; extended `Transition` vocabulary | Turn completion must be determined by explicit protocol termination signals; silence-based inference or default timeouts must not substitute |

---

## 3. Nine Dedicated Synthesizers (One Product, One Authority)

The core design invariant is: **every observable product in the conversation pipeline must be owned by exactly one synthesizer**. No two code paths may race to determine the same truth.

```mermaid
flowchart TD
    classDef synth fill:#ffffff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e;
    classDef input fill:#f8f9fa,stroke:#868e96,stroke-width:1px,color:#495057;
    classDef output fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;

    HE["Human Event<br/>(persisted fact)"]:::input
    DOOR["Dispatch Door<br/>(admission plan)"]:::input

    HE --> DOOR
    DOOR --> S4["④ Identity Registration<br/>RuntimeBinding truth"]:::synth
    S4 --> S6["⑥ Admission Projection<br/>Composer interactive state"]:::synth
    S4 --> S7["⑦ Frame Transport<br/>raw frames + EOF"]:::synth
    S7 --> S2["② Streaming Parser Entry<br/>live Transition stream"]:::synth
    S2 -->|"Transition::Control"| S3["③ Interaction Gate<br/>WaitingForHuman + Token"]:::synth
    S2 --> PT["PersistentTurn<br/>cursor EventParts"]:::input
    S2 --> S1["① Turn Completion Authority<br/>TurnOutcome"]:::synth
    S7 --> S1
    S1 --> CANON["Canonical Settlement<br/>Dispatch terminal + Event finalized"]:::output

    S5["⑤ Active Turn Control Plane<br/>ControlDisposition"]:::synth -.->|"Cancel / Steer"| S4
    S8["⑧ Follow-up Policy<br/>busy-time input handling"]:::synth -.->|"Steer vs Queue vs Reject"| DOOR
    S9["⑨ Capability Truth<br/>dynamic capability matrix"]:::synth -.->|"Tested capabilities"| S6
```

| # | Synthesizer | Exclusive Product | Synthesis Rule | Design Principle |
|:---|:---|:---|:---|:---|
| **①** | Turn Completion Authority | `TurnOutcome ∈ {still-open, completed, failed, cancelled}` | Protocol termination × Transport EOF × Cancel confirmation × Explicit deadline | Completion must be synthesized from explicit protocol termination signals; silence-based inference or default policies must not substitute |
| **②** | Streaming Parser Entry | Live `Transition` stream + cursor `EventPart` | Driver bytes → Adapter Parser → sole emitter into `PersistentTurn` | Live streaming and terminal settlement must share the same Transition stream; split parsing stories are forbidden |
| **③** | Interaction Gate | `TurnState::WaitingForHuman` + one-time opaque Token (no clock expiry) | Intercept `Control` action, park turn as `still-open`; resume on valid Token response | Pending interactions must never be force-closed by external timeouts; they await explicit user response or fail-closed settlement on turn termination |
| **④** | Identity Registration | `RuntimeBinding` (conversationId × membershipId × dispatchId ↔ adapter-private session) | Full-key exact match for all control/steer/attach | Session identity must never be folded or partially matched; all control operations require full-key verification |
| **⑤** | Active Turn Control Plane | `ControlDisposition ∈ {accepted, no-active-turn, unknown-session, unsupported, transport-unavailable}` | Accept cancel → write `cancel-requested` → await completion authority settlement | Control results must return precise disposition types; different failure causes must not be folded into one error |
| **⑥** | Admission Projection | Composer state (`CanSend \| ReadOnlyLoading \| CanSteer`) | `open` yields `Prepared` only; `send` with verified identity yields `Bound` | Admission state must reflect true binding progress; unverified preparation must never be reported as bound |
| **⑦** | Frame Transport | Standard frames + EOF + over-limit signals | Extract from Stdio/HTTP byte stream | The transport layer is solely responsible for frame boundary extraction; stateful business-semantic parsing must not be mixed in |
| **⑧** | Follow-up Policy | Busy-time input handling (native Steer vs DirectTurn queue vs Reject) | Determined by adapter capability truth + current turn state | User input during active turns must have an explicit disposition path; silent discard is forbidden |
| **⑨** | Capability Truth | Dynamic capability matrix | Derived from control plane tests, parser evidence, and resume probes | Capability declarations must be based on runtime-tested results; static configuration guessing must not substitute |

---

## 4. Four-Tier Vertical Breakdown

As a vertical slice, Conversation defines concrete components and duties across all four horizontal layers:

### ① Tier 1: Flutter Presentation & Application Layer
- **Views & Interactions**:
  - `Composer`: Rich-text editing, send debouncing, and multimodal attachment staging.
  - `AgentConversationWorkspace`: Reactive bubble streaming, pagination, and Markdown rendering.
  - `ProcessBlackboard`: Reasoning steps visualization, tool call progress bars, and diagnostics.
  - `ApprovalModal`: One-shot human approval modal and parameter summaries.
- **Profile Management & Echo**:
  - UI display, editing, avatar/name configuration, and custom prompts for Human and Agent Profiles.
- **Application Controller**:
  - `ClientConversationController` manages reactive interaction state (`_sending`, `_liveTurns`, `draft`, `_dispatchPending`).
- **Progress Reflection**:
  - Listens to backend state machine transitions, reflecting phase updates on the UI progress bar and blackboard in strict lockstep.

### ② Tier 2: Bridging Contract Layer
- **Communication Protocol**:
  - Desktop: `licoup.stdio.v1` strongly typed JSON-RPC method frames.
  - Mobile: C-ABI memory-safe FFI commands.
- **Data Carrier Guarantee**:
  - Losslessly transfers `(Principal + Profile + Event)` payloads.
  - Exposes bidirectional Observer streams for incremental events and terminal evidence.
- **Boundary Constraint**:
  - Prohibits raw CLI argument array (`argv`) pass-through.

### ③ Tier 3: Rust Domain & Infrastructure Layer
- **Domain Ingestion Layer**:
  - Receives `(Human/Agent + Profile + Event)` payloads from the bridge.
  - Validates permissions, checks Membership, and resolves Dispatch targets.
- **Session Manager & Strict State Machine**:
  - **Core Orchestration Engine**: Drives turn lifecycles deterministically.
  - **Controlled Calls**: Converts incoming RPC events into safe function calls targeting Rust Infrastructure and Native OS adapters under state machine constraints.
- **L1 Parser & Settlement Arbiter (`native_agent_parser`)**:
  - Normalizes multiple agent protocols (ACP/RPC/PTY/Codex/OpenCode) into typed transitions.
- **L2 Interaction Routing (`native_agent_interaction`)**:
  - Manages tool approval tokens and fail-closed settlements.
- **Rust Infrastructure Backing**:
  - `ConversationStore` (SQLite WAL): Exclusive persistence for canonical Conversation history and EventParts.
  - `ProviderHistoryAccess` (provider-neutral core boundary): Reads provider-managed retained history only through provider authorization and projects it into the client without replacing the canonical store.
  - `DynamicConfig`: Runtime loading of scan manifests and agent configurations.
  - `SecretCustody`: Unified session key derivation and credential management.
  - `NetworkTransport` & `PTY/TTY`: Streaming HTTP connections and virtual console pipes.

### ④ Tier 4: Native OS Adaptation Layer
- **Terminal & Pipes**:
  - macOS/Linux POSIX PTY (`openpty`, `termios`, `winsize`) and Windows ConPTY / Named Pipes.
- **Supervision & Signals**:
  - OS-level signal handling (`SIGINT`, `SIGTERM`, `SIGKILL`) and process exit reaping.
- **Platform Hardware Custody**:
  - OS Keychains (macOS Keychain, WinCred, D-Bus Secret, Android Keystore, iOS Secure Enclave).

---

## 5. Bidirectional Binding Model & Layer Responsibilities

### 5.1 Bidirectional Interaction Model
Frontend-backend interaction strictly follows **Bidirectional Binding** and Single Source of Truth (SSoT) principles:
- **Downlink Action Flow**: The frontend (Flutter) captures user intent and sends strongly typed request frames (`conversation.message.post`, `interaction.respond`, `agent.conversation.cancel`) as the action initiator down to the protocol layer.
- **Uplink Authoritative Flow**: The backend (Rust functional core) acts as the sole authoritative fact store, driving domain logic, durable persistence, and underlying pipeline scheduling. It pushes live state transitions, streaming deltas (`EventParts`), terminal verdicts, and approval requests reactively to the frontend via Observer streams.

### 5.2 Layer Responsibilities & Boundary Invariants

| Layer | Primary Duties | Data & Control Boundary Invariants |
|:---|:---|:---|
| **Presentation Layer**<br>(Flutter Views & Controllers) | 1. Captures user inputs, gestures, and form interactions;<br>2. Debounces and locks interactive UI state (`_sending = true`, clearing drafts);<br>3. Packs user actions into typed Dart request envelopes;<br>4. Listens to Observer streams to update the process blackboard and chat bubbles;<br>5. Renders sanitized summaries and one-time interactive approval cards. | **State Mirror Invariant**: Serves as a pure rendering mirror of the backend state machine. Updates UI solely from authoritative stream facts; does not maintain local SQLite duplicates, and never fabricates or infers lifecycle facts. |
| **Contract Layer**<br>(Bridging Protocols) | 1. Encodes and decodes bidirectional structured JSON-RPC method frames;<br>2. Manages cross-process (stdio) and cross-language (FFI) memory safety boundaries;<br>3. Provides typed error protection for timeouts and disconnections. | **Stateless Channel Invariant**: Serves as a pure, stateless transport channel passing typed requests and streams without hosting stateful domain logic. |
| **Domain & Infrastructure Layer**<br>(Rust Functional Core & Infra) | 1. Exclusive persistence authority for canonical Conversation facts (sole reader/writer of SQLite/WAL);<br>2. Exclusive dispatch admission authority (resolves `@mention`, group strategy, and memberships);<br>3. Exclusive completion arbiter (L1 normalization and terminal verdict);<br>4. Exclusive process lifecycle supervisor (startup, input pipes, Grace Period ladder, SIGTERM/KILL);<br>5. Exclusive interaction routing (one-time scoped token generation and fail-closed settlement). | **Fact Authority Invariant**: Operates as the system's sole authoritative source for canonical Conversation facts. Provider-managed history remains an authorized external source and is never silently merged into that authority; every canonical event exposed to the frontend is backed by an immutable persisted record; Exact Resume session identity invariants remain enforced. |

---

## 6. Direct Chat Base & Group Orchestration Architecture

In product and domain architecture design:
1. **Direct Chat (1:1) is directly user-facing**: Provides an immersive single-agent workspace (dedicated model tuning, system prompts, and direct session state machines);
2. **Group Chat is also directly user-facing**: Provides a collaborative multi-human and multi-agent workspace (member rosters, @mentions, Flywheel graph bindings, and Assistant cards);
3. **Underlying Architectural Invariant**: **Direct Chat is the core atomic execution building block, while Group Chat is an Orchestration & Coordination Encapsulation Layer layered atop multiple underlying direct chat execution pipelines**.

```mermaid
flowchart TB
    %% Styling definitions
    classDef userFacing fill:#e7f5ff,stroke:#1971c2,stroke-width:2px,color:#0c4a6e,font-weight:bold;
    classDef groupOrch fill:#fff4e6,stroke:#e8590c,stroke-width:2px,color:#7c2d12;
    classDef directPipeline fill:#f3f0ff,stroke:#6741d9,stroke-width:2px,color:#3b0764;
    classDef storeBox fill:#ebfbee,stroke:#2b8a3e,stroke-width:2px,color:#1e3a1f,font-weight:bold;
    classDef itemBox fill:#ffffff,stroke:#adb5bd,stroke-width:1.5px,color:#212529;

    subgraph USER_ENTRIES["[User-Facing Entry Points] — Both Direct and Group Chats are Directly Exposed to Users"]
        direction LR
        ENTRY_DIRECT["Entry A: 1:1 Direct Chat Workspace<br/>• Directly user-facing immersive agent UI<br/>• Dedicated parameters · Single prompt · Direct execution"]:::userFacing
        ENTRY_GROUP["Entry B: Multi-Agent Group Workspace<br/>• Directly user-facing collaborative UI<br/>• Member roster · @mentions · Graph bindings · Assistant cards"]:::userFacing
    end

    subgraph GROUP_LAYER["[Group Chat Dedicated: Multi-Party Orchestration & Encapsulation Layer]"]
        direction TB
        subgraph G_MODULES["Group-Specific Orchestration Facets"]
            direction LR
            G_MEM["1. Multi-Membership & Access<br/>(Membership Registry)"]:::itemBox
            G_DISPATCH["2. Dispatch & Addressing<br/>(@mention Parsing / Context Slicing)"]:::itemBox
            G_STRATEGY["3. Workflow & Goal Governance<br/>(Flywheel Graph / Assistant)"]:::itemBox
            G_AGGREGATE["4. Multi-Event Aggregation<br/>(Shared Timeline Projection)"]:::itemBox
            G_MEM --> G_DISPATCH --> G_STRATEGY --> G_AGGREGATE
        end
    end

    subgraph DIRECT_PIPELINES["[Core Base: Atomic Direct Chat Execution Pipelines]"]
        direction LR
        subgraph PIPE_A["Agent Alpha Direct Pipeline"]
            direction TB
            A_FSM["Dedicated State Machine"]:::itemBox
            A_PROF["Alpha Dedicated Profile"]:::itemBox
            A_PTY["PTY / Subprocess Pipe"]:::itemBox
            A_FSM --- A_PROF --- A_PTY
        end
        subgraph PIPE_B["Agent Beta Direct Pipeline"]
            direction TB
            B_FSM["Dedicated State Machine"]:::itemBox
            B_PROF["Beta Dedicated Profile"]:::itemBox
            B_PTY["PTY / Subprocess Pipe"]:::itemBox
            B_FSM --- B_PROF --- B_PTY
        end
        subgraph PIPE_N["Agent N Direct Pipeline"]
            direction TB
            N_FSM["Dedicated State Machine"]:::itemBox
            N_PROF["Agent N Dedicated Profile"]:::itemBox
            N_PTY["PTY / Subprocess Pipe"]:::itemBox
            N_FSM --- N_PROF --- N_PTY
        end
    end

    subgraph FACT_STORE["[Durable Fact Store]"]
        DB["ConversationStore (SQLite WAL)<br/>Same conversations table · Unified Events / EventParts fact authority"]:::storeBox
    end

    %% Execution flows
    %% 1. Direct route: User interacts directly with a single agent, bypassing group orchestration
    ENTRY_DIRECT ==>|"[Direct Route] User chats directly with a single agent"| PIPE_A

    %% 2. Group route: User posts in group chat, going through orchestration layer before dispatching
    ENTRY_GROUP ==>|"[Group Route] User posts in group chat"| GROUP_LAYER
    G_DISPATCH -.->|"Slices & Dispatches"| PIPE_A
    G_DISPATCH -.->|"Slices & Dispatches"| PIPE_B
    G_DISPATCH -.->|"Slices & Dispatches"| PIPE_N

    %% 3. Results aggregation
    PIPE_A -.->|"Returns Events"| G_AGGREGATE
    PIPE_B -.->|"Returns Events"| G_AGGREGATE
    PIPE_N -.->|"Returns Events"| G_AGGREGATE
    G_AGGREGATE ==>|"Projects to Shared Timeline"| ENTRY_GROUP

    %% 4. Persistence
    PIPE_A ==>|"Commits Facts"| DB
    PIPE_B ==>|"Commits Facts"| DB
    PIPE_N ==>|"Commits Facts"| DB
```

### Direct Route vs Group Route Comparison:

| Architectural Dimension | 1:1 Direct Chat (Direct User Service) | Multi-Agent Group Chat (Orchestration Encapsulation) | Design Principles |
|:---|:---|:---|:---|
| **User Entry Point** | **Dedicated Direct Workspace**: Immersive focus, single-agent parameters, and prompt configurations. | **Dedicated Group Workspace**: Member rosters, @mention auto-complete, Assistant cards, strategy versions. | Both UI entries serve users directly with composable components. |
| **Dispatch Path** | **Direct Path**: User post $\to$ bypasses group orchestration $\to$ immediately activates agent FSM and PTY. | **Orchestrated Path**: User post $\to$ group dispatch door (@mention/graph) $\to$ slices & dispatches to $N$ direct pipelines. | Direct chat optimizes for zero-latency execution; group chat optimizes for context coordination. |
| **Pipeline Reuse** | Exclusively uses 1 direct chat execution pipeline (FSM + Profile + PTY/Stream). | Reuses and concurrently orchestrates $N$ direct pipelines for pipelined or parallel execution. | **The direct chat execution pipeline is the sole underlying execution engine**. Group chat does not reinvent execution. |
| **Timeline Projection** | Single timeline committed to SQLite and streamed directly to UI. | EventParts from underlying pipelines are merged by the aggregator and projected into the shared timeline. | **Unified Durable Store**: Both share the exact same `conversations` and `events` schema. |

---

The group roster is a projection of active Agent Memberships. `principal.agentId`
identifies the adapter and matches `TargetCandidate.target`; a candidate record's
`id` is not a Membership identity. A missing discovery result keeps the member
visible as unavailable. Retired `kimi` and `kimi-desktop` adapters are hidden
from the current roster without removing Memberships or historical Events;
`kimi-code` remains distinct. An empty roster does not block reading the timeline
or editing a message. Roster clicks and mention completion use the Membership's
display name, falling back to its agent ID, matching native addressing aliases.

## 7. Human / Agent Dedicated Profile Abstraction

In the Conversation architecture, **every participant—whether Human or Agent—must and does have a dedicated Profile data encapsulation**.

```mermaid
sequenceDiagram
    autonumber
    actor User as User
    participant UI as Flutter View (Profile UI)
    participant Ctrl as Frontend Controller
    participant Bridge as Bridge Contract
    participant Ingestion as Rust Domain Ingestion
    participant SM as Rust Session State Machine

    User->>UI: 1. Configure / Select Agent or Human Profile (model/role/prompt)
    UI->>Ctrl: 2. Bind Profile to active Membership
    Ctrl->>Bridge: 3. Downlink payload: (Principal + Profile + Event)
    Bridge->>Ingestion: 4. Unpack strongly typed struct & preflight check
    Ingestion->>SM: 5. Inject into Session State Machine as execution context
    SM->>SM: 6. Drive subprocesses/APIs according to Profile parameters
```

- **Full-Stack Lifecycle**:
  - **Frontend**: Full editing and live echo (avatars, display names, context preferences, security levels).
  - **Contract Layer**: Transmitted losslessly as strongly typed schema fields.
  - **Backend**: Ingested by the domain layer to drive state machine transitions and execution policies.

---

## 8. Configuration-Owned Conversation Lifecycles

`crates/licoup-conversation/resources/state-machines.json` is the single source
for the durable Conversation transition relations. The crate build compiles its
six selected machines into private Rust types and transition tables: direct
turns, sends, memberships, dispatches, subagent claims, and dispatch deliveries.
The generated types retain the persisted kebab-case names used by current
callers.

The generated tables answer which state follows a given state and event. They do
not perform database, process, network, or scheduling work. The Conversation
store owns guards, effects, and transactions in `src/store/lifecycle.rs` and its
repositories; each durable state change is committed with its associated store
effects. This keeps the transition relation declarative while the runtime
remains responsible for executing it.

| Durable record | Generated state type | Persisted lifecycle |
|:---|:---|:---|
| Direct turn | `TurnState` | `pending`, `claimed`, `running`, `waiting-for-human`, then a terminal state |
| Send | `SendState` | `sending` to `delivered` or `failed` |
| Membership | `MembershipStatus` | `active` or `left` |
| Dispatch | `DispatchState` | `accepted`, `running`, cancellation request, and terminal state |
| Subagent claim | `SubagentDispatchClaimState` | claim, execution, reconciliation, and terminal state |
| Dispatch delivery | `DispatchDeliveryState` | `pending`, `delivering`, `delivered`, or `failed` |

## 9. Durable Status Reflection

The Rust Conversation store is authoritative for durable records and committed
events. The UI renders those results and projections; it does not consume a
separate generated-transition stream or commit lifecycle changes itself. Local
drafts and other presentation state remain UI concerns, while persisted status
comes from the Conversation authority. A disconnected observer does not by
itself change a turn's durable status; recovery reads the stored lifecycle and
applies the Conversation recovery rules.

---

## 10. End-to-End Standard Sequence Flows

### ① User Message Two-Phase Dispatch Flow

`conversation.dispatch.after-post` also accepts an optional boolean
`suppressAssistant` (default `false`). It excludes the designated Assistant
from direct targeting and steering for that message. Rust still derives mentions
from the committed Event, persists shared history and returns existing attachable
turns. The [Assistant control](../functionality/ADAPTIVE-FLYWHEEL.md#group-conversation-start)
owns the UI choice; independently bound Flywheel execution is unchanged.

```mermaid
sequenceDiagram
    autonumber
    actor User as User
    participant UI as Flutter View & Controller
    participant Bridge as Bridge Contract
    participant Domain as Rust Domain Store
    participant Dispatch as Rust Dispatch Door
    participant Driver as Agent Driver & Process

    User->>UI: 1. Input message in Composer and click Send
    Note over UI: UI State Locked:<br/>• _sending = true<br/>• Input locked against duplicate clicks

    UI->>Bridge: 2. Downlink action: conversation.message.post { conversationId, authorMembershipId, content }
    Bridge->>Domain: 3. Route to persist_posted_message
    Domain->>Domain: 4. [Backend Persistence] Commit finalized Human Event to SQLite
    Domain-->>Bridge: 5. Return confirmation { eventId }
    Bridge-->>UI: 6. Confirmation received
    Note over UI: Release Draft:<br/>• _draft = ''<br/>• Refresh local list

    UI->>Bridge: 7. Downlink action: conversation.dispatch.after-post { conversationId, eventId, suppressAssistant? }
    Bridge->>Dispatch: 8. Pass committed (conversationId, eventId)
    Dispatch->>Domain: 9. Read text from DB, parse @mention / bound Flywheel Graph
    Dispatch->>Dispatch: 10. Register Dispatch(accepted) + unfinalized Agent Event slot
    Dispatch->>Driver: 11. Launch Agent process and attach listeners
    Dispatch-->>Bridge: 12. Return active turn handles { turns: [turnHandle] }
    Bridge-->>UI: 13. Attach _liveTurns and enter stream observation
```

---

### ② Realtime Stream Generation & Attachment Flow

```mermaid
sequenceDiagram
    autonumber
    participant Driver as Agent Process
    participant L3 as L3 Transport Supervisor
    participant L1 as L1 Parser & Arbiter
    participant Domain as Domain Persistence
    participant UI as Flutter Frontend

    Driver->>L3: 1. Emit raw bytes / JSON frames
    L3->>L1: 2. Forward raw frames across parser boundary
    L1->>L1: 3. Reduce into Typed Transitions (Reasoning, ToolCall, ContentPart, Usage)
    L1->>Domain: 4. Append EventPart to unfinalized Agent Event
    Domain->>Domain: 5. Advance cursor watermark & commit
    Domain-->>UI: 6. Stream incremental event uplink (Observer Stream)
    Note over UI: UI Updates:<br/>• Stream text into bubble<br/>• Render reasoning in blackboard

    Driver->>L3: 7. Process exit / protocol EOF
    L3->>L1: 8. Deliver EOF / finish signal
    L1->>L1: 9. Settlement arbiter resolves Terminal outcome
    L1->>Domain: 10. Finalize Agent Event
    Domain-->>UI: 11. Uplink terminal evidence
    Note over UI: UI clears busy lock, turn completes cleanly
```

---

### ③ Blocking Human-in-the-Loop Approval Flow

```mermaid
sequenceDiagram
    autonumber
    participant Driver as Agent Process
    participant L1 as L1 Parser
    participant L2 as L2 Interaction Router
    participant Domain as Domain Persistence
    participant UI as Flutter Frontend
    actor User as User

    Driver->>L1: 1. Tool execution requested
    L1->>L2: 2. Detected as InteractionRequested; issue scoped one-shot Token
    L2->>Domain: 3. Set turn state to WaitingForHuman
    Domain-->>UI: 4. Uplink approval card event (sanitized parameter summary)
    Note over UI: Popup interactive approval card

    User->>UI: 5. Click "Approve" or "Reject"
    UI->>L2: 6. Downlink action: interaction.respond { token, approved: true/false }
    Note over L2: Validate Token authenticity & single-use invariant
    L2->>Driver: 7. Inject response into Agent stdin
    L2->>Domain: 8. Resume turn state to Processing
    Domain-->>UI: 9. Uplink resumed state
```

---

### ④ Turn Cancellation & Failure Safety Flow

```mermaid
sequenceDiagram
    autonumber
    actor User as User
    participant UI as Flutter Frontend
    participant Bridge as Bridge Contract
    participant L3 as L3 Supervision
    participant L1 as L1 Arbiter
    participant Domain as Domain Persistence

    User->>UI: 1. Click "Stop / Cancel"
    UI->>Bridge: 2. Downlink action: agent.conversation.cancel { turnHandle }
    Bridge->>L3: 3. Trigger process supervision ladder
    Note over L3: Supervision Ladder:<br/>① Graceful cancel frame<br/>② Wait grace period<br/>③ Send SIGTERM<br/>④ Force SIGKILL if still alive

    L3->>L1: 4. Report process interrupt
    L1->>L1: 5. Arbiter produces Cancelled Terminal Transition
    L1->>Domain: 6. Commit final state: Cancelled
    Domain-->>UI: 7. Uplink cancelled evidence
    Note over UI: Restore Composer to editable
```

---

## 11. Error Handling & Consistency Invariants

1. **Observer Disconnection $\ne$ Turn Failure**:
   - UI navigation or window minimization disconnecting the Observer stream **never cancels the underlying Agent turn**.
   - Rust core preserves in-flight progress; reopening the view with `conversationId` and the last watermark replays all subsequent `EventParts` cleanly.
2. **No Frontend Guesswork**:
   - Network dropouts, crashes, or timeouts must be explicitly diagnosed and committed by Rust L1/L3 as typed error codes. Flutter strictly renders localized messages matching those codes.

---

## 12. Trusted History and Recovery Boundary

The group sidebar binds directly to the group's own associated-session subset.
`conversation_native_sessions` is the membership authority and retains
each confirmed `(conversationId, membershipId, nativeSessionId)` once; replacing
the current runtime binding, changing a model, or a Membership leaving does not
erase earlier associations. The same Agent and native session can belong to
multiple groups only when each group has its own recorded binding. Schema 18
is the current Conversation layout. The atomic conversion from published schema
12 adds the current bindings and explicit dispatch provenance, and records the
known owner of old runtime source links. Unpublished schemas 13 through 17 are
not supported migration inputs; newer or malformed metadata is rejected before
schema changes. Missing ownership evidence never creates a relationship.

Desktop `conversation.get` opts into `includeNativeSessionReferences: true` to
receive local `membershipId`, `agentId`, and `nativeSessionId` lookup facts. The
ordinary get, portable Conversation, export, and MCP/protocol contracts remain
unchanged; runtime paths and dispatch handles are not included. Application
state maps these references directly into a dedicated group session collection.
The list consumes that collection's projection. Missing metadata is fetched by
exact native identity with at most four reads in flight; each completed read
updates the projection immediately. Selection, refresh and message paging use
the same group collection and exact readers. Group state neither enumerates nor
filters the Agents' browse catalogs, and never writes its subset back into them.
Unrelated browse changes do not rebuild or replace the group collection. Group
switches and binding changes invalidate stale responses. No recorded relationship
means an empty group collection; product names, projects, models and timestamps
cannot create membership. Unreadable native rows are not replaced with invented
conversations. Existing message paging and execution remain shared mechanisms.

The canonical `conversation.events.page` action returns ascending events with a
default page size of 20 (maximum 100). `latest: true` reads the newest retained
events; `beforeSequence` reads the preceding page with an exclusive sequence
anchor. Both use descending indexed reads and reverse only the selected page.
`afterSequence` remains an independent forward read for incremental updates and
exact event anchors; omitting all selectors reads forward from the beginning.
Active selectors are mutually exclusive and invalid combinations return
`invalid_request`. `hasEarlier` and nullable `nextBeforeSequence` describe earlier
retained rows; the latter is the first returned sequence only when earlier rows
exist. `nextCursor` remains the last returned sequence as a string for forward
reads, or null for an empty page. `totalCount` counts retained rows and is never a
sequence cursor: deleted events can leave gaps. The desktop opens the latest 20
events and requests 20 earlier events when scrolling back; exact task-card anchor
reads remain separate from that contiguous page window.

Canonical Conversation history remains owned by LicoUp's local
`ConversationStore`. Provider-managed cloud history is a separate retained
object source and may be projected into the conversation experience only after
provider authorization.

The boundary is fixed as follows:

1. Provider authorization makes retained provider-managed history readable by
   default. The default history read does not call a recovery key.
2. Client-side encryption for provider-managed history is an explicit opt-in.
   It does not silently change the default readable path.
3. History recovery restores all retained objects still available under the
   authorization. Provider access rules continue to apply; recovery cannot
   bypass access or recreate missing, deleted, expired, or otherwise unavailable
   objects.
4. Identity recovery material is authenticated separately and never locks
   default reads. Replacement-device recovery prepares identity authority and
   complete available history for one atomic caller-owned commit; identity
   material alone does not recreate absent history.
5. LicoUp does not route or store this history through a Station. Malicious
   Stations remain outside the history path, which makes no mandatory notary
   or endpoint-evidence promise.

```mermaid
flowchart LR
    U["User"] --> A["Provider authorization"]
    A --> H["Provider-managed retained history"]
    H --> L["LicoUp local projection"]
    R["History recovery"] --> H
    E["Explicit client encryption"] -.-> H
    I["Identity recovery"] -.-> K["Endpoint identity and keys"]
    S["Malicious Station<br/>transport only"]
```


## 13. Local Execution Inspection

`agent.conversation.execution` is a read-only local stream for one exact
`conversationId`, `membershipId`, and `turnHandle`. The handle is the native
dispatch ID. Adapter session IDs and late-arriving adapter turn IDs do not
replace it. The store resolves and verifies the corresponding Agent Event
before returning any records. Live bubbles carry that execution reference.
Native history readback may attach it only through an explicit provider key
recorded for the same Agent and native session. The dispatch owns this small
provenance association; conflicting matches remain unbound. Codex app-server
turn acknowledgements and rollout turn contexts provide this association.
Host-generated transport IDs, text similarity, timestamps and the currently
active Agent never supply a missing provider key. Older unbound history remains
readable without an invented execution reference.

The stream replays retained records in order, reports when historical reading
has caught up, and then observes new records through the existing wakeable turn
observer. Request and terminal payloads belong to the dispatch row. Runtime
records reuse the existing cursor-bearing EventPart chunks; reading joins those
stored chunks without decoding and re-encoding their original text. There is no
second transcript or separate runtime-frame store.

The local capture observer records available application-protocol input before
parser projection and output after encoding at the send boundary. It preserves
unknown fields and tool or intermediate content that the public projection
omits. Source and direction belong to record metadata, not an added prefix in
the original text. Text stays unchanged; non-UTF-8 bytes use an explicitly
labelled reversible encoding. Normalized runtime records and protocol records
remain distinguishable. This does not reconstruct bytes or metadata discarded
before capture was introduced.

Capture belongs to the exact dispatch. Worker threads receive an explicit
observer, and persistent transports bind their reader for the active dispatch
rather than assigning queued frames when a later turn consumes them. Unbound
traffic is not assigned to the most recent turn. Capture closes before terminal
settlement. A capture write failure marks the retained terminal evidence as
incomplete without changing the Agent's actual result or cancelling its work.

Execution cursors are opaque increasing values, distinct from public runtime
cursors and transport sequence numbers. They preserve the persistent order of
captured request, runtime records (including user-speech metadata), and terminal
payload. Gaps are valid. `afterCursor` is exclusive; transport parts of one large
record are reassembled completely before advancing it. Reconnecting observers
therefore continue without duplicating or truncating records. Missing payloads
in older dispatches remain missing; a new reader cannot recreate them.

The explicit local inspection path preserves available raw record contents,
including unknown nested fields, arrays, request metadata, intermediate output,
and terminal evidence. It does not pass through the public assistant-summary
redactor. Ordinary transcript, approval, export and remote boundaries retain
their own contracts. This method is not an MCP capability. It neither reads
protected credentials nor adds an external runtime-data destination.

Observation is separate from execution. Closing the viewer or disconnecting
its stream never steers, cancels, restarts, or completes a turn. A terminal
record remains readable after host restart or registry eviction. If a retained
nonterminal dispatch has no observable runtime, the reader reports that
observation is unavailable and preserves the stored execution status; it does
not invent an interruption or wait indefinitely for an absent observer.

The presentation binding exposes immutable execution values through the shared
projection and intent contracts. Opening and closing a viewer attaches and
detaches its observation in the composition layer. Frontend widgets receive no
native reader or transport lifecycle port. Record lists retain their immutable
prefix across incremental updates, so observation does not copy the transcript
or create a second execution authority.

`agent.conversation.execution.detach` releases one observer using its original
stream request/workflow identity and exact execution scope. It wakes the
observer and closes that stream without terminating the shared conversation
transport or the Agent. Late frames remain isolated to the detached request.

The frontend mirrors the admitted dispatch while waiting for reply text. Its
waiting bubble is a presentation of existing send/dispatch facts, not a new
persisted assistant message or a second lifecycle authority. Thinking and tool
records do not satisfy the first-reply condition. Actual reply text or a real
terminal outcome ends the wait. The visual and search behavior is owned by the
[design system](../functionality/DESIGN-SYSTEM.md#conversation-loading-and-hierarchy).

## Peer message identity boundary

The accepted peer-conversation target preserves original message identity, author,
represented principal, source endpoint, forwarder and local ordering as distinct facts.
Transport receipt, endpoint receipt, application admission, read status and agreed
acceptance are not interchangeable. Edits and deletions reference their causal source
and revision; synchronization and recovery cannot resurrect deleted content or give a
forwarder the original author's authority.

Mergeable communication history does not make execution permission, budget admission
or revocation eventually consistent. These remain decisions of their existing owners.
[Workflow control](ASSISTANT-WORKFLOW-CONTROL.md) owns effect admission and
[the security boundary](SECURITY-AND-DATA-BOUNDARY.md) owns result settlement after
revocation. Peer integration must verify these distinctions; local messaging alone is
not proof of them.
