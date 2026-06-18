---
id: 0003
status: proposed
governs: src/core/mesh.rs, examples/**, EVAL_ANALYSIS.md, ARCHITECTURE.md, docs/**
why: Hypha's current conductivity field is a decaying local usage and pressure-gradient heuristic; it is not a flow variable with edge costs, pressure solve, or conservation.
rejected: claiming Physarum routing from a local scalar; adding a graph flow solver before a routing consumer or measurement requires it.
supersedes: none
superseded_by: none
extends: 0001
confidence: medium
review_trigger: revisit before renaming `conductivity`, adding a flow solver, or publishing routing/evaluation claims based on Physarum behavior.
---

# ADR-0003: conductivity is a usage heuristic

**Status**: Proposed
**Date**: 2026-06-18
**Deciders**: arc

## Context

`MeshPeer::conductivity` contributes 30 percent of `MeshPeer::score()`. The
field starts at `1.0`, increases in `record_message()` based on the absolute
difference between local pressure and the peer's pressure, and decays during
`heartbeat()` by multiplying by `0.95` with a floor of `0.5`.

That is a reasonable local history signal. It rewards peers that recently
carried traffic under a pressure difference and lets unused links cool down.
But it is not a Physarum or slime-mold flow variable. There is no edge length or
cost, no pressure solve across the graph, no flow conservation, and no path
objective. The local decay term helps, but it does not import the convergence
properties of a real flow model.

ADR-0001 says biological names must keep the mechanism that makes the claim
true. `conductivity` currently does not meet that bar for formal routing or
path-finding claims.

## Decision

Hypha treats the current conductivity field as a local usage and pressure
heuristic. It may remain in the internal score while the mesh code is
prototype-stage, but docs, examples, and evaluations must not describe it as a
formal Physarum routing model.

The preferred implementation direction is to rename the concept to something
operational, such as `usage_ewma` or `path_usage`, unless a small flow-model
spike proves useful and cheap enough for Hypha's target deployments.

## Options considered

Keep the name and add caveats. Rejected because the name keeps inviting the
reader to assume a proof or path-finding behavior that the implementation does
not have.

Rename it to a usage heuristic. Chosen as the default direction. This preserves
the local scoring behavior while making the claim match the mechanism.

Implement a formal flow model now. Deferred. A useful version would need edge
costs, a graph pressure solve, a flow update rule, decay, and tests that compare
route choice or allocation behavior against a simpler heuristic. That is a
larger routing decision, not a rename.

Delete the field. Rejected for now because the current score still uses recent
traffic history, and removing it should be tested against the existing mesh
maintenance behavior rather than done as a documentation cleanup.

## Consequences

Docs and examples should stop using slime-mold or Physarum language for the
current peer score. If the field remains named `conductivity` in code for a
transition period, its docs should say it is a local heuristic, not a proof or
flow model.

Evaluation work should not report "slime mold" success from reachability alone.
If Hypha later wants the formal model, the evaluation must compare the flow
model against simpler routing or allocation heuristics on latency, delivery,
energy, and stability.

This ADR does not require an immediate code rename. It records the boundary so
future edits do not build more public surface around the misleading term before
the rename or formal model decision lands.

## Lineage

Extends ADR-0001 by applying the evidence rule to the mesh conductivity field.
Related ADRs cover peer scoring trust separately because a local usage heuristic
and a trust model can change independently.
