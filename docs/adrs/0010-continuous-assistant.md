# ADR 0010: Continuous Assistant, Matters and Goal Follow-through

| Related document | Path | Authority |
|:---|:---|:---|
| Normative version | This document | Decision and rationale |
| Localization | [简体中文](0010-continuous-assistant.zh-CN.md) | Chinese projection |
| Owning specification | [Continuous Assistant](../architecture/CONTINUOUS-ASSISTANT.md) | All continuity mechanisms and invariants |
| Prior decision | [ADR 0004](0004-assistant-authored-flexible-workflows.md) | Explicit Assistant and temporary Graph execution |
| Product | [PRODUCT.md](../../PRODUCT.md) | Product intent and authority boundaries |

## Status

Accepted target design, 2026-09-07. Implementation is pending the independent
implementation plan and its evidence. This ADR does not change current support,
release status or the installed MCP tool catalog.

## Context

The long-lived Assistant already separates user-facing responsibility from
its temporary Graph tools. A remaining gap is continuity before and between
Goals: deciding what the User means, recognizing a new or resumed Matter,
identifying an actual delegation, selecting relevant context, and retaining
commitments through native session replacement.

Users should not have to select coding/goal/knowledge modes, refresh a group,
start new conversations to avoid contamination, or manually move between Agents.
The scope includes research, documents, coordination and everyday work as well
as software development. Neither coding classification nor a very long prompt
is an adequate organizing principle.

## Decision

Adopt the [Continuous Assistant specification](../architecture/CONTINUOUS-ASSISTANT.md)
as the sole owner of the new continuity semantics. Preserve the explicit
Conversation Assistant role, canonical Event/Part history, Membership admission,
existing PersistentTurn and Adaptive Flywheel execution, and native adapters.

An Agent interprets intent and proposes Matter, agreement and commitment changes.
The deterministic Conversation authority validates and commits those proposals.
Context is selected for the current work; native sessions remain private,
replaceable execution bindings. Goal follow-through schedules reconsideration
through the existing host. Responsibility qualification supplies measured
eligibility without copying capability/price catalogs or introducing a weighted
intelligence score.

This extends ADR 0004; it does not reinstate its retired delivery scheduler,
fixed Designer/Worker/Reviewer topology or hidden plan/transcript authority.
The designated Membership is never silently reassigned. Native exact-resume
failure remains honest; a product-level rehydration is a distinct new binding.
Existing security approvals and original output authorship remain intact.

## Alternatives

| Alternative | Reason not selected |
|:---|:---|
| A coding-mode classifier as the Goal gate | Confuses professional capability with outstanding responsibility; excludes non-coding work |
| A compulsory small-model router before every turn | Adds latency even when one qualified invocation can understand and respond |
| One forever-growing native session | Couples identity to provider state and mixes unrelated Matters |
| A new opaque long-term memory database | Mixes facts, agreements, knowledge and obligations without clear owners |
| A new universal execution/orchestration engine | Duplicates established turn, Graph and effect authorities |
| Normalize every Agent to plain text | Discards native tools, continuation, approvals and environment fidelity |

## Rationale

The stable product abstraction is collaboration responsibility, not a particular
model session. Semantic interpretation is model work; permission, revision and
effect enforcement are deterministic work. Separating these responsibilities
allows better and cheaper models to qualify over time without changing the
User's interaction model or rebuilding the execution foundation.

Keeping context, knowledge and commitment semantics distinct makes retrieval,
correction, forgetting and follow-up individually testable. Capability-aware
native execution preserves the specialist advantages users already rely on.

## Consequences

Implementation adds scoped continuity aggregates and source associations,
versioned interpretation/context ports, responsibility evaluation and host-owned
follow-up. It extends generated contracts and private runtime binding semantics
in a coordinated migration, not by adding vendor branches to Flutter.

Tasks freeze shared contracts before independent ownership slices begin.
Rollout measures false takeover and missed responsibility as well as task
success, latency, total cost and direct-native parity. Old conversations migrate
without invented Goals or automatic external work. Optional knowledge services
remain independently discovered owners, not mandatory infrastructure.

## Implementation evidence

No runtime evidence is asserted by this documentation change. Adoption evidence
must identify contract validation, state-machine and recovery tests, continuous
multi-domain scenarios, model evaluation, native parity, and the relevant platform
and adapter lanes before current-state documents are promoted. Local plan and
raw verification outputs follow the repository's ignored-path policy.
