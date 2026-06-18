# Topology: one substrate, emergent shape

## Overview

Hypha models a deployment as a set of nodes that advertise capabilities and
power state. The resulting topology is not hardcoded as "star" or "mesh"; it is
derived from the energy and capability inputs available in the deployment.

A node with `Capability::Storage` and stable power is a natural sink. Nodes with
`Capability::Sensing` are natural sources. When one sink dominates, the topology
is a single-sink star. When no durable sink is available, nodes need
store-carry-forward buffering, gossip, and eventual convergence. The same
substrate should support both shapes.

Siblings:

- `ARCHITECTURE.md` describes the crate and runtime layers.
- `EMBEDDED.md` describes the host/embedded split and transport bridge.
- `firmware/MESH_OTA_DESIGN.md` describes peer firmware-update work.

## The Principle

Every node advertises two things:

- **Capabilities:** sensing, storage, compute, and transport roles it can
  actually perform.
- **Metabolism:** power source, residual energy, drain, and operating mode.

Roles are elected from those inputs. A durable, mains-powered storage node can
act as a sink. A constrained sensor can remain a source. A battery node can
decline work when its energy state is poor. This is the same family of problems
as cluster-head and sink election in wireless sensor networks: topology follows
from residual energy, communication cost, and role capability.

## Topology Spectrum

### Single-Sink Star

If a deployment has one always-on storage node, election is trivial. Sources
publish readings to that sink, and the sink owns durable storage and downstream
fusion. MQTT or another simple uplink can be the right transport here. This
shape is still hypha: it is the deployment shape implied when one node is
clearly best suited to hold state.

### Multi-Sink

If multiple storage-capable nodes exist, sources can route to the best reachable
sink and fail over when one disappears. This should be built only when a
deployment has a real availability requirement, because it adds coordination,
replication, and conflict-resolution work.

### Sinkless or Intermittent Mesh

If no always-on sink is reachable, nodes must buffer locally and exchange state
opportunistically. This is where store-carry-forward networking, gossip, CRDTs,
and power-aware bidding become necessary rather than decorative.

### Compute Placement

The same model can place work. A compute task should run where the needed
capability and energy budget exist. This is a natural extension of the
capability/metabolism model, but it should wait for a real task that needs it.

## Firmware Boundary

Firmware should remain consumer-agnostic. A board scans, reports capabilities
and metabolism, streams observations over a transport, and accepts only generic
control-plane commands that are part of the public contract. Deployment roles
come from placement and configuration, not from hardcoded application meaning.

Public hypha owns:

- spore identity, capability, metabolism, and observation types
- host node behavior and transport shims
- generic firmware and OTA mechanisms
- examples with synthetic site labels

Deployment repositories own:

- site labels, hostnames, credentials, and board rosters
- interpretation of observations
- downstream consumers and automation
- private topology and placement notes

If application-specific meaning feels necessary in firmware, that is a signal
that the firmware is absorbing a deployment concern and should be pushed back to
configuration or the consuming application.

## Trust Boundary

Energy and capability inputs can propose roles, but the trust boundary can veto
them. A node with storage and power still must not become a sink for data it is
not trusted to hold. Nodes should assert only their own observations, and peer
observations must be treated as untrusted input until a consuming application
validates or fuses them.

The spectrum is a map, not a mandate. Build the single-sink path first, add local
buffering when outage resilience earns it, and leave multi-sink, sinkless, and
compute-bidding behavior latent until a deployment needs them.

## Lineage

- Cluster-head and sink election: residual-energy-driven role election in
  wireless sensor networks.
- Delay-tolerant networking: store-carry-forward behavior for intermittent
  links.
- Gossip and CRDTs: eventual convergence without a central coordinator.
- Hypha core model: `Metabolism` and `Capability` are the inputs that make the
  deployment shape explicit.
