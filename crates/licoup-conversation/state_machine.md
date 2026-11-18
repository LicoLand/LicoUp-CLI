# Conversation state machines

This document specifies the canonical in-process transition authority for a
Membership-scoped turn and an accepted outbound send. Storage compare-and-set
guards persist a transition result; they do not define a second transition
policy. Runtime adapters report protocol signals and do not own lifecycle
enums.

## TurnState

| From | Event | To |
| --- | --- | --- |
| `Pending` | `Claim` | `Claimed` |
| `Pending` | `Start` | `Running` |
| `Pending` | `Fail` | `Failed` |
| `Pending` | `Interrupt` | `Interrupted` |
| `Pending` | `Cancel` | `Cancelled` |
| `Claimed` | `Start` | `Running` |
| `Claimed` | `Fail` | `Failed` |
| `Claimed` | `Interrupt` | `Interrupted` |
| `Claimed` | `Cancel` | `Cancelled` |
| `Running` | `WaitForHuman` | `WaitingForHuman` |
| `Running` | `Succeed` | `Succeeded` |
| `Running` | `Fail` | `Failed` |
| `Running` | `Interrupt` | `Interrupted` |
| `Running` | `Cancel` | `Cancelled` |
| `WaitingForHuman` | `Resume` | `Running` |
| `WaitingForHuman` | `Fail` | `Failed` |
| `WaitingForHuman` | `Interrupt` | `Interrupted` |
| `WaitingForHuman` | `Cancel` | `Cancelled` |

`Succeeded`, `Failed`, `Interrupted`, and `Cancelled` are absorbing. Every
later event returns the same terminal state. Every other pair absent from the
table returns `TransitionError` and leaves the caller's state unchanged.

The transition relation includes the durable SQL compare-and-set behavior in
the existing Conversation store: pending turns may be claimed or started,
claimed turns may start, and running turns may settle. The additional explicit
waiting, interruption, and cancellation edges make the existing enum variants
reachable without weakening terminal monotonicity.

## SendState

| From | Event | To |
| --- | --- | --- |
| `Sending` | `Deliver` | `Delivered` |
| `Sending` | `Fail` | `Failed` |

`Delivered` and `Failed` are absorbing for every later send event. No retry is
implicit; a retry is a new send with a new `Sending` state.

## Compile-time and property checks

The FSM module denies Clippy wildcard enum match arms locally. Its transition
functions match every state and event explicitly, so a new variant fails to
compile until its behavior is decided. The published const tables are checked
against the transition functions for every state/event pair. Proptest also
applies random 10,000-event sequences and verifies that transition errors do
not mutate state and terminal states never escape.
