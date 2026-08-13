use ed25519_dalek::SigningKey;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use libp2p::{futures::StreamExt, gossipsub, swarm::SwarmEvent, Multiaddr, PeerId};
use rand_core::OsRng;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

pub mod capabilities;
pub mod compute;
pub mod core;
pub mod eval;
pub mod mesh;
pub mod mycelium;
pub mod sync;

pub use crate::core::{
    BasicSensor, BatteryMetabolism, Bid, Capability, EnergyFacts, EnergyStatus, Metabolism,
    MockMetabolism, PowerMode, Task, VirtualSensor,
};

use crate::eval::MetricsCollector;
use crate::mesh::{MeshConfig, MeshControl, TopicMesh};
use crate::mycelium::{Mycelium, MyceliumEvent, NetProfile, Spike};
use crate::sync::{SharedState, SyncMessage};

fn heartbeat_interval(period: Duration) -> tokio::time::Interval {
    let start = tokio::time::Instant::now() + period;
    let mut interval = tokio::time::interval_at(start, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}

const MAX_ANTI_ENTROPY_HEARTBEATS: u8 = 20;

/// Peer-seeded retry schedule with a bounded, jittered interval.
///
/// The previous independent Bernoulli trial had the same approximate mean
/// interval but no upper bound. Keeping the schedule local avoids synchronized
/// fleet-wide pulses while guaranteeing another attempt within twenty
/// heartbeats.
struct AntiEntropySchedule {
    state: u64,
    remaining: u8,
}

impl AntiEntropySchedule {
    fn new(peer_id: &PeerId) -> Self {
        // FNV-1a gives a stable seed without tying protocol behavior to
        // `DefaultHasher`, whose algorithm is intentionally unspecified.
        let mut state = 0xcbf2_9ce4_8422_2325_u64;
        for byte in peer_id.to_bytes() {
            state ^= u64::from(byte);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        if state == 0 {
            state = 0x9e37_79b9_7f4a_7c15;
        }

        let mut schedule = Self {
            state,
            remaining: 0,
        };
        schedule.reset();
        schedule
    }

    fn reset(&mut self) {
        // xorshift64: deterministic per persisted peer identity and cheap on a
        // heartbeat path. Full jitter in 1..=20 keeps the mean near the old
        // ten-heartbeat Bernoulli policy while adding a hard retry bound.
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.remaining = (self.state % u64::from(MAX_ANTI_ENTROPY_HEARTBEATS)) as u8 + 1;
    }

    fn tick(&mut self) -> bool {
        self.remaining -= 1;
        if self.remaining == 0 {
            self.reset();
            true
        } else {
            false
        }
    }
}

struct RunConfig {
    run_for: Duration,
    heartbeat_every: Duration,
    pulse_delta: f32,
    dynamic_heartbeat: bool,
    on_listen: Option<tokio::sync::oneshot::Sender<Multiaddr>>,
}

pub struct SporeNode {
    pub peer_id: PeerId,
    pub power_mode: PowerMode,
    pub metabolism: Arc<Mutex<dyn Metabolism>>,
    pub storage: Database,
    pub db: Keyspace,
    pub signing_key: SigningKey,
    pub capabilities: Vec<Capability>,
    pub sensors: Vec<Box<dyn VirtualSensor>>,
    pub mesh: Arc<Mutex<TopicMesh>>,
    pub metrics: Arc<Mutex<MetricsCollector>>,
    pub shared_state: Arc<Mutex<SharedState>>,
    anti_entropy_schedule: Arc<Mutex<AntiEntropySchedule>>,
}

impl SporeNode {
    /// Quintessential Mycelial Initialization: Recovers identity from storage
    pub fn new(storage_path: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        Self::new_with_metabolism(
            storage_path,
            Arc::new(Mutex::new(BatteryMetabolism::default())),
        )
    }

    /// Initialize with a custom metabolism (e.g. for simulation/testing)
    pub fn new_with_metabolism(
        storage_path: &std::path::Path,
        metabolism: Arc<Mutex<dyn Metabolism>>,
    ) -> Result<Self, Box<dyn Error>> {
        let storage = Database::builder(storage_path).open()?;
        let db = storage.keyspace("hypha_state", KeyspaceCreateOptions::default)?;

        // Recover Node Identity from storage
        let signing_key = if let Some(bytes) = db.get("node_identity_key")? {
            SigningKey::from_bytes(bytes.as_ref().try_into()?)
        } else {
            // `SigningKey::generate` requires a CSPRNG compatible with `rand_core` 0.6.
            // `rand 0.9`'s `ThreadRng` is not compatible here (different rand_core major).
            let mut csprng = OsRng;
            let key = SigningKey::generate(&mut csprng);
            db.insert("node_identity_key", key.to_bytes())?;
            key
        };

        let peer_id = PeerId::from_public_key(
            &libp2p::identity::Keypair::ed25519_from_bytes(signing_key.to_bytes())?.public(),
        );

        let mesh = Arc::new(Mutex::new(TopicMesh::new(
            "hypha".to_string(),
            MeshConfig::default(),
        )));
        let metrics = Arc::new(Mutex::new(MetricsCollector::new()));
        let shared_state = Arc::new(Mutex::new(SharedState::new("hypha_global_state")));
        let anti_entropy_schedule = Arc::new(Mutex::new(AntiEntropySchedule::new(&peer_id)));

        Ok(Self {
            peer_id,
            power_mode: PowerMode::Normal,
            metabolism,
            storage,
            db,
            signing_key,
            capabilities: Vec::new(),
            sensors: Vec::new(),
            mesh,
            metrics,
            shared_state,
            anti_entropy_schedule,
        })
    }

    pub fn add_sensor(&mut self, sensor: Box<dyn VirtualSensor>) {
        info!(peer_id = %self.peer_id, sensor = %sensor.name(), "Added virtual sensor");
        self.sensors.push(sensor);
    }

    pub fn add_capability(&mut self, cap: Capability) {
        info!(peer_id = %self.peer_id, ?cap, "Registered capability");
        self.capabilities.push(cap);
    }

    fn has_capability(&self, required: &Capability) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.satisfies(required))
    }

    fn local_bid_for_task(&self, task: &Task, energy_score: f32) -> Option<Bid> {
        if energy_score < 0.2 || task.reach_intensity < 0.1 {
            return None;
        }

        if !self.has_capability(&task.required_capability) {
            return None;
        }

        Some(Bid {
            task_id: task.id.clone(),
            bidder_id: self.peer_id.to_string(),
            energy_score: energy_score * task.reach_intensity,
            cost_mah: 50.0,
        })
    }

    pub fn set_power_mode(&mut self, mode: PowerMode) {
        self.metabolism.lock().unwrap().set_mode(mode.clone());
        self.power_mode = mode;
    }

    /// Local energy score: 1.0 is a stable mains-powered node.
    pub fn energy_score(&self) -> f32 {
        self.metabolism.lock().unwrap().energy_score()
    }

    /// Local quorum-count bidding heuristic.
    ///
    /// The caller supplies only a count of known competing bids. This is an
    /// advisory local silence rule, not a distributed auction protocol.
    pub fn evaluate_task_with_quorum(&self, task: &Task, known_bids: usize) -> Option<Bid> {
        let score = self.energy_score();

        // If 3+ healthy nodes are already bidding, lower-energy nodes stay silent.
        if known_bids >= 3 && score < 0.8 {
            return None;
        }

        self.local_bid_for_task(task, score)
    }

    /// Compatibility wrapper for the quorum-count local bidding heuristic.
    pub fn evaluate_task(&self, task: &Task, known_bids: usize) -> Option<Bid> {
        self.evaluate_task_with_quorum(task, known_bids)
    }

    pub fn heartbeat_interval(&self) -> Duration {
        let score = self.energy_score();
        let pressure = {
            let mesh = self.mesh.lock().unwrap();
            mesh.local_pressure
        };

        let base_ms = if score < 0.2 {
            60_000 // 1 minute
        } else if score < 0.5 {
            10_000 // 10 seconds
        } else {
            1_000 // 1 second
        };

        // High local pressure accelerates heartbeat up to 4x, provided we have
        // enough energy.
        if score > 0.4 && pressure > 5.0 {
            let factor = (pressure / 5.0).min(4.0);
            Duration::from_millis((base_ms as f32 / factor) as u64)
        } else {
            Duration::from_millis(base_ms)
        }
    }

    /// Consume energy for an operation. Returns false if exhausted.
    pub fn consume_energy(&self, mah: f32) -> bool {
        self.metabolism.lock().unwrap().consume(mah)
    }

    /// Get current mAh remaining
    pub fn mah_remaining(&self) -> f32 {
        self.metabolism.lock().unwrap().remaining()
    }

    /// Check if node is exhausted (cannot participate)
    pub fn is_exhausted(&self) -> bool {
        self.energy_score() < 0.05
    }

    /// Get message count from storage (for consistency checking)
    pub fn message_count(&self) -> usize {
        self.db.prefix("msg_").count()
    }

    /// Get all message IDs (for delta computation)
    pub fn message_ids(&self) -> Vec<String> {
        self.db
            .prefix("msg_")
            .filter_map(|item| {
                item.key()
                    .ok()
                    .map(|k| String::from_utf8_lossy(&k).to_string())
            })
            .collect()
    }

    /// Simulate receiving a message (for evaluation without full network)
    pub fn simulate_receive(&self, msg_id: &str, payload: &[u8]) -> Result<(), Box<dyn Error>> {
        let key = format!("msg_{}", msg_id);
        self.db.insert(key, payload)?;
        Ok(())
    }

    /// Prototype UCAN token check for a task.
    pub fn validate_ucan(&self, token: &str, _required_cap: &Capability) -> bool {
        // In a real implementation:
        // 1. Parse token using `ucan` crate APIs
        // 2. Validate signature against issuer DID
        // 3. Check capabilities against required_cap
        //
        // For prototype: THIS IS NOT SECURITY. It's a placeholder to keep
        // the call sites honest about where auth checks belong.
        if token.is_empty() {
            return false;
        }

        // Mock validation: "auth-valid" token is always valid.
        if token.contains("auth-valid") {
            return true;
        }

        false
    }

    /// Local best-bid bidding heuristic.
    ///
    /// The caller supplies and owns the bid vector. This method may append this
    /// node's bid when it beats the caller-local best known bid, but it does not
    /// coordinate consensus or commitment with other nodes.
    pub fn process_task_bundle_best_bid(
        &self,
        task: &Task,
        known_bids: &mut Vec<Bid>,
    ) -> Option<Bid> {
        let score = self.energy_score();
        let my_id = self.peer_id.to_string();

        // Prototype UCAN gate. This is not a complete authorization boundary.
        if let Some(token) = &task.auth_token {
            if !self.validate_ucan(token, &task.required_capability) {
                tracing::warn!(task_id = %task.id, "Rejected task due to invalid UCAN");
                return None;
            }
        } else {
            // Reject unauthenticated tasks in secure mode
            // For now, we allow them for backward compatibility/testing
        }

        let bid = self.local_bid_for_task(task, score)?;

        // Only bid if the bid we would emit beats the current best known bid
        // supplied by the caller. Non-finite peer bids are ignored; they should
        // not block a local finite bid.
        let best_bid = known_bids
            .iter()
            .filter(|b| b.task_id == task.id && b.energy_score.is_finite())
            .max_by(|a, b| a.energy_score.total_cmp(&b.energy_score));

        if let Some(best) = best_bid {
            if bid.energy_score < best.energy_score {
                return None;
            }
        }

        let bid = Bid {
            bidder_id: my_id,
            ..bid
        };
        known_bids.push(bid.clone());
        Some(bid)
    }

    /// Compatibility wrapper for the caller-local best-bid heuristic.
    pub fn process_task_bundle(&self, task: &Task, known_bids: &mut Vec<Bid>) -> Option<Bid> {
        self.process_task_bundle_best_bid(task, known_bids)
    }

    /// Construct a `Mycelium` swarm bound to this node's persisted identity.
    ///
    /// This is an "advanced" API intended for integration tests / custom runners.
    pub fn build_mycelium(&self) -> Result<Mycelium, Box<dyn Error>> {
        self.build_mycelium_with_profile(NetProfile::default())
    }

    pub fn build_mycelium_with_profile(
        &self,
        profile: NetProfile,
    ) -> Result<Mycelium, Box<dyn Error>> {
        let keypair = libp2p::identity::Keypair::ed25519_from_bytes(self.signing_key.to_bytes())?;
        let expected_peer_id = PeerId::from_public_key(&keypair.public());
        debug_assert_eq!(
            expected_peer_id, self.peer_id,
            "persisted peer_id must match swarm identity"
        );
        Mycelium::new_with_profile(keypair, self.mesh.clone(), self.metrics.clone(), profile)
    }

    /// Trigger a local prototype mesh pressure spike.
    ///
    /// This is advisory pressure telemetry, not an authenticated alert or
    /// wake protocol.
    pub fn trigger_sync_spike(&self, intensity: u8) -> Result<(), Box<dyn Error>> {
        info!(peer_id = %self.peer_id, %intensity, "Triggering mesh pressure spike");
        let spike = Spike {
            source: self.peer_id.to_string(),
            intensity,
            pattern_id: 0,
        };
        let mut mesh = self.mesh.lock().unwrap();
        mesh.handle_spike(&spike.source, spike.intensity);
        Ok(())
    }

    /// Run the networking loop for a bounded amount of time.
    ///
    /// This exists so tests can execute real libp2p behavior without an infinite loop.
    /// Callers can optionally provide a one-shot to learn the first listen address.
    pub async fn run_for(
        &mut self,
        mycelium: Mycelium,
        run_for: Duration,
        heartbeat_every: Duration,
        pulse_delta: f32,
        dynamic_heartbeat: bool,
        on_listen: Option<tokio::sync::oneshot::Sender<Multiaddr>>,
    ) -> Result<Mycelium, Box<dyn Error>> {
        let sync_schedule = self.anti_entropy_schedule.clone();
        self.run_for_with_sync_policy(
            mycelium,
            RunConfig {
                run_for,
                heartbeat_every,
                pulse_delta,
                dynamic_heartbeat,
                on_listen,
            },
            move || sync_schedule.lock().unwrap().tick(),
        )
        .await
    }

    async fn run_for_with_sync_policy<F>(
        &mut self,
        mut mycelium: Mycelium,
        config: RunConfig,
        mut should_sync: F,
    ) -> Result<Mycelium, Box<dyn Error>>
    where
        F: FnMut() -> bool,
    {
        mycelium.subscribe_all()?;
        info!(peer_id = %self.peer_id, "Hypha Spore active");

        let deadline = tokio::time::Instant::now() + config.run_for;
        let mut heartbeat = heartbeat_interval(config.heartbeat_every);
        let mut on_listen = config.on_listen;
        let mut listen_sent = false;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Ok(mycelium);
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    return Ok(mycelium);
                }
                _ = heartbeat.tick() => {
                    // 1. Energy Status Advertisement
                    let (energy, is_mains, mah_remaining) = {
                        let metabolism = self.metabolism.lock().unwrap();
                        (
                            metabolism.energy_score(),
                            metabolism.is_mains_powered(),
                            metabolism.remaining(),
                        )
                    };
                    let p = EnergyStatus::new(self.peer_id.to_string(), energy).with_facts(
                        EnergyFacts {
                            state_of_charge: Some(energy.clamp(0.0, 1.0)),
                            is_mains: Some(is_mains),
                            mah_remaining: Some(mah_remaining),
                            projected_drain_mah_per_hour: None,
                        },
                    );

                    let phase = {
                        let mut mesh = self.mesh.lock().unwrap();
                        mesh.tick_pulse(config.pulse_delta);
                        mesh.pulse_phase
                    };

                    // Pulse-Gating: Only publish status/heartbeats at pulse peak
                    if phase > 0.8 {
                        let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                            mycelium.status_topic.clone(),
                            serde_json::to_vec(&p)?,
                        );

                    // 2. Mesh Heartbeat & Adaptation
                    let (controls, _stats) = {
                        let mut mesh = self.mesh.lock().unwrap();

                        // Adaptive Mesh Configuration: re-calculate based on current energy
                        mesh.config = MeshConfig::adaptive(energy);

                        let c = mesh.heartbeat();
                        (c, mesh.stats())
                    };

                        for (target_peer, ctrl) in controls {
                            let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                mycelium.control_topic.clone(),
                                serde_json::to_vec(&(target_peer, ctrl))?,
                            );
                        }
                    }

                    // Update pressure based on local stats
                    {
                        let mut mesh = self.mesh.lock().unwrap();
                        let backlog = mesh.message_cache.len() as f32; // Proxy for pressure
                        mesh.set_pressure(backlog * 0.1);
                    }

                    // Adjust local heartbeat dynamically
                    if config.dynamic_heartbeat {
                        heartbeat = heartbeat_interval(self.heartbeat_interval());
                    }

                    // 3. Shared State Anti-Entropy (Probabilistic)
                    // Every few heartbeats, broadcast a SyncStep1 to pull missing updates.
                    if should_sync() {
                        let state = self.shared_state.lock().unwrap();
                        let sync_msg = state.create_sync_step_1();
                        if let Ok(bytes) = serde_json::to_vec(&sync_msg) {
                            let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                mycelium.shared_state_topic.clone(),
                                bytes,
                            );
                        }
                    }
                }
                event = mycelium.swarm.select_next_some() => {
                    if !listen_sent {
                        if let SwarmEvent::NewListenAddr { address, .. } = &event {
                            if let Some(tx) = on_listen.take() {
                                let _ = tx.send(address.clone());
                            }
                            listen_sent = true;
                        }
                    }
                    if let SwarmEvent::Behaviour(MyceliumEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: source_peer_id,
                        message_id: id,
                        message,
                    })) = event {
                        let energy = self.energy_score();
                        self.metrics.lock().unwrap().record_delivery(Duration::from_millis(50));

                        if message.topic == mycelium.status_topic.hash() {
                            match serde_json::from_slice::<EnergyStatus>(&message.data) {
                                Ok(p) => {
                                    let mut mesh = self.mesh.lock().unwrap();
                                    mesh.update_peer_score(&source_peer_id.to_string(), p.energy_score);

                                    if p.energy_score > energy + 0.3 {
                                        info!(peer_id = %self.peer_id, "Sensing high-energy neighbor {}, moving to passive sync", p.source_id);
                                    }
                                }
                                Err(e) => {
                                    // Treat malformed status as untrusted input (DoS otherwise).
                                    tracing::warn!(
                                        peer_id = %source_peer_id,
                                        err = %e,
                                        "Ignoring malformed EnergyStatus"
                                    );
                                }
                            }
                        } else if message.topic == mycelium.control_topic.hash() {
                            match serde_json::from_slice::<(String, MeshControl)>(&message.data) {
                                Ok((target_id, ctrl)) => {
                                    if target_id == self.peer_id.to_string() {
                                        let mut mesh = self.mesh.lock().unwrap();
                                        if let Some(response) =
                                            mesh.handle_control(&source_peer_id.to_string(), ctrl)
                                        {
                                            let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                                mycelium.control_topic.clone(),
                                                serde_json::to_vec(&(source_peer_id.to_string(), response))?,
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        peer_id = %source_peer_id,
                                        err = %e,
                                        "Ignoring malformed MeshControl message"
                                    );
                                }
                            }
                        } else if message.topic == mycelium.task_topic.hash() {
                            match serde_json::from_slice::<Task>(&message.data) {
                                Ok(task) => {
                                    info!(%id, task_id = %task.id, "Task detected in network");
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        peer_id = %source_peer_id,
                                        err = %e,
                                        "Ignoring malformed Task"
                                    );
                                }
                            }
                        } else if message.topic == mycelium.spike_topic.hash() {
                            // Prototype pressure telemetry. Not an alert bus.
                            if let Ok(spike) = serde_json::from_slice::<Spike>(&message.data) {
                                if spike.affects_mesh_pressure() {
                                    info!(
                                        peer_id = %self.peer_id,
                                        source = %spike.source,
                                        intensity = spike.intensity,
                                        "Received mesh pressure spike"
                                    );
                                    let mut mesh = self.mesh.lock().unwrap();
                                    mesh.handle_spike(&spike.source, spike.intensity);
                                }
                            } else {
                                tracing::warn!(
                                    peer_id = %source_peer_id,
                                    "Ignoring malformed Spike"
                                );
                            }
                        } else if message.topic == mycelium.shared_state_topic.hash() {
                            // CRDT Sync
                            match serde_json::from_slice::<SyncMessage>(&message.data) {
                                Ok(SyncMessage::Update(bytes)) => {
                                    let state = self.shared_state.lock().unwrap();
                                    if let Err(e) = state.apply_update(&bytes) {
                                        tracing::warn!("Failed to apply CRDT update: {}", e);
                                    } else {
                                        tracing::info!("Applied CRDT update from {}", source_peer_id);
                                    }
                                }
                                Ok(SyncMessage::SyncStep1(sv_bytes)) => {
                                    let state = self.shared_state.lock().unwrap();
                                    if let Ok(reply) = state.handle_sync_step_1(&sv_bytes) {
                                        let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                            mycelium.shared_state_topic.clone(),
                                            serde_json::to_vec(&reply).unwrap(),
                                        );
                                    }
                                }
                                Ok(SyncMessage::SyncStep2(update_bytes)) => {
                                    let state = self.shared_state.lock().unwrap();
                                    if let Err(e) = state.handle_sync_step_2(&update_bytes) {
                                        tracing::warn!("Failed to apply sync step 2: {}", e);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Malformed sync message: {}", e);
                                }
                            }
                        } else {
                            let key = format!("msg_{}", id);
                            let _ = self.db.insert(key, &message.data);

                            let mut mesh = self.mesh.lock().unwrap();
                            mesh.record_message(&source_peer_id.to_string(), &id.to_string());

                            // Emergent Relaying: high-energy nodes relay messages to deepen reach
                            let energy = self.energy_score();
                            let (pressure, pulse_phase) = {
                                let mesh = self.mesh.lock().unwrap();
                                (mesh.local_pressure, mesh.pulse_phase)
                            };

                            // Relaying strategy:
                            // 1. High energy (>0.6)
                            // 2. Low pressure (<7.0)
                            // 3. Pulse-gated (peak) OR high-energy mains power
                            let should_relay = if energy > 0.9 {
                                true // Mains power relays everything
                            } else {
                                energy > 0.6 && pressure < 7.0 && pulse_phase > 0.7
                            };

                            if should_relay {
                                let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                    message.topic.clone(),
                                    message.data.clone(),
                                );
                                info!(%id, "Emergent relay triggered");
                            }

                            info!(%source_peer_id, %id, "Message persisted");
                        }
                    }
                }
            }
        }
    }

    /// Default run loop: listen + run forever.
    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        let mut mycelium = self.build_mycelium()?;
        // Default: listen on an ephemeral local port.
        mycelium.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
        let _ = self
            .run_for(
                mycelium,
                Duration::from_secs(u64::MAX / 4),
                self.heartbeat_interval(),
                0.05,
                true,
                None,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod eval_suite {
    use super::*;
    use libp2p::swarm::dial_opts::DialOpts;
    use tempfile::tempdir;
    use yrs::{ReadTxn, Transact};

    async fn connect_swarms(
        left: &mut Mycelium,
        right: &mut Mycelium,
        right_peer: PeerId,
    ) -> Result<(), Box<dyn Error>> {
        right.listen_on("/ip4/127.0.0.1/tcp/0".parse()?)?;
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } = right.swarm.select_next_some().await
            {
                break address;
            }
        };
        left.swarm.dial(
            DialOpts::peer_id(right_peer)
                .addresses(vec![address])
                .build(),
        )?;

        let left_peer = *left.swarm.local_peer_id();
        let mut left_connected = false;
        let mut right_connected = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !(left_connected && right_connected) && tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = left.swarm.select_next_some() => {
                    if matches!(event, SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == right_peer) {
                        left_connected = true;
                    }
                }
                event = right.swarm.select_next_some() => {
                    if matches!(event, SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == left_peer) {
                        right_connected = true;
                    }
                }
            }
        }
        assert!(left_connected && right_connected, "swarms did not connect");
        left.swarm
            .behaviour_mut()
            .gossipsub
            .add_explicit_peer(&right_peer);
        right
            .swarm
            .behaviour_mut()
            .gossipsub
            .add_explicit_peer(&left_peer);

        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                _ = left.swarm.select_next_some() => {}
                _ = right.swarm.select_next_some() => {}
                _ = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
        }
        Ok(())
    }

    fn state_vector(node: &SporeNode) -> yrs::StateVector {
        node.shared_state
            .lock()
            .unwrap()
            .doc
            .transact()
            .state_vector()
    }

    async fn disconnect_swarms(left: &mut Mycelium, right: &mut Mycelium, right_peer: PeerId) {
        let left_peer = *left.swarm.local_peer_id();
        assert!(left.swarm.disconnect_peer_id(right_peer).is_ok());
        let mut left_closed = false;
        let mut right_closed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !(left_closed && right_closed) && tokio::time::Instant::now() < deadline {
            tokio::select! {
                event = left.swarm.select_next_some() => {
                    if matches!(event, SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == right_peer) {
                        left_closed = true;
                    }
                }
                event = right.swarm.select_next_some() => {
                    if matches!(event, SwarmEvent::ConnectionClosed { peer_id, .. } if peer_id == left_peer) {
                        right_closed = true;
                    }
                }
            }
        }
        assert!(left_closed && right_closed, "swarms did not disconnect");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dynamic_heartbeat_reschedule_does_not_create_ready_loop() {
        let tmp = tempdir().unwrap();
        let mut node = SporeNode::new(tmp.path()).unwrap();
        let mycelium = node.build_mycelium().unwrap();
        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = ticks.clone();

        node.run_for_with_sync_policy(
            mycelium,
            RunConfig {
                run_for: Duration::from_millis(120),
                heartbeat_every: Duration::from_millis(20),
                pulse_delta: 0.1,
                dynamic_heartbeat: true,
                on_listen: None,
            },
            move || {
                observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                false
            },
        )
        .await
        .unwrap();

        assert_eq!(ticks.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn anti_entropy_schedule_is_deterministic_bounded_and_peer_jittered() {
        let peer_id = PeerId::random();
        let mut left = AntiEntropySchedule::new(&peer_id);
        let mut right = AntiEntropySchedule::new(&peer_id);
        let mut intervals = Vec::new();
        let mut since_sync = 0_u8;

        while intervals.len() < 1_000 {
            since_sync += 1;
            let left_syncs = left.tick();
            assert_eq!(left_syncs, right.tick());
            if left_syncs {
                intervals.push(since_sync);
                since_sync = 0;
            }
        }

        assert!(
            intervals
                .iter()
                .all(|interval| (1..=MAX_ANTI_ENTROPY_HEARTBEATS).contains(interval)),
            "anti-entropy interval escaped its hard bound: {intervals:?}"
        );
        let mean =
            intervals.iter().map(|&value| f64::from(value)).sum::<f64>() / intervals.len() as f64;
        assert!((9.5..=11.5).contains(&mean), "unexpected mean: {mean}");

        let other_peer_id = PeerId::random();
        assert_ne!(peer_id, other_peer_id);
        let mut first = AntiEntropySchedule::new(&peer_id);
        let mut second = AntiEntropySchedule::new(&other_peer_id);
        let first_ticks: Vec<_> = (0..100).map(|_| first.tick()).collect();
        let second_ticks: Vec<_> = (0..100).map(|_| second.tick()).collect();
        assert_ne!(
            first_ticks, second_ticks,
            "distinct peer identities produced a synchronized retry sequence"
        );
    }

    #[test]
    fn node_schedule_crosses_repeated_short_run_windows() {
        let tmp = tempdir().unwrap();
        let node = SporeNode::new(tmp.path()).unwrap();
        let schedule = node.anti_entropy_schedule.clone();
        let mut attempts = 0;

        // Seven three-heartbeat windows exceed the maximum interval. The
        // node-owned schedule must continue across those boundaries rather
        // than replaying its initial prefix for every `run_for` call.
        for _window in 0..7 {
            for _heartbeat in 0..3 {
                attempts += usize::from(schedule.lock().unwrap().tick());
            }
        }

        assert!(attempts > 0, "short run windows starved anti-entropy");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn production_schedule_converges_two_swarms_within_maximum_interval() {
        let tmp = tempdir().unwrap();
        let left_path = tmp.path().join("production-left");
        let right_path = tmp.path().join("production-right");
        std::fs::create_dir_all(&left_path).unwrap();
        std::fs::create_dir_all(&right_path).unwrap();
        let mut left_node = SporeNode::new(&left_path).unwrap();
        let mut right_node = SporeNode::new(&right_path).unwrap();
        let right_peer = right_node.peer_id;
        let mut left = left_node.build_mycelium().unwrap();
        let mut right = right_node.build_mycelium().unwrap();
        left.subscribe_all().unwrap();
        right.subscribe_all().unwrap();
        connect_swarms(&mut left, &mut right, right_peer)
            .await
            .unwrap();

        left_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("production-left", "ready");
        right_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("production-right", "ready");
        assert_ne!(state_vector(&left_node), state_vector(&right_node));

        let heartbeat = Duration::from_millis(50);
        let run_for = heartbeat * u32::from(MAX_ANTI_ENTROPY_HEARTBEATS + 4);
        let (left_result, right_result) = tokio::join!(
            left_node.run_for(left, run_for, heartbeat, 0.1, false, None),
            right_node.run_for(right, run_for, heartbeat, 0.1, false, None),
        );
        left_result.unwrap();
        right_result.unwrap();

        assert_eq!(state_vector(&left_node), state_vector(&right_node));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_swarms_converge_after_quiescence_and_reconnect_with_bounded_sync() {
        let tmp = tempdir().unwrap();
        let left_path = tmp.path().join("left");
        let right_path = tmp.path().join("right");
        std::fs::create_dir_all(&left_path).unwrap();
        std::fs::create_dir_all(&right_path).unwrap();
        let mut left_node = SporeNode::new(&left_path).unwrap();
        let mut right_node = SporeNode::new(&right_path).unwrap();
        let right_peer = right_node.peer_id;
        let mut left = left_node.build_mycelium().unwrap();
        let mut right = right_node.build_mycelium().unwrap();
        left.subscribe_all().unwrap();
        right.subscribe_all().unwrap();
        connect_swarms(&mut left, &mut right, right_peer)
            .await
            .unwrap();

        left_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("left-before", "ready");
        right_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("right-before", "ready");

        let left_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let right_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_left = left_requests.clone();
        let count_right = right_requests.clone();
        let (left_result, right_result) = tokio::join!(
            left_node.run_for_with_sync_policy(
                left,
                RunConfig {
                    run_for: Duration::from_millis(900),
                    heartbeat_every: Duration::from_millis(100),
                    pulse_delta: 0.1,
                    dynamic_heartbeat: false,
                    on_listen: None,
                },
                move || {
                    count_left.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    true
                },
            ),
            right_node.run_for_with_sync_policy(
                right,
                RunConfig {
                    run_for: Duration::from_millis(900),
                    heartbeat_every: Duration::from_millis(100),
                    pulse_delta: 0.1,
                    dynamic_heartbeat: false,
                    on_listen: None,
                },
                move || {
                    count_right.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    true
                },
            ),
        );
        let mut left = left_result.unwrap();
        let mut right = right_result.unwrap();
        assert_eq!(state_vector(&left_node), state_vector(&right_node));

        disconnect_swarms(&mut left, &mut right, right_peer).await;
        left_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("left-partition", "ready");
        right_node
            .shared_state
            .lock()
            .unwrap()
            .update_peer_status("right-partition", "ready");
        assert_ne!(state_vector(&left_node), state_vector(&right_node));

        connect_swarms(&mut left, &mut right, right_peer)
            .await
            .unwrap();
        let count_left = left_requests.clone();
        let count_right = right_requests.clone();
        let (left_result, right_result) = tokio::join!(
            left_node.run_for_with_sync_policy(
                left,
                RunConfig {
                    run_for: Duration::from_millis(900),
                    heartbeat_every: Duration::from_millis(100),
                    pulse_delta: 0.1,
                    dynamic_heartbeat: false,
                    on_listen: None,
                },
                move || {
                    count_left.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    true
                },
            ),
            right_node.run_for_with_sync_policy(
                right,
                RunConfig {
                    run_for: Duration::from_millis(900),
                    heartbeat_every: Duration::from_millis(100),
                    pulse_delta: 0.1,
                    dynamic_heartbeat: false,
                    on_listen: None,
                },
                move || {
                    count_right.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    true
                },
            ),
        );
        left_result.unwrap();
        right_result.unwrap();

        assert_eq!(state_vector(&left_node), state_vector(&right_node));
        for (node, requests) in [(&left_node, left_requests), (&right_node, right_requests)] {
            let requests = requests.load(std::sync::atomic::Ordering::Relaxed);
            assert!(
                (16..=20).contains(&requests),
                "unexpected sync request count: {requests}"
            );
            let deliveries = node.metrics.lock().unwrap().delivered_count();
            assert!(deliveries > 0, "the real wire path delivered no messages");
            assert!(
                deliveries <= (requests as u64 * 4),
                "unbounded receive amplification: {deliveries} deliveries for {requests} requests"
            );
        }
    }

    #[test]
    fn test_quorum_sensing_efficiency() {
        let tmp = tempdir().unwrap();
        // Use MockMetabolism for deterministic testing
        let metabolism = Arc::new(Mutex::new(MockMetabolism::new(1.0, false)));
        let mut node = SporeNode::new_with_metabolism(tmp.path(), metabolism.clone()).unwrap();
        node.add_capability(Capability::Compute(100));

        let task = Task {
            id: "compute-task".to_string(),
            required_capability: Capability::Compute(100),
            priority: 1,
            reach_intensity: 1.0,
            source_id: "test-source".to_string(),
            auth_token: None,
        };

        // 1. No other bidders -> Spore bids (energy 1.0)
        assert!(node.evaluate_task_with_quorum(&task, 0).is_some());

        // 2. 5 other bidders already exist -> Spore stays silent to save energy
        // Simulate low battery by modifying mock
        metabolism.lock().unwrap().energy = 0.3; // Low battery equivalent

        assert!(
            node.evaluate_task_with_quorum(&task, 5).is_none(),
            "Should stay silent due to quorum"
        );
    }
}
