# ADR 0004: Assistant-authored flexible workflows

- `context` — ADR 0003 established a persistent, endpoint-local Profile
  for every Agent member of a group conversation but deliberately left its
  concrete fields and usage to a follow-up. The client also ships a fixed
  delivery generation (`delivery_plan`, `delivery_scheduler`,
  `delivery_state`, `lico_delivery_*`) that sequences Designer/Worker/Reviewer
  roles and private Plan state. That generation is not the product-owned
  Adaptive Flywheel Graph runtime, duplicates workflow lifecycle concepts, and
  must be retired so one Assistant Membership can own a user goal through
  direct work or a bounded temporary Graph without a second execution
  authority.
- `decision` —
  - One visible active Agent Membership may be designated as the long-lived
    Assistant of a Conversation. The designation is explicit, stored on the
    Conversation, and unchanged by temporary workflow creation, completion,
    failure, observer detach, or strategy capsule changes. Existing multi-Agent
    groups with no explicit designation remain undesignated; ambiguity is never
    resolved by silent assignment.
  - Every active Agent Membership has a persistent, bounded, revisioned
    Profile intent that is endpoint-local and per-(conversation, membership).
    Model availability, Boolean capabilities, actual model price, coding
    score, Skill availability, runtime environment and readiness are never
    copied into a second catalog: they are derived per request from their
    existing owners (`targets`, `provider_model_pricing`,
    `agent_intelligence_catalog`, `skill_hub`, host readiness). Derived facts
    are read at most once per request/revision through a request-scoped cache
    and projected as allowlisted opaque ids, enums, numbers and booleans only.
    An unknown required fact rejects; an unknown optional price or score
    ranks after known values and stays visibly unknown.
  - The designated Assistant references the product-owned `licoup-guide`
    software-use Skill at `crates/licoup-native/resources/licoup-guide/SKILL.md`.
    It shares guidance with provider registration; registration remains an
    explicitly approved operation. Development processes and third-party
    planning Skills are maintained outside the client.
  - Candidate discovery hard-filters Membership, Authority, privacy/location,
    readiness, model, Skill, environment and capability constraints, then
    applies one stable lexicographic order. No weighted score, second route
    catalog, or hidden transcript exists. The Assistant binds exact
    `conversationId` plus `membershipId` values, and every binding plus its
    Authority subset is independently revalidated immediately before durable
    run admission so model intent cannot turn stale or ineligible facts into
    permission.
  - Temporary workflows use the existing Adaptive Flywheel compiler, reducer,
    parallel-frontier limits and effect ports, marked assistant-temporary and
    omitted from the imported strategy catalog. All locally discoverable
    errors (structure, quota, model, Membership, Skill, environment,
    capability, readiness, Authority) are returned before the first Agent,
    script or external effect. Dynamic failures settle through the existing
    typed runtime failure path: the tool result carries a stable code, stage,
    retryability and privacy-safe recovery class, is returned once, and has no
    elapsed-time terminal transition. After a typed failure the same Assistant
    may work directly or submit a later Graph; durable reconciliation never
    reissues an effect whose identity already exists.
  - Direct and group messages addressed to the Assistant use the same
    Membership-scoped PersistentTurn stream, attach, steer, resume, cancel and
    tool semantics as one-to-one Agent conversation; addressing only selects a
    Membership and observer loss never cancels work. Explicit addressing of
    another active Membership remains the direct primitive. Every temporary
    Subagent is an explicit active Membership whose text, reasoning, tool,
    artifact, diagnostic and terminal output appends to the same Canonical
    Conversation Event/Part timeline.
  - The group composer exposes one circular Assistant control, never a
    strategy actor chip. Its active state addresses future messages to the
    designated Assistant; pausing only suppresses future dispatch and never
    cancels, interrupts, or rewrites an already running PersistentTurn or
    Graph. The control has no hover editor or extra vertical container and is
    exactly as tall as the adjacent input capsule. An undesignated group opens
    explicit Assistant configuration instead of silently choosing a
    Membership.
  - Imported strategy capsules remain workflow tools adjacent to, but never
    identical with, the Assistant. Their collapsed projection reports the
    Assistant's current work state. The Assistant may start, steer, replace,
    or stop a workflow through the existing Graph control surface while
    retaining the only user-dialogue ownership.
  - Assistant Agent, model, and reasoning effort are edited through the same
    target/model/reasoning catalog component used by Adaptive Flywheel actor
    bindings. The Assistant is the first card in the Adaptive Flywheel editor
    and exposes Agent, Model, and Thinking Effort as three adjacent columns;
    it is not configured from a composer popover. Model and reasoning intent
    are stored in the Assistant's revisioned Profile and passed through the
    existing direct-turn dispatch; the card remains independent of imported
    workflow bindings and the client does not create a second runtime-settings
    store.
  - Canonical participant-flow bubbles distinguish the Assistant from
    Subagents. Subagents remain chronological Conversation participants and
    show the model and reasoning effort from the selected workflow binding;
    the Assistant bubble omits those execution details because its Profile is
    independently editable.
  - The fixed delivery generation is removed in one cutover: `delivery_plan`,
    `delivery_scheduler`, `delivery_state`, `delivery_routes`, `lico_delivery_*`
    and fixed Designer/Worker/Reviewer sequencing leave the source, tests,
    module catalog, MCP surface and formal documentation with no compatibility
    read, dual write, fallback command or public schema. Direct Membership
    delegation (`lico_subagent_*`) and topology-neutral Graph execution remain.
- `rationale` —
  - A long-lived Assistant is the single accountable owner of a user goal;
    delegating to a temporary Graph is one tool it may use, not a new role.
  - Deriving facts from existing owners keeps one source of truth and makes
    receipts explainable; copying would create drift and a second authority.
  - Bounding the Graph to the existing compiler/reducer preserves the tested
    effect gate and makes "no effect before preflight" locally provable.
  - Removing the delivery generation entirely avoids a compatibility tax and
    a second workflow lifecycle; the Adaptive Flywheel Graph already covers
    durable execution.
- `alternatives` —
  - Reuse the delivery scheduler as the temporary-Graph runtime: rejected
    because it carries fixed-role sequencing, private Plan state and a second
    route catalog that the Assistant boundary explicitly forbids.
  - Store derived Profile facts in the Profile row: rejected because the
    owners can change and a copied fact would become stale authority.
  - Auto-designate an Assistant for existing multi-Agent groups: rejected
    because it would change existing group behavior without user intent.
  - A second strategy catalog for Assistant workflows: rejected because it
    duplicates the imported-catalog lifecycle and the bounded Graph is a
    runtime object, not a product catalog entry.
- `consequences` —
  - The Conversation schema adds Assistant designation and Profile intent
    state idempotently, including preferred reasoning effort; existing
    ambiguous groups stay undesignated.
  - Profile snapshots and route receipts are deterministic and privacy-safe
    projections; they contain no prompt body, credential, absolute path,
    machine identity or backend runtime data.
  - Assistant workflow tools replace `lico_delivery_*` completely; delivery
    tests, catalog entries and documents are removed in the same cutover.
  - The Assistant/Profile/workflow boundary is frozen before implementation
    branches begin; later changes are follow-up decisions.
- `status` — implemented and verified, 2026-08-23.
