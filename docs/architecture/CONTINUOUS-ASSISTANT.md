# Continuous Assistant Architecture

| Related document | Path | Authority |
|:---|:---|:---|
| Normative version | This document | Target continuity semantics and ownership |
| Localization | [简体中文](CONTINUOUS-ASSISTANT.zh-CN.md) | Chinese projection |
| Product | [PRODUCT.md](../../PRODUCT.md) | Product goals and external-operation boundary |
| Decision | [ADR 0010](../adrs/0010-continuous-assistant.md) | Adoption, alternatives and consequences |
| Conversation | [Conversation domain](CONVERSATION-DOMAIN.md) | Canonical history, Membership and turn authority |
| Native Agents | [Agent adapters](AGENT-ADAPTERS-ARCHITECTURE.md) | Native transport, parsing and capabilities |
| Workflows | [Adaptive Flywheel](../functionality/ADAPTIVE-FLYWHEEL.md) | Graph execution authority |
| Workflow evolution | [Workflow control and compiler](ASSISTANT-WORKFLOW-CONTROL.md) | Target compiler, node activation and local/peer intervention |
| Current evidence | [STATUS.md](../STATUS.md) | Implemented and verified capability |

**Status: accepted target design, 2026-09-07; child-conversation amendment
2026-09-08; source adoption mechanism 2026-09-09.** This document specifies the
target architecture. Current implementation facts belong to
[`STATUS.md`](../STATUS.md). It is not compatibility, live-model, or release
evidence. Normative words below apply to the
target. Existing turn, Graph, security and release contracts remain
authoritative in their respective domains. The 2026-09-08 amendment replaces
the earlier single-visible-Conversation presentation: admitted durable work
uses one Canonical child Conversation plus a parent timeline card.

## 1. Purpose

A User works with one visible Assistant without managing coding modes, model
sessions, context resets or a separate memory interface. The Assistant can
answer, discuss, coordinate professional work, wait, and return to unfinished
commitments. Coding is one capability family, not the definition of a Goal.

Continuity is a product responsibility. Reasoning is provided by replaceable
Agents. A continuous Conversation does not require one infinite prompt or one
immortal provider session. Automatic organization reduces mode-management work;
it does not conceal authorship, approvals, spending or external destinations.

## 2. Vocabulary and ownership

These terms are specific to this target design; existing repository terms keep
the meanings in [CONTEXT.md](../../CONTEXT.md).

| Object | Meaning | Sole owner |
|:---|:---|:---|
| Assistant role | The enduring responsibility attached to a Conversation's explicit Assistant designation | Canonical Conversation |
| Designation epoch | A revision boundary when the explicitly designated Membership changes | Canonical Conversation |
| Matter | A revisable association of conversation material about one subject or undertaking | Conversation continuity state |
| Commitment / Goal | An outstanding, bounded result accepted from the User's expressed intent; Goal is its durable tracking record | Conversation continuity state |
| Agreement | A sourced, scoped decision or constraint, with effective and superseding revisions | Conversation continuity state |
| Interpretation proposal | What the host reads out of what the User and the Agent actually said | Cognition works it out; Conversation checks and commits it |
| Context assembly | An ephemeral, authorized selection of original material, agreements and work references for one invocation | Context composition boundary |
| Work context | A private, versioned binding of a Matter and Membership to a native execution session | Existing runtime-binding owner |
| Follow-up | A durable reason to reconsider an unresolved Goal | Conversation host, using existing dispatch |
| Qualification evidence | Version-bound evaluation of a candidate for a specific responsibility | Agent intelligence evaluation owner |
| Task conversation relation | Durable Goal identity bound to one parent Conversation, one child Canonical Conversation and one parent card anchor | Conversation continuity state |
| Parent card anchor | The original parent Event/Part sequence occupied by the task card | Conversation continuity state |
| Parent context grant | Explicit scoped parent SourceRefs for one named child recipient | Conversation continuity state |
| Goal completion transition | Accepted Goal lifecycle change plus the existing-notification identity | Conversation continuity state |

The role is not a hidden Agent Principal. Delegates remain explicit admitted
Memberships. Switching a model, native session or expert does not silently
change the designated Assistant. An explicit replacement advances the
designation epoch, invalidates old decision claims and preserves output
provenance. Outstanding Goals remain attached to the role; in-flight operations
retain their original Membership, epoch and effect identities for reconciliation.

Canonical Events are stored once. Matter associations may be many-to-many and
may refer to message spans or parts. A compound message may update several
Matters. Reclassification appends a revision; it does not rewrite the original
User Event. Matters are not secondary Conversations, ACL bypasses or hidden
transcripts. Ordinary chat stays on the parent Conversation and does not force
a child. Admitted durable follow-through automatically receives one real
Canonical child Conversation under that parent, with admitted Memberships and
true Event/Part/PersistentTurn history. The parent keeps one task card at the
original Event sequence. The first delivery is scoped to the Conversation that
owns the Event. A parent/child relation does not widen data, participant or
model rights. Task-relevant parent context enters a child only through an
admitted ParentContextGrant that names the recipient and the exact SourceRefs.

## 3. Separation of responsibilities

| Responsibility | Owns | Does not own |
|:---|:---|:---|
| Semantic interpretation | Intent, Matter candidates, proposed commitment and capability needs | Permission, budget, execution completion |
| Context composition | Relevant authorized inputs, source provenance, context transition decision | Canonical history or vendor-internal compaction |
| Goal follow-through | Unfulfilled conditions, evidence links, waits, next attention and closure | A second Graph engine or tool executor |
| Qualification | Measured competence and eligibility for one responsibility | Copied model, price, Skill or runtime catalogs |
| Existing execution host | PersistentTurn, Graph admission and effect reconciliation | The truth of a User-level outcome merely because a run ended |
| Flutter | Interaction, inspectable projections and controls | Topic classifiers, lifecycle synthesis or background scheduling |

Implementation remains within the four-tier client architecture. Conversation
stores continuity aggregates and their revisions in the existing SQLite/WAL
boundary. Rust services coordinate through typed ports. Generated bridge
contracts project their facts to Flutter. Native adapters retain isolated
protocol implementations. Separate services, a central cloud backend and a new
general-purpose memory database are not prerequisites.

## 4. Interpreting intent and entering a Goal

Semantic interpretation is Agent work. Lexical rules and indexes may identify
candidates or enforce limits, but do not decide that a User has delegated work.
The interpretation considers independent dimensions:

- Subject: new, existing, resumed, compound, or unresolved Matter association.
- Speech act: question, exploration, delegation, correction, approval, pause,
  cancellation, reference, quotation, or hypothetical discussion.
- Responsibility: no outstanding result, bounded immediate work, durable
  follow-through, or an already authorized recurring policy instance.
- Capability: the skills, tools, environment and expertise needed next.

A clear request may create a Matter and Goal without a mode switch or a form.
Exploration may remain uncommitted. Existing discussion can become a commitment
later. Describing a development possibility is not permission to implement it;
a non-coding delivery can require days of follow-up. The Assistant may propose
an initial definition of done, but consequential ambiguity needs resolution.
It never silently relaxes a User-defined acceptance condition.

An interpretation proposal records what the host read out of an exchange: which
source Event and Parts, the Conversation revision and designation epoch it
observed, Matter candidates, relevant agreement revisions, proposed changes,
capability needs, and reasons it is unsure. It keeps the original input. A
model's own confidence is not a permission and not a completion signal. Unknown
fields, stale versions and unauthorized references are refused. The host works
the proposal out from what the User and the Agent actually said; it never asks
the Agent's reply to carry it or to fit a shape.

The proposal is committed through the Conversation authority with compare-and-
set checks. Reads and interpretation are side-effect-free. A semantic proposal
is not an executable command. Correcting an association does not erase previous
execution, move permissions or transfer an in-flight effect to another Goal.

One Agent invocation may combine interpretation and response. An economical
specialist may also interpret first when evaluation justifies the extra hop.
There is no compulsory classifier on every turn. Insufficient context first
permits bounded retrieval; persistent ambiguity can escalate reasoning or ask
one natural question. A cheap first interpreter must not become a lossy gateway
that withholds original User input from the executor.

## 5. Information lifecycle, not a memory silo

| Information class | Persistence and authority | Use in context |
|:---|:---|:---|
| Conversation fact | Original Event/Part and provenance | Retrieve exact evidence when needed |
| Agreement | Scoped, effective, revisioned decision | Include current relevant constraints |
| Domain knowledge | Its admitted knowledge/source owner | Retrieve source/version and freshness |
| Goal state | Conditions, waits, evidence and outstanding responsibility | Restore the current work obligation |
| Working note | Fallible, sourced, replaceable aid | Include only while useful |
| Native execution state | Runtime-private session and tool environment | Preserve through supported native continuation |

Age alone never turns a statement into knowledge. A speculative old message
stays speculative; a newly confirmed decision can become effective immediately.
Agent-inferred preferences remain distinguishable from direct User statements.
Summaries do not override the source of an agreement or manufacture completion
evidence. Contradictions, supersession and scope narrowing remain explicit.

Critical agreement and Goal changes commit before subsequent affected work.
Optional consolidation can run later in a bounded local maintenance job, but
cannot delay a User correction, block the composer or make hidden paid calls.
Preserving original records follows the User's retention policy, not an
unconditional forever-storage rule.

Deletion or revocation invalidates associated indexes, summaries, assemblies
and reusable work contexts. An assembly is revalidated at dispatch after an
ACL or agreement change. Already disclosed information cannot be recalled from
a provider; the product reports the actual boundary rather than promising it.
Local derived stores must not silently resurrect deleted data from an old
summary or imported backup generation.

Optional Requirement Cognition and Knowledge Domain services remain independent
owners. LicoUp discovers their advertised, versioned capabilities through an
admitted discovery boundary; it does not mirror another repository's endpoint
list or schema. Read-only lookup never creates requirements or knowledge.
Unavailability yields a typed unknown or source-unavailable outcome, not guessed
knowledge or automatic remote creation. Local continuity works without them.

## 6. Context composition and attention

The User-visible timeline, current attention, outstanding work and native
session are separate axes. A topic switch changes attention, not the status of
unrelated Goals. A late result is associated with its original Matter and can
be surfaced without injecting that Matter into the current model context.

Composition starts from a bounded orientation: exact current input, a short
recent exchange, authorized Matter candidates, current agreements and pending
questions. Agent-guided retrieval then refines the selection. Candidate search
combines exact references, entities, lexical matching and recency; semantic
retrieval is an optional provider, not the canonical store. Access control is
applied before candidate retrieval and again before disclosure.

Each assembly records its source/version manifest, relevant agreement revisions,
visibility scope, intended destination, token estimate and selection reasons.
Payloads remain private and follow retention rules; public receipts contain only
approved non-sensitive identifiers and reason codes. Retrieved documents and
old messages are data, not elevated instructions. Source content cannot grant
new capabilities, edit policy or cause external effects by prompt injection.

| Decision | Required condition | Behavior |
|:---|:---|:---|
| Continue | Same relevant work; exact valid binding | Use native continuation and incremental context |
| Compact | Same work; excess context; verified native support | Use native compaction and preserve sourced checkpoints |
| Fork | Deliberate reuse of an authorized reasoning/work branch | Use native fork only with known inheritance semantics |
| New context | Unrelated Matter or required isolation | Start a clean admitted work context |
| Rehydrate | Old binding unavailable, stale or unsuitable | Create a new binding from authorized source-backed state |

Prompt-cache expiry is a cost/latency observation, not semantic amnesia. It
alone causes no compression, Goal closure or archive decision. Returning after
a long absence refreshes relevant facts and assumptions even when the old
session can still resume. Forking is not isolation if it copies the unwanted
history. Native retained memory, workspace rules and ambient tools also belong
to the isolation review; filtering the outgoing prompt alone is insufficient.

A failed exact resume remains a failed resume. Rehydration is a separate,
recorded new-binding operation, never silent fallback inside the adapter's
resume method. This preserves existing exact-session integrity.

## 7. Native execution fidelity

A WorkContext binding is private and scoped by Conversation, Membership,
Matter and binding generation. A turn and its effects pin the exact generation.
Existing unscoped native histories become legacy-context records with no
invented Matter assignment. A classifier can propose associations from admitted
history, but migration does not automatically replay turns or create Goals.

Adapter negotiation distinguishes supported, unsupported, unverified and
temporarily unavailable capabilities. Native resume, fork, compaction, steer,
approval, tools, multimodal input and workspace isolation are independent facts.
No universal implementation is inferred from an Agent's name or protocol family.

LicoUp delegates a bounded outcome, constraints and evidence references rather
than micromanaging the native execution loop. Native Agent tools, configuration,
Skills, environment, model and reasoning settings remain available under their
existing owners. Product continuity is universal; fidelity levels are truthful
and capability-dependent. Do not reduce all Agents to text-in/text-out.

LicoUp shows an Agent's reply as the Agent produced it. Continuity works out
what the reply means from the reply itself. It never asks the reply to fit a
format, and never calls plain language invalid, empty or an abstention. If a
reply has to fit a shape before we can understand it, that is a defect.

Only one writer may mutate a given native session at a time. Foreground dialog
must remain responsive while independent professional work executes. Parallel
contexts require demonstrated runtime support and resource admission. Where
unsupported, the host queues at an honest boundary or uses another explicitly
admitted Membership; it does not launch conflicting writers or silently change
the designated Assistant.

The native runtime owns its internal compaction. LicoUp owns portable agreements,
Goals, source references and handoff checkpoints. They exchange deltas at explicit
boundaries; two layers must not independently rewrite the same execution history.
Raw native identities, credentials and session paths never enter public bridge
receipts. Delegate authorship remains visible in canonical events, even when the
Assistant presents a concise synthesis.

## 8. Goal follow-through and closure

A Goal records source intent, versioned expected result, acceptance conditions,
scope, responsible role, resource envelope, linked executions, evidence, next
attention and closure disposition. A Goal is distinct from a turn or Graph run.

Turn continuation, the Assistant's outcome judgment, agreed acceptance and external
effect settlement are separate facts. The Assistant may end a turn while acceptance
is unverified or an effect remains unresolved. Record those facts without inventing
success or forcing another model call. Ordinary conversation needs no persistent Goal;
none of these distinctions requires fields or a completion declaration in the Agent's
natural reply. The following Goal lifecycle applies only to explicitly tracked work.

Goal lifecycle: `active`, `waiting`, `verifying`, `achieved`, `cancelled`,
`superseded`. `achieved`, `cancelled` and `superseded` are absorbing. Reopening
creates a linked new Goal/versioned undertaking rather than erasing closure.
Pause is a separate scheduling control. A cancellation request blocks new work
immediately; final cancellation waits for reconciliation of relevant in-flight
operations. Blockers are typed facts, not false successes.

Every nonterminal, unpaused Goal has a next-attention record: an active linked
execution, a dispatchable next step, or a named wait with a recovery trigger and
review policy. No Goal may remain merely 'in progress' with no responsible
next action, wait or check. Paused Goals retain the reason and resumption route.

Run completion, callbacks and new admitted evidence may wake a recorded external
wait; User replies may request more work. Scheduled reviews require an already
authorized trigger. Duplicate events coalesce. A due time, missing acceptance or an
unfinished Goal does not itself authorize a model call or fabricate success, failure,
approval or an external event. An ended turn may await evidence or User acceptance.

The Assistant decides whether to continue. Goal tracking records what is owed and
what remains unverified; it does not impose an outer model loop. PersistentTurn or
Adaptive Flywheel executes admitted actions through the existing dispatch, authority,
resource and effect gates. Native termination does not replace agreed acceptance.

Closure requires current evidence for every required condition, the agreed
acceptance method, and no unresolved operation that can invalidate the result.
Machine-checkable conditions use deterministic oracles. Subjective conditions
use the agreed evaluator or User acceptance. An executor's self-report, Graph
termination, delivery receipt, worker exit, reviewer turn or PersistentTurn
settlement alone cannot close a Goal or emit a GoalCompletionTransition.
Accepted closure updates the original parent card in place and publishes one
existing-system notification keyed by a stable `notificationId`. No-focus and
no-replacement-message are invariants of that transition, not wire flags.
Retry, review and restart reuse the same Goal, child Conversation and card
identities. The card remains the historical Event/Part at its original
sequence; `partId` is part of that identity.

Measure progress by new accepted evidence, reduced uncertainty, satisfied
conditions or resolved blockers. Repeated work without such change triggers a
bounded change of strategy, escalation or pause under policy. Count retries,
spending and work volume against explicit envelopes, not an endless 'keep going'
prompt. Persistent maintenance goals produce bounded evaluation intervals or
instances; they are not permanently running turns.

## 9. Durability, concurrency and effects

Continuity state, accepted source-event cursor and follow-up outbox are committed
atomically in the Conversation store. The outbox reliably hands off to the
existing host; it is not a new execution authority. Consumption is at least once,
with stable logical wake/effect identities and deduplication.

Claims include aggregate revision, designation epoch and host generation.
Concurrent proposals use compare-and-set; stale proposals are recomputed or
rejected, never silently overwrite a User correction. Native operations retain
their own logical effect identity across retries. A new turn ID is not a reason
to repeat an already-known effect.

After a crash, existing cold recovery retains interrupted/failed turn facts.
Goal recovery first reconciles prior effects, then schedules a new admitted
step. Known-not-executed, known-executed and execution-unknown remain distinct.
An unknown external result is queried or reviewed before any replay. No claim
of global exactly-once execution is made when the destination lacks that contract.

Closing a view detaches an observer. Pausing suppresses future advancement.
Cancelling requests termination and reconciliation. Deleting source data applies
retention semantics. These operations do not substitute for one another.

The initial host is the User-controlled local Rust runtime. Device sleep or
shutdown suspends actual execution; restart coalesces missed reviews and checks
freshness before acting. An OS timer is not a guarantee of background execution.
Future always-on hosts require explicit endpoint admission, a single active
ownership generation, safe transfer and revocation. They do not make a Station
trusted and do not change Lico Arc wire ownership.

## 10. Qualification and economical routing

Evaluate responsibilities rather than assigning a universal intelligence score.
The candidate identity binds model/reasoning choice, prompt or Skill digest,
context policy, tool contract, adapter/runtime version, dataset version and
policy revision. Keep these as evaluation provenance, not duplicate catalogs.
Read current model prices, tools, Skills and readiness from their existing
owners. Unknown or stale qualification cannot be promoted into verified ability.

The evaluation owner stores observations and their scope. Routing applies hard
admission constraints and the existing candidate ordering; available comparative
evidence can improve future defaults. Qualification is not a prerequisite for ordinary
work, and simple tasks do not require multiple Agents, a Reviewer or an extra Judge.
This does not replace ADR 0004 with another route catalog.

Required evaluations include Matter association, delegated versus hypothetical
intent, follow-through need, current agreements, evidence freshness, context
selection, professional capability choice, correction, cancellation and
appropriate abstention. Test both missed commitments and false takeovers.
Split training/tuning/held-out data by complete conversation or Matter family,
not random adjacent turns. Include multi-domain and multilingual conversations.

Report coverage and selective error together so 'abstain on everything' cannot
pass. Calibrate with held-out outcomes and confidence intervals, not model
self-confidence. Compare end-to-end cost per accepted result, correction effort,
extra routing latency and native-fidelity loss against a fixed strong baseline.
No fixed model brand or price is part of the product contract.

Rollout progresses through offline synthetic evaluation, admitted shadow
observation, bounded low-risk activation and broader responsibility coverage.
Shadow processing obeys the same data-disclosure rules as real work. It does not
send private chats to a new provider or paid evaluator silently. Regression or
changed candidate identity withdraws qualification; explicit User selections
remain respected and unsupported automation stays visible rather than faked.

Comparisons include failures, cancellations, work still in flight, rework and late
charges. Account for changes in task difficulty before claiming improvement. When
evidence is insufficient, retain the current default without purchasing experiments.
Automatic adoption is reversible and affects only authorized future choices; it cannot
change in-flight bindings, acceptance conditions, permissions or budgets.

## 11. Authority and user experience

Automatic interpretation may organize local state within the current delegation.
It does not grant external access. Every model invocation, context disclosure,
expert change and tool effect uses the already applicable admission and approval
path. A new recipient, expanded content or consequential scope change cannot
inherit authorization merely from a Goal, timer, installation or old approval.

There is no compulsory Goal/coding/knowledge mode selector and no
demand-submission form. The User continues ordinary chat. When the Assistant
admits durable long-running work, the product creates one child Canonical
Conversation in the parent's second-level sidebar list and one parent timeline
card fixed at the creating Event's sequence. Coordinator, worker and reviewer
Members converse in that child with real authorship. Progress and accepted
completion update the same card; they do not reinsert it, pin it or move later
messages. Two children A and B keep distinct sequences; later completion
cannot change earlier card order. The User can correct an association, change
a constraint, pause or cancel through ordinary dialogue or accessible
controls. Inspectable details retain actual actors, provenance and spending.
Do not suppress required approvals in the name of seamlessness.

Render projections from Rust. Do not reconstruct lifecycle from prose, silence,
spinners or observer attachment. Context classification and knowledge maintenance
must not run in Flutter. Support capability-appropriate foreground response while
long work progresses; never claim work continues on an unavailable host.

## 12. Verification and adoption

Acceptance covers a continuous, interleaved collaboration, not isolated prompts.
Mandatory classes are non-coding delegation, coding-as-one-capability,
exploration without takeover, multi-Matter messages, topic return, scoped
correction, delayed evidence, subjective acceptance, cancellation, crash recovery,
unknown effects, role replacement, native memory isolation, permission changes,
cache expiry, unavailable knowledge, data deletion, model regression, A/B child
order with out-of-order completion, true worker/reviewer authorship and
isolation, stable reopen/retry/restart, and a single existing-system completion
notice without focus steal.

Deterministic tests prove state, authority, isolation and idempotency invariants.
Model evaluations measure understanding and economical routing. Native parity
checks compare the integrated adapter with direct use of the same admitted
runtime/environment and task. Hermetic proofs and authorized live proofs remain
separate; support is promoted only by the owning compatibility evidence.

Adoption freezes contracts first, implements independent ownership slices against
those contracts, integrates through the existing host, and then enables automation
by measured qualification. Existing conversations migrate without synthetic goals,
rewritten authorship or automatic effects. Feature rollback can stop new
interpretation while preserving Goal state, auditability and explicit recovery.
Local plans, raw evaluations and execution reports stay in the repository's ignored
plan/report locations. The design does not itself change release status.

Current source behavior, distinct from the target above: the host persists one
adoption policy in `continuity_schema` and projects it on `conversation.get`.
Stages follow existing qualification facts. Disabled policy blocks new automatic
interpretation and dispatch only. Explicit user-requested execution keeps its
prior contract. Enum labels and caller-built TrustedConfig or InteractionUseCase
facts are not stored authority. Production live issuance resolves the stored
active owner, admits an evaluation session, and runs the host producer, which
invokes the bound admitted PersistentTurn on known cases and grades typed
outputs before qualification. An unbound runtime stays unavailable. A hermetic
observer is test-only. Reload revalidates that
authorization, collection receipts, and drops revoked or archived owners.
Archive or owner loss denies the next automatic admission without reboot.
`expanded` requires distinct qualified responsibilities. TestEvidence cannot promote real-model
Qualified. Offline and admitted-shadow stages do not dispatch automatically even
when adoption is enabled. Real model qualification is unknown until separately
authorized and run. Parent cards do not show executor badges. This is not
mobile always-on.

## 13. Frozen machine contract (not live capability)

The closed field set lives in
`schemas/client_bridge/conversation.json#continuousAssistant`. Rust and Dart
projections are generated from that embed. Temporary ports return typed
`unsupported_capability` with `effectClass=none`. This section does not claim
enabled automation, native capability, or release evidence.

No reply-shape contract belongs here. These types describe host state that
LicoUp checks and commits. None of them says what an Agent must send. An
Agent's reply is shown as the Agent produced it. A generated type that shapes
the Agent's own reply text is a defect.

| Public generated types | Private ports / leaves |
|:---|:---|
| Matter, Agreement, GoalContract, GoalProgress, InterpretationProposal, ContextManifest, WorkContext, Wake, QualificationRecord, TaskConversationRelation, ParentCardAnchor, ParentContextGrant, ParentGrantBasis, ContextCompositionRequest, GoalCompletionTransition, TaskChildAdmission | ContinuityRead/Commit, Interpretation, ContextComposition, NativeWorkContext, GoalEvaluation, FollowUp, Qualification, DiscoveredKnowledge |
| WriteEnvelope, SourceRef, Utf8ByteSpan, ContinuityFailure | M1 leaves: `continuity`, `assistant_continuity/{cognition,context}`, `work_context` + `work_context_ports`, `qualification`, `frontend/features/continuous_assistant` |

`ContinuityReadPort` pages Goal→child relations and admitted parent grants by
recipient membership. It is not a second Conversation list or scheduler.
`ContextCompositionPort::compose_authorized` is the sole required composition
method. It takes a `ContextCompositionRequest` naming the current recipient
and revocation generation; M1 retrieves grants through the read port. There
is no ambient two-argument `compose`.
Child Conversations remain ordinary Canonical Conversation records created
through existing `conversation.create` / membership / Event actions. The parent
card identity is a `ParentCardAnchor` onto an existing message Event and
metadata Part; no new Event kind is added. Completion notices reuse the
existing notification-center item `id`. `subagent_dispatch_claims` stays an
intra-Conversation dispatch lineage and is not the parent/child Conversation
relation. A one-time scan retired the single-visible-Conversation presentation
in these owned documents; there is no permanent removed-string gate.

The external tool catalog belongs to [Subagent MCP](../protocols/subagent-mcp.md).
Assistant workflow operations use the native CLI; Conversation dispatch stays
on its current actions. `designation_epoch` is a version fact on
the designated Assistant Membership, not a new Principal. `admit-task-child`
is an internal continuity command for durable work, not a User demand form.
