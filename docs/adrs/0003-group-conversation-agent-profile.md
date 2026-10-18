# ADR 0003: Group-conversation Agent Profile

- `context` — A group conversation's Agent member currently has no persistent
  profile. Its identity is only the `Membership` record (principal id, display
  name, access, status; `crates/licoup-native/src/domain/client_conversation/mod.rs`
  `Membership`) plus the technical session binding
  (`runtime_bindings`: session id, path, working directory). There is no
  per-member place for any durable, inspectable per-conversation state.
  Related concepts exist but do not fill this gap: `Conversation Role`
  (CONTEXT.md) is an Adaptive Flywheel strategy-domain concept (an ordered
  eligible-Agent pool), and `licoProfile` exists only as a send-time bind
  field of the single-Agent conversation and is read only by the Lico Agent
  driver (`crates/licoup-native/src/platform/lico_agent_driver/execution.rs`).
  Group dispatch does not carry any profile field
  (`crates/licoup-native/src/domain/client_conversation/service.rs`
  `run_direct_turn` parameters).
- `decision` — Every Agent member of a group conversation has a persistent
  Profile: an abstract per-conversation, per-membership concept with fields
  that are intentionally unspecified for now (the concrete fields, format,
  and default usage are a follow-up decision). The Profile is endpoint-local
  state (it never leaves the client boundary) and is not part of the wire
  contract. A candidate usage under consideration is recording that
  conversation's long-term memory; whether it becomes the default usage is
  left open.
- `rationale` —
  - A durable, inspectable per-member home for conversation-scoped state is
    missing today; only the framework-owned native session exists, which is
    opaque and not shaped for conversation-level context.
  - Deciding the concept first and the fields later keeps the durable
    structure in place without locking premature semantics.
  - Keeping it endpoint-local preserves the client's authority over its
    protected content.
- `alternatives` —
  - Rely solely on the Agent's native session: rejected because the session
    is framework-owned, opaque, and not portable across lanes.
  - A single conversation-level state (not per member): rejected because each
    Agent member needs its own persistent per-conversation context.
  - Persist the Profile in the wire contract or station: rejected because the
    Profile is endpoint-local protected content.
- `consequences` —
  - A persistent per-(conversation, membership) Profile storage is added to
    the client conversation database, including migration for existing
    conversations.
  - Concrete fields, format, and default usage (including the long-term
    memory candidate) are deferred to a follow-up decision and are not
    implied by this decision.
- `status` — implemented and verified, 2026-08-23.
