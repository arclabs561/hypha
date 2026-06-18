---
id: 0005
status: proposed
governs: crates/hypha-core/src/agent.rs, src/sync.rs, src/lib.rs, examples/**, docs/**
why: Hypha currently sends task and bid data separately from Yrs shared-state updates; tasks carry no state vector, epoch, or other causality boundary.
rejected: pretending task messages are ordered with CRDT state; adding a state-vector floor before a real task consumer needs versioned shared state.
supersedes: none
superseded_by: none
extends: 0001
confidence: medium
review_trigger: revisit before a task references a shared-state key, before adding task replay, or before exposing task allocation as a correctness-sensitive API.
---

# ADR-0005: tasks do not reference state versions

**Status**: Proposed
**Date**: 2026-06-18
**Deciders**: arc

## Context

Hypha has two coordination surfaces. `Task` and `Bid` live in
`hypha-core::agent` and are used by local evaluation, examples, and libp2p
status flows. `SharedState` wraps a Yrs document and exchanges CRDT updates,
state vectors, and sync-step messages.

Those surfaces do not currently share a clock or causality boundary. A task has
an id, required capability, priority, reach intensity, source id, and optional
auth token. It does not say which version of `SharedState` it observed, which
state vector must be present before processing, or which application epoch it
belongs to.

That is acceptable for toy tasks and local examples. It is not enough for a task
that means "process the readings currently in shared state" or "act on the
configuration I just wrote." In those cases, delivery order between the task
plane and CRDT plane would become part of correctness.

## Decision

Hypha treats current tasks as independent coordination messages that do not
reference shared-state versions. Until a later ADR supersedes this decision,
docs and examples must not imply that a task is causally ordered with Yrs
`SharedState`.

If a future consumer needs a task to depend on shared state, that consumer must
first choose an explicit boundary. The default candidate is a Yrs state-vector
floor carried by the task. A simpler application epoch can be considered if the
consumer does not need CRDT-level causality.

## Options considered

Add a Yrs state-vector floor to every task now. Deferred. It is the strongest
fit when a task depends on CRDT content, but it would expand the public task
schema before any current consumer needs it.

Add an application-level sequence or epoch now. Deferred. It is simpler than a
Yrs state vector but risks becoming a second clock with unclear relationship to
the CRDT plane.

Declare that current tasks cannot reference shared-state versions. Chosen for
now. It matches the implementation, keeps examples honest, and leaves the
causality design open until a real consumer can name its requirement.

Ignore the gap. Rejected because future examples could accidentally imply a
correctness property that does not exist.

## Consequences

Examples should keep task inputs self-contained or label themselves as local
allocation demos. They should not say that a task sees, reserves, or consumes a
particular CRDT state version.

Future task-allocation work has a clear gate: before tasks operate on shared
state, choose and test the causality boundary. Tests should include the failure
case where the task message arrives before the state update it refers to.

This ADR does not choose the final auction algorithm, capability semantics, or
energy-status schema. Those can change independently, although they will likely
be part of the same coordination-type cleanup phase.

## Lineage

Extends ADR-0001 by applying the evidence rule to task/state correctness claims.
Future task-schema ADRs should reference this record if they add a state vector,
application epoch, or another causality boundary.
