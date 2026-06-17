# Topology: one substrate, emergent shape

## Overview

Hypha has looked like two projects fighting: a decentralized power-aware P2P spore mesh
(gossip, CRDT sync, energy-based task bidding, local autonomy — the original intent), and
a fleet of dumb sensors phoning one central store over MQTT (the single-sink sensor
deployment that put hypha-core on real hardware). This document says they are not two
projects. They are one substrate whose **topology emerges from the energy×capability
gradient** rather than being hardcoded — the same idea wireless-sensor-network research
calls cluster-head / sink election (LEACH, HEED). A "mothership" is not a special concept;
it is the node that wins the aggregation role because it advertises `Capability::Storage`
and always-on mains power. The home is the *degenerate single-sink* case of that model
(one such node exists, so election is trivial and everything stars toward it). Remove the
always-on node — a battery/solar field deployment — and the same code falls back to
store-carry-forward buffering and gossip: hypha's original thesis, lit up exactly when the
environment requires it. Nothing is discarded; the home deployment is a special instance,
not a betrayal. Siblings: `ARCHITECTURE.md` (the layer stack), `EMBEDDED.md` (the
hypha-core / host split and the transport shim), `firmware/MESH_OTA_DESIGN.md`.

## The one principle

Every node runs hypha-core and advertises two things: its **capabilities**
(`Sensing`, `Storage`, `Compute`) and its **metabolism** (power source, residual energy,
drain). Roles are not assigned; they are *elected* from that gradient, continuously. The
node best-positioned to hold durable state — storage capability plus stable power — becomes
the **sink** (the "mothership"). The nodes positioned to sense — a radio, a good vantage —
become **sources**. This is exactly the cluster-head-election problem WSN literature has
studied for two decades (LEACH elects heads by residual energy; HEED adds communication
cost); hypha's contribution is not a new election algorithm but that the same
`Metabolism` + `Capability` model drives *every* role decision, so topology is one
emergent consequence of one gradient rather than a separate configuration concern.

## The spectrum (same code, different gradient)

**Single-sink star — the home (live today).** One always-on node has `Storage` +
mains power and always-on connectivity; every other node is a mains-powered `Sensing`
source. Election is trivial — there is one viable sink — so the emergent topology is a
star: sources publish to the sink over MQTT, the sink owns the durable bounded store and
all fusion. This is the BLE-vantage presence deployment. It is hypha-core's *first real
production consumer*: real power model, real radios advertised as `Capability::Sensing`,
real spores. The star is not a compromise of the mesh vision; it is what the mesh vision
*computes* when one node dominates the gradient.

**Multi-sink — redundancy (when earned).** Add a second always-on storage node and the
election is no longer trivial: sources route to the nearest/best reachable sink, and a
sink outage degrades gracefully instead of dropping the stream. This is multi-sink WSN,
not a Beowulf compute cluster — the motivation is failover of *durable state*, and it
should wait for a real availability requirement (a home with one mothership plus a
silence-detector doctor probably does not need it yet).

**Sinkless / intermittent — the field (the original thesis).** Battery or solar nodes
with no always-on storage neighbor cannot star toward anything. Here the gradient has no
dominant node, so the emergent shape is the P2P mesh hypha was designed for:
store-carry-forward buffering (delay-tolerant networking — hold readings locally, forward
on contact), gossip for sync, CRDTs for eventual convergence, and power-aware *bidding*
so a node with little residual energy declines work. This is not legacy ambition; it is
the general case of which the home star is the degenerate instance. The original intent is
preserved precisely by making the home deployment a *point on its spectrum*.

**Compute placement — Beowulf, generalized.** The same election applied to
`Capability::Compute` + fuel/`Metabolism` is task placement: a job bids to the node with
spare energy and the capability to run it, sandboxed in hypha-compute (wasmtime, fuel
mapped to metabolism). Beowulf-style parallelism is not a separate feature; it is the
storage election run over compute instead of state. It stays unbuilt until a job exists
that needs it (consumer-first) — but it costs nothing to leave as the natural extension,
because it is the *same mechanism*.

## What this resolves

- **The home star is hypha, not a detour from it.** It validated hypha-core on hardware
  because a real need (BLE site-local presence for the local network) pulled the
  abstraction into existence instead of it being built speculatively. That is the
  healthiest possible origin for the substrate.
- **The original P2P/CRDT/bidding intent is kept, as the general case.** It is the shape
  the substrate takes when the environment removes the always-on sink. It does not have to
  win an argument against the star; it *contains* the star.
- **Transport is not the identity.** MQTT carries the home star; libp2p/gossip carries the
  sinkless mesh; hypha-core types ride both via the transport shim (`EMBEDDED.md`).
  Choosing MQTT for the home did not abandon the mesh — it selected the transport that fit
  the gradient.

## Consumer-agnostic firmware; roles are deployments

A board does not know what it is *for*. It scans, advertises `Capability::Sensing`,
reports metabolism, and streams over a transport — that is the whole of the public
firmware. "Fixed room-presence vantage," "mobile ego sensor for follower-detection,"
"yard environmental node" are not firmware variants; they are **deployment roles** =
where the board is placed + what config it is given (scan mode, mobility, identity) +
which consumer subscribes to its stream. The same image serves all of them. This is the
line that keeps hypha a *public, agnostic substrate*:

- **hypha (public)**: the spore firmware + the topology/metabolism model. Knows nothing
  about any specific site, presence algorithm, or experiment. Everything
  site-specific enters at build/run time as config (SSID, broker, identity), never as
  committed code or baked binaries.
- **netmon (public)**: the analysis substrate — channel-hopping bandit, identity-fusion
  and localization math — as reusable crates. Knows nothing about your house.
- **the consuming deployment (private)**: wires agnostic boards + agnostic algorithms
  into a concrete use — site-local presence, the ego-sensor experiment, whatever next — and
  owns the site config, credentials, labels, and data. The consumer holds the
  knowledge of *what the data means*; the substrate holds only *how to move it*.

So one fleet of identical boards can be a presence grid today, lend one unit to a mobile
follower-detection experiment tomorrow, and grow a yard sensor next — no firmware fork,
because the role lives in the deployment, not the device. New consumer-specific logic
belongs in the consuming repo; if it ever feels like it must live in the firmware, that is
the signal the firmware is absorbing a consumer concern it should refuse.

## Deployment boundary (don't over-build)

The privacy/trust gradient still governs *which* role a node may hold regardless of what
the energy gradient would elect: a LOW-trust, internet-adjacent node must not become the
sink for HIGH data even if it has the spare storage (the home boards are deliberately
sources that hold no durable state, by the consumer's privacy/role model). Emergent
topology proposes; the trust boundary disposes. And the spectrum is a map, not a mandate:
build the single-sink star (done), add the board-side buffer when outage-resilience earns
it, and let multi-sink / sinkless / compute-bidding stay latent until a real deployment
needs them. The point of unifying them is that you never have to *rewrite* to move along
the spectrum — not that you build all of it now.

## Lineage

- Cluster-head / sink election: LEACH (Heinzelman et al.), HEED — residual-energy-driven
  role election in WSNs; the direct ancestor of "the mothership is elected, not assigned."
- Delay-tolerant networking: store-carry-forward for intermittent links — the sinkless
  buffering path.
- Self-organization / gossip: emergent global structure from local interaction — the
  sinkless sync path.
- hypha-internal: `Metabolism` + `Capability` (hypha-core) are the gradient; the
  Mirollo-Strogatz firefly oscillator (`hypha-firefly`) is the same emergence principle
  applied to LED phase sync; `EMBEDDED.md` is the transport shim that makes one substrate
  span MQTT and libp2p.
