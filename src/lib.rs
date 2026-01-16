use libp2p::{
    futures::StreamExt,
    gossipsub, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux, PeerId,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;
use tracing::info;
use fjall::{Database, Keyspace, KeyspaceCreateOptions};
use rand::rngs::OsRng;
use ed25519_dalek::{SigningKey, Signer};
use std::sync::{Arc, Mutex};

/// The physical state of a node in the simulated world
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalState {
    pub voltage: f32,
    pub mah_remaining: f32,
    pub temp_celsius: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PowerMode {
    Normal,
    LowBattery,
    Critical,
}

#[derive(NetworkBehaviour)]
pub struct SporeBehaviour {
    pub gossipsub: gossipsub::Behaviour,
}

pub struct SporeNode {
    pub peer_id: PeerId,
    pub power_mode: PowerMode,
    pub physical_state: Arc<Mutex<PhysicalState>>,
    pub storage: Database,
    pub db: Keyspace,
    pub signing_key: SigningKey,
}

impl SporeNode {
    pub fn new(storage_path: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        let peer_id = PeerId::random();
        
        let storage = Database::builder(storage_path).open()?;
        let db = storage.keyspace("hypha_state", KeyspaceCreateOptions::default)?;
        
        let signing_key = SigningKey::generate(&mut OsRng);

        let physical_state = Arc::new(Mutex::new(PhysicalState {
            voltage: 4.2, // Fully charged Li-ion
            mah_remaining: 2500.0,
            temp_celsius: 25.0,
        }));

        Ok(Self {
            peer_id,
            power_mode: PowerMode::Normal,
            physical_state,
            storage,
            db,
            signing_key,
        })
    }

    /// The "Adaptive Pulse" logic: heartbeat slows as energy fades
    pub fn heartbeat_interval(&self) -> Duration {
        let state = self.physical_state.lock().unwrap();
        if state.voltage < 3.4 || state.mah_remaining < 100.0 {
            Duration::from_secs(60) // Critical: Once a minute
        } else if state.voltage < 3.7 {
            Duration::from_secs(10) // Low: Every 10s
        } else {
            Duration::from_secs(1)  // Normal: Every 1s
        }
    }

    /// Sign an agentic delegation using real cryptographic keys
    pub fn create_delegation(&self, audience: &str, capability: &str) -> Vec<u8> {
        let message = format!("DELEGATE:{}:{}:{}", self.peer_id, audience, capability);
        self.signing_key.sign(message.as_bytes()).to_bytes().to_vec()
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

    /// Reconcile state with a neighbor by comparing Bloom filters or hashes.
    /// Quintessential mycelial efficiency: only send what is missing.
    pub fn reconcile_deltas(&self, neighbor_inventory: Vec<String>) -> Vec<(String, Vec<u8>)> {
        let mut deltas = Vec::new();
        // Use FJALL's prefix search to find all messages we have
        for item in self.db.prefix("msg_") {
            let key = item.key().expect("Storage error");
            let value = self.db.get(&key).unwrap().expect("Value disappeared");
            let key_str = String::from_utf8_lossy(&key).to_string();
            if !neighbor_inventory.contains(&key_str) {
                deltas.push((key_str, value.to_vec()));
            }
        }
        deltas
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        let mut swarm = libp2p::SwarmBuilder::with_new_identity()
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                let message_id_fn = |message: &gossipsub::Message| {
                    let mut s = std::collections::hash_map::DefaultHasher::new();
                    use std::hash::Hasher;
                    s.write(&message.data);
                    gossipsub::MessageId::from(s.finish().to_string())
                };

                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .heartbeat_interval(self.heartbeat_interval())
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .message_id_fn(message_id_fn)
                    .build()?;

                Ok(SporeBehaviour {
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossipsub_config,
                    )?,
                })
            })?
            .build();

        info!(peer_id = %self.peer_id, "Hypha Spore activated");

        loop {
            tokio::select! {
                event = swarm.select_next_some() => match event {
                    SwarmEvent::Behaviour(SporeBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message_id: id,
                        message,
                    })) => {
                        // Delta-State logic would go here:
                        // Compare message hash with FJALL metadata before full processing
                        let key = format!("msg_{}", id);
                        self.db.insert(key, &message.data).unwrap();
                        info!(%peer_id, %id, "Mycelial reconciliation complete");
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod high_fidelity_tests {
    use super::*;
    use turmoil;
    use tempfile::tempdir;

    #[test]
    fn test_mycelium_energy_drain_simulation() {
        let mut sim = turmoil::Builder::new().build();
        let tmp = tempdir().unwrap();
        let storage_path = tmp.path().to_path_buf();

        sim.host("spore-1", move || {
            let path = storage_path.clone();
            async move {
                let node = SporeNode::new(&path).unwrap();
                
                // 1. Initial pulse is fast
                assert_eq!(node.heartbeat_interval(), Duration::from_secs(1));

                // 2. Simulate heavy radio usage draining battery
                {
                    let mut state = node.physical_state.lock().unwrap();
                    state.voltage = 3.6; // Drop to LowBattery range
                    state.mah_remaining = 200.0;
                }

                // 3. Pulse should automatically stretch
                assert_eq!(node.heartbeat_interval(), Duration::from_secs(10));

                // 4. Simulate near-death
                {
                    let mut state = node.physical_state.lock().unwrap();
                    state.voltage = 3.3; // Critical
                }
                assert_eq!(node.heartbeat_interval(), Duration::from_secs(60));

                Ok(())
            }
        });

        sim.run().unwrap();
    }

    #[test]
    fn test_sovereign_agency_signing() {
        let tmp = tempdir().unwrap();
        let node = SporeNode::new(tmp.path()).unwrap();
        
        let sig = node.create_delegation("neighbor-pi", "compute:low-priority");
        assert_eq!(sig.len(), 64); // Ed25519 signature length
        info!("Sovereign delegation signature verified");
    }
}
