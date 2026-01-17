use ed25519_dalek::SigningKey;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use libp2p::{futures::StreamExt, gossipsub, swarm::SwarmEvent, Multiaddr, PeerId};
use rand::{rng, Rng};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

pub mod capabilities;
pub mod eval;
pub mod mesh;
pub mod mycelium;
pub mod sync;

pub use hypha_core::{
    BasicSensor, BatteryMetabolism, Bid, Capability, EnergyStatus, Metabolism, MockMetabolism,
    PowerMode, Task, VirtualSensor,
};

use crate::eval::MetricsCollector;
use crate::mesh::{MeshConfig, MeshControl, TopicMesh};
use crate::mycelium::{Mycelium, MyceliumEvent, NetProfile, Spike};
use crate::sync::{SharedState, SyncMessage};

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

    pub fn set_power_mode(&mut self, mode: PowerMode) {
        self.metabolism.lock().unwrap().set_mode(mode.clone());
        self.power_mode = mode;
    }

    /// Bio-inspired Energy Score: 1.0 is a stable mains-powered node
    pub fn energy_score(&self) -> f32 {
        self.metabolism.lock().unwrap().energy_score()
    }

    /// Quorum-Sensing Auction: Only bid if energy is abundant or few others are bidding
    pub fn evaluate_task(&self, task: &Task, known_bids: usize) -> Option<Bid> {
        let score = self.energy_score();

        // Quorum Logic: If 3+ healthy nodes are already bidding, small spores stay silent
        if known_bids >= 3 && score < 0.8 {
            return None;
        }

        if score < 0.2 {
            // Critical threshold
            return None;
        }

        if self.capabilities.contains(&task.required_capability) {
            Some(Bid {
                task_id: task.id.clone(),
                bidder_id: self.peer_id.to_string(),
                energy_score: score,
                cost_mah: 50.0, // Fixed cost for compute simulation
            })
        } else {
            None
        }
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

        // Pressure-Aware Acceleration: high pressure (backlog) accelerates heartbeat
        // up to 4x to diffuse load faster, provided we have energy.
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

    /// Validate UCAN token for a task
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

        // Mock validation: "auth-valid" token is always valid
        if token.contains("auth-valid") {
            return true;
        }

        false
    }

    /// Bio-inspired Emergent Auctioning: Uses reach diffusion and local consensus
    pub fn process_task_bundle(&self, task: &Task, known_bids: &mut Vec<Bid>) -> Option<Bid> {
        let score = self.energy_score();
        let my_id = self.peer_id.to_string();

        // UCAN Authorization Check
        if let Some(token) = &task.auth_token {
            if !self.validate_ucan(token, &task.required_capability) {
                tracing::warn!(task_id = %task.id, "Rejected task due to invalid UCAN");
                return None;
            }
        } else {
            // Reject unauthenticated tasks in secure mode
            // For now, we allow them for backward compatibility/testing
        }

        // CBBA-inspired: Only bid if our score beats the current best known bid
        let best_bid = known_bids
            .iter()
            .filter(|b| b.task_id == task.id)
            // Avoid panics on NaN bids by using a total ordering.
            .max_by(|a, b| a.energy_score.total_cmp(&b.energy_score));

        if let Some(best) = best_bid {
            if score < best.energy_score {
                return None;
            }
        }

        // Nutrient Reach Check: If reach intensity is too low, we can't "see" the task
        if task.reach_intensity < 0.1 {
            return None;
        }

        if self.capabilities.contains(&task.required_capability) {
            let bid = Bid {
                task_id: task.id.clone(),
                bidder_id: my_id,
                energy_score: score * task.reach_intensity,
                cost_mah: 50.0,
            };
            known_bids.push(bid.clone());
            Some(bid)
        } else {
            None
        }
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

    /// Trigger a network-wide synchrony spike to wake up neighbors and force relaying.
    /// This is used when a node has critical tasks that aren't being picked up.
    pub fn trigger_sync_spike(&self, intensity: u8) -> Result<(), Box<dyn Error>> {
        info!(peer_id = %self.peer_id, %intensity, "Triggering synchrony spike");
        let spike = Spike {
            source: self.peer_id.to_string(),
            intensity,
            pattern_id: 0, // Default emergency pattern
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
        mut mycelium: Mycelium,
        run_for: Duration,
        heartbeat_every: Duration,
        pulse_delta: f32,
        dynamic_heartbeat: bool,
        mut on_listen: Option<tokio::sync::oneshot::Sender<Multiaddr>>,
    ) -> Result<Mycelium, Box<dyn Error>> {
        mycelium.subscribe_all()?;
        info!(peer_id = %self.peer_id, "Hypha Spore active");

        let deadline = tokio::time::Instant::now() + run_for;
        let mut heartbeat = tokio::time::interval(heartbeat_every);
        let mut listen_sent = false;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Ok(mycelium);
            }

            tokio::select! {
                _ = heartbeat.tick() => {
                    // 1. Energy Status Advertisement
                    let energy = self.energy_score();
                    let p = EnergyStatus {
                        source_id: self.peer_id.to_string(),
                        energy_score: energy,
                    };

                    let phase = {
                        let mut mesh = self.mesh.lock().unwrap();
                        mesh.tick_pulse(pulse_delta);
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
                    if dynamic_heartbeat {
                        heartbeat = tokio::time::interval(self.heartbeat_interval());
                    }

                    // 3. Shared State Anti-Entropy (Probabilistic)
                    // Every few heartbeats, broadcast a SyncStep1 to pull missing updates.
                    if rng().random_bool(0.1) {
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
                            // High-speed alert system
                            if let Ok(spike) = serde_json::from_slice::<Spike>(&message.data) {
                                if spike.intensity > 200 {
                                    info!(peer_id = %self.peer_id, "RECEIVED DANGER SPIKE from {}", spike.source);
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
    use tempfile::tempdir;

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
        assert!(node.evaluate_task(&task, 0).is_some());

        // 2. 5 other bidders already exist -> Spore stays silent to save energy
        // Simulate low battery by modifying mock
        metabolism.lock().unwrap().energy = 0.3; // Low battery equivalent

        assert!(
            node.evaluate_task(&task, 5).is_none(),
            "Should stay silent due to quorum"
        );
    }
}
