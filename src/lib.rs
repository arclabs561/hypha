use ed25519_dalek::SigningKey;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use libp2p::{futures::StreamExt, gossipsub, swarm::SwarmEvent, Multiaddr, PeerId};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::info;

pub mod capabilities;
pub mod eval;
pub mod mesh;
pub mod mycelium;

use crate::eval::MetricsCollector;
use crate::mesh::{MeshConfig, MeshControl, TopicMesh};
use crate::mycelium::{Mycelium, MyceliumEvent, NetProfile, Spike};
// NOTE: UCAN semantics types exist in `capabilities.rs` but aren't wired into
// runtime validation yet. Keep them local there until used to avoid churn.

/// The physical state of a node in the simulated world
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalState {
    pub voltage: f32,
    pub mah_remaining: f32,
    pub temp_celsius: f32,
    pub is_mains_powered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PowerMode {
    Normal,
    LowBattery,
    Critical,
}

/// High-level capability of a spore (The agentic layer)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Capability {
    Compute(u32),
    Storage(u64),
    Sensing(String),
}

/// Energy status advertisement for gradient-based routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyStatus {
    pub source_id: String,
    pub energy_score: f32, // 0.0 (dead) to 1.0 (mains/full)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub required_capability: Capability,
    pub priority: u8,
    /// Reach intensity (diffuses through mesh)
    pub reach_intensity: f32,
    pub source_id: String,
    /// UCAN Authorization token (not a JWT).
    pub auth_token: Option<String>,
}

impl Task {
    pub fn new(id: String, cap: Capability, priority: u8, source_id: String) -> Self {
        Self {
            id,
            required_capability: cap,
            priority,
            reach_intensity: 1.0,
            source_id,
            auth_token: None,
        }
    }

    pub fn with_auth(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Diffuse reach to a neighbor
    pub fn diffuse(&self, conductivity: f32, neighbor_energy: f32, neighbor_pressure: f32) -> f32 {
        let pressure_factor = 1.0 - (neighbor_pressure.min(10.0) / 10.0);
        // More liberal diffusion to ensure reach
        self.reach_intensity
            * conductivity.min(3.0)
            * (neighbor_energy + 0.2).min(1.0)
            * (pressure_factor + 0.1).min(1.0)
            * 0.9
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bid {
    pub task_id: String,
    pub bidder_id: String,
    pub energy_score: f32,
    pub cost_mah: f32,
}

pub trait VirtualSensor: Send + Sync {
    fn name(&self) -> &str;
    fn read(&self) -> f32;
    fn update_from_mesh(&mut self, value: f32);
}

pub struct BasicSensor {
    pub name: String,
    pub last_value: f32,
}

impl VirtualSensor for BasicSensor {
    fn name(&self) -> &str {
        &self.name
    }
    fn read(&self) -> f32 {
        self.last_value
    }
    fn update_from_mesh(&mut self, value: f32) {
        self.last_value = value;
    }
}

pub struct SporeNode {
    pub peer_id: PeerId,
    pub power_mode: PowerMode,
    pub physical_state: Arc<Mutex<PhysicalState>>,
    pub storage: Database,
    pub db: Keyspace,
    pub signing_key: SigningKey,
    pub capabilities: Vec<Capability>,
    pub sensors: Vec<Box<dyn VirtualSensor>>,
    pub mesh: Arc<Mutex<TopicMesh>>,
    pub metrics: Arc<Mutex<MetricsCollector>>,
}

impl SporeNode {
    /// Quintessential Mycelial Initialization: Recovers identity from storage
    pub fn new(storage_path: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        let storage = Database::builder(storage_path).open()?;
        let db = storage.keyspace("hypha_state", KeyspaceCreateOptions::default)?;

        // Recover Node Identity from storage
        let signing_key = if let Some(bytes) = db.get("node_identity_key")? {
            SigningKey::from_bytes(bytes.as_ref().try_into()?)
        } else {
            let key = SigningKey::generate(&mut OsRng);
            db.insert("node_identity_key", key.to_bytes())?;
            key
        };

        let peer_id = PeerId::from_public_key(
            &libp2p::identity::Keypair::ed25519_from_bytes(signing_key.to_bytes())?.public(),
        );

        let physical_state = Arc::new(Mutex::new(PhysicalState {
            voltage: 4.2,
            mah_remaining: 2500.0,
            temp_celsius: 25.0,
            is_mains_powered: false,
        }));

        let mesh = Arc::new(Mutex::new(TopicMesh::new(
            "hypha".to_string(),
            MeshConfig::default(),
        )));
        let metrics = Arc::new(Mutex::new(MetricsCollector::new()));

        Ok(Self {
            peer_id,
            power_mode: PowerMode::Normal,
            physical_state,
            storage,
            db,
            signing_key,
            capabilities: Vec::new(),
            sensors: Vec::new(),
            mesh,
            metrics,
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
        let mut state = self.physical_state.lock().unwrap();
        match mode {
            PowerMode::Normal => {
                state.voltage = 4.0;
                state.mah_remaining = 2000.0;
            }
            PowerMode::LowBattery => {
                state.voltage = 3.6;
                state.mah_remaining = 500.0;
            }
            PowerMode::Critical => {
                state.voltage = 3.3;
                state.mah_remaining = 50.0;
            }
        }
        self.power_mode = mode;
    }

    /// Bio-inspired Energy Score: 1.0 is a stable mains-powered node
    pub fn energy_score(&self) -> f32 {
        let state = self.physical_state.lock().unwrap();
        if state.is_mains_powered {
            return 1.0;
        }
        // Normalize voltage (3.3 to 4.2) and capacity
        let v_score = (state.voltage - 3.3) / (4.2 - 3.3);
        let c_score = state.mah_remaining / 2500.0;
        (v_score * 0.4 + c_score * 0.6).clamp(0.0, 1.0)
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
        if score < 0.2 {
            Duration::from_secs(60)
        } else if score < 0.5 {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(1)
        }
    }

    /// Consume energy for an operation. Returns false if exhausted.
    pub fn consume_energy(&self, mah: f32) -> bool {
        let mut state = self.physical_state.lock().unwrap();
        if state.mah_remaining <= 0.0 {
            return false;
        }
        state.mah_remaining = (state.mah_remaining - mah).max(0.0);
        // Update voltage based on remaining capacity (simple model)
        let capacity_ratio = state.mah_remaining / 2500.0;
        state.voltage = 3.3 + (capacity_ratio * 0.9); // 3.3V to 4.2V
        true
    }

    /// Get current mAh remaining
    pub fn mah_remaining(&self) -> f32 {
        self.physical_state.lock().unwrap().mah_remaining
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
            .max_by(|a, b| a.energy_score.partial_cmp(&b.energy_score).unwrap());

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

    pub fn build_mycelium_with_profile(&self, profile: NetProfile) -> Result<Mycelium, Box<dyn Error>> {
        let keypair = libp2p::identity::Keypair::ed25519_from_bytes(self.signing_key.to_bytes())?;
        let expected_peer_id = PeerId::from_public_key(&keypair.public());
        debug_assert_eq!(
            expected_peer_id, self.peer_id,
            "persisted peer_id must match swarm identity"
        );
        Ok(Mycelium::new_with_profile(
            keypair,
            self.mesh.clone(),
            self.metrics.clone(),
            profile,
        )?)
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
                            let p: EnergyStatus = serde_json::from_slice(&message.data)?;
                            let mut mesh = self.mesh.lock().unwrap();
                            mesh.update_peer_score(&source_peer_id.to_string(), p.energy_score);

                            if p.energy_score > energy + 0.3 {
                                info!(peer_id = %self.peer_id, "Sensing high-energy neighbor {}, moving to passive sync", p.source_id);
                            }
                        } else if message.topic == mycelium.control_topic.hash() {
                            let (target_id, ctrl): (String, MeshControl) = serde_json::from_slice(&message.data)?;
                            if target_id == self.peer_id.to_string() {
                                let mut mesh = self.mesh.lock().unwrap();
                                if let Some(response) = mesh.handle_control(&source_peer_id.to_string(), ctrl) {
                                    let _ = mycelium.swarm.behaviour_mut().gossipsub.publish(
                                        mycelium.control_topic.clone(),
                                        serde_json::to_vec(&(source_peer_id.to_string(), response))?,
                                    );
                                }
                            }
                        } else if message.topic == mycelium.task_topic.hash() {
                            let task: Task = serde_json::from_slice(&message.data)?;
                            info!(%id, task_id = %task.id, "Task detected in network");
                        } else if message.topic == mycelium.spike_topic.hash() {
                            // High-speed alert system
                            if let Ok(spike) = serde_json::from_slice::<Spike>(&message.data) {
                                if spike.intensity > 200 {
                                    info!(peer_id = %self.peer_id, "RECEIVED DANGER SPIKE from {}", spike.source);
                                    let mut mesh = self.mesh.lock().unwrap();
                                    mesh.handle_spike(&spike.source, spike.intensity);
                                }
                            }
                        } else {
                            let key = format!("msg_{}", id);
                            let _ = self.db.insert(key, &message.data);

                            let mut mesh = self.mesh.lock().unwrap();
                            mesh.record_message(&source_peer_id.to_string(), &id.to_string());

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
        let mut node = SporeNode::new(tmp.path()).unwrap();
        node.add_capability(Capability::Compute(100));

        let task = Task {
            id: "compute-task".to_string(),
            required_capability: Capability::Compute(100),
            priority: 1,
            reach_intensity: 1.0,
            source_id: "test-source".to_string(),
            auth_token: None,
        };

        // 1. No other bidders -> Spore bids
        assert!(node.evaluate_task(&task, 0).is_some());

        // 2. 5 other bidders already exist -> Spore stays silent to save energy
        let mut state = node.physical_state.lock().unwrap();
        state.voltage = 3.6; // Low battery
        drop(state);

        assert!(
            node.evaluate_task(&task, 5).is_none(),
            "Should stay silent due to quorum"
        );
    }
}
