# Bridging Contract Layer — Client-Native Interaction Boundary

[Documentation](../README.md) · [Architecture](README.md) · [Architecture (zh-CN)](README.zh-CN.md)

This document defines the technical specification for **Tier 2: Bridging Contract / RPC Protocol Layer**, isolating **Tier 1: Flutter Presentation Layer** from **Tier 3: Rust Functional Core Layer**.

## Communication Contracts

1. **Desktop stdio RPC**:
   - Flutter reaches the persistent Rust native host through the `licoup.stdio.v1` bidirectional frame.
   - Desktop replies are decoded once per envelope and shared by request routing and the generated protocol validators. Large envelopes decode in a background Dart isolate; the paused stdout stream preserves frame order and applies backpressure until decoding completes. EOF is handled after accepted frames drain, so process exit cannot discard a final reply still being decoded. No functional request deadline is added.
   - Application composition binds the Agent conversation service and canonical conversation service to `AgentConversationNativePort` and `ClientConversationNativePort`. Their semantic operations carry session and turn identity types plus opaque native payloads; services do not accept a command runner for stateful operations.
   - The platform implementation alone encodes these operations as structured method-and-parameter frames. Agent operations use generated `agent.conversation.*` methods; canonical requests use generated `client.conversation.execute` with the requested action in its payload. `ConversationProtocolMethod`, generated from the native protocol registry, remains the method authority.
   - Stateful operations **strictly never use a CLI argument array** as their client-to-native transport.
   - Generic command and process entry points reject the stateful `agent conversation` and `conversation` namespaces. Disabling persistent stateless execution or injecting a one-shot executor cannot redirect a stateful operation to a subprocess. Test fixtures inject semantic ports or structured transports explicitly.
2. **Mobile Platform FFI**:
   - On Android and iOS, communication passes through C-ABI FFI command boundaries (`android_ffi.rs`, `ios_ffi.rs`), invoking Rust functional core services in-process without shell processes.
   - The desktop local conversation port reports an unavailable runtime on mobile. Mobile conversation routing continues through the approved secure relay and FFI paths; it never launches the desktop host.
3. **Stateless Commands**:
   - The same RPC frame carries bounded stateless queries as `method: "execute"` with an argument array (e.g., catalog and target queries parsed via the public CLI command model).
   - Explicit catalog, report, skill, pairing, and native history queries share at most four lazily opened persistent read connections. Each available connection takes the next eligible query from the shared priority queue and publishes its result independently. The transport does not cache results or impose a query deadline. Native cache publication preserves concurrent readers; mutations and process-owned services keep their ordered lane, and conversation observers keep their host connection. Disposal rejects new reads, drains accepted work, and closes each connection as it becomes idle.
   - Explicitly injected executors and tests may use one-shot stateless execution; it is never the transport for a stateful turn.
4. **Security & Secrets**:
   - Credential create and update requests rewrite private input onto stdin before process launch.
   - Secret values never enter command-line arguments, reports, or the public frame projection.

## Implementation Authorities

The implementation authorities for the Bridging Contract Layer are:
- **Client Native Transport**: `apps/desktop/lib/src/platform/native_client/`
- **Semantic Conversation Ports**: `apps/desktop/lib/src/contracts/conversation_native_port.dart`
- **Platform Conversation Encoding**: `apps/desktop/lib/src/platform/native_client/native_conversation_port.dart`
- **Client Agent Services**: `apps/desktop/lib/src/backend/features/agents/services/`
- **Canonical Conversation Service**: `apps/desktop/lib/src/backend/features/conversations/services/client_conversation_service.dart`
- **Native Frame Router**: `crates/licoup-native/src/bin/licoup/stdio_rpc/`
- **Mobile FFI Bridges**: `crates/licoup-native/src/ffi/` (`android_ffi.rs`, `ios_ffi.rs`)

## Refactor requirements for control and causal observation

Control priority must extend through transport, frame decoding, preparation and
store admission. Reuse the existing cancellation channel. A priority queue alone
does not prove responsiveness when a large frame, synchronous parser or long
transaction blocks it; measure the actual path under those loads.

Correlate the original user action through IPC, admission, queued permit, Agent first
event, persistence, projection, Dart preparation and the actual displayed frame.
Separate waiting from computation and retain causal links across asynchronous work
and multiple predecessors. Use bounded, optionally sampled telemetry. Business
receipts, effect identity and original history remain durable regardless of telemetry
sampling. These measurements do not create another event or execution authority.

These are accepted migration requirements. Existing trace hooks or a passing parser
test alone do not establish end-to-end control latency or frame performance.
