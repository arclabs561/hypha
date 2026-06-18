---
id: 0006
status: proposed
governs: crates/hypha-core/src/agent.rs, src/lib.rs, examples/**, tests/schema_compat.rs, docs/**
why: Hypha's current task coordination surface compresses energy, capability, alert, and bidding semantics into primitive values and two different local bidding functions.
rejected: keeping exact-equality capabilities as the long-term contract; treating sender-provided energy scores as facts; presenting quorum and best-bid logic as one auction protocol.
supersedes: none
superseded_by: none
extends: 0005
confidence: medium
review_trigger: revisit before changing the public Task, Bid, Capability, EnergyStatus, or Spike schema, or before exposing task allocation as more than a local heuristic.
---

# ADR-0006: coordinate with typed facts and one bidding contract

**Status**: Proposed
**Date**: 2026-06-18
**Deciders**: arc

## Context

Hypha's current coordination types are useful for examples, but they are too
compressed for a durable public API.

`EnergyStatus` contains only `source_id` and `energy_score`. That loses the
difference between state of charge, mains power, remaining capacity, and
projected drain. Receivers cannot recompute a score for their own policy, and a
sender-provided score can be mistaken for an observed fact.

`Capability` mixes capacity and identity in one enum. `Compute(u32)` and
`Storage(u64)` are quantities, while `Sensing(String)` is an unconstrained
label. The current matching code uses exact equality, so `Compute(101)` does
not satisfy a task requiring `Compute(50)`, and `"thermal"` does not match
`"temperature"`.

Task bidding currently has two local functions. `evaluate_task` applies a
quorum-style silence rule based on a count of known bids. `process_task_bundle`
uses the caller-supplied current best bid and mutates that caller-local vector.
Both can be useful heuristics, but they are not one shared auction protocol.

`Spike` is a separate alert-like shape with `intensity` and `pattern_id`, but
there is no typed vocabulary, signature rule, or quorum rule. ADR-0002 and
ADR-0004 already prevent treating that path as a trust boundary.

ADR-0005 also says current tasks do not reference shared-state versions. This
ADR covers the other half of the coordination cleanup: the facts and matching
rules inside the task plane itself.

## Decision

Hypha should converge on typed coordination facts and one explicit local
bidding contract before changing the public task API.

The next implementation should split sender-observed energy facts from
receiver-computed scoring. A sender may report facts such as state of charge,
mains flag, optional remaining capacity, and optional projected drain. The
receiver computes whatever scheduling score it needs from those facts.

Capability matching should distinguish capacities from identities. Quantities
such as compute and storage should satisfy smaller requirements when units are
defined. Sensor capabilities should use a closed enum or stable vocabulary
rather than free-form strings.

Task allocation should choose one contract for the current local API. Until a
distributed auction is designed, the contract should say that bidding is local,
caller-supplied, and advisory. Quorum-sensing and best-bid behavior should not
both be presented as the same protocol.

Alert-like messages should not reuse `Spike` as an untyped intensity channel if
they affect behavior. A future alert path needs a typed severity or kind, plus
the trust rule chosen by the relevant security ADR.

## Options considered

Keep the current primitive schema and add comments. Rejected as the long-term
contract because comments cannot fix exact-equality capacity matching or the
fact-versus-score ambiguity in `EnergyStatus`.

Replace everything with a full distributed auction now. Deferred. The current
examples need a clearer local contract first, and ADR-0005 says task/state
causality is still deliberately absent.

Split the problem into typed facts plus one local bidding contract. Chosen for
the next design step. It preserves the prototype shape while making the
eventual public API less ambiguous.

Treat `Spike` as a generic alert bus. Rejected until there is a typed
vocabulary, authentication or trust rule, and a decision about whether alerts
are advisory or action-triggering.

## Consequences

Future schema work should be batched. Changing `EnergyStatus`, `Capability`,
`Task`, `Bid`, and `Spike` independently risks a series of incompatible
half-migrations. A migration should update schema compatibility tests,
examples, and docs together.

The public examples should keep saying "local bidding heuristic" until the
chosen contract is implemented and tested. If the implementation keeps both
heuristics for comparison, they should have different names and not share a
single auction claim.

The capability change needs unit semantics before code. `Compute(u32)` cannot
be promoted to capacity matching until the unit is named, such as Wasmtime fuel
or a task-specific cost unit.

This ADR does not decide task/state causality, which remains governed by
ADR-0005. It also does not decide peer trust or alert security, which remain
governed by ADR-0002 and ADR-0004.

## Lineage

Extends ADR-0005 by keeping the task plane local and advisory while defining
what must be clarified before that plane becomes a stable coordination API.
