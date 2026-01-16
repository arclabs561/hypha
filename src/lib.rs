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
use ed25519_dalek::SigningKey;
use ucan::builder::UcanBuilder;

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

        Ok(Self {
            peer_id,
            power_mode: PowerMode::Normal,
            storage,
            db,
            signing_key,
        })
    }

    pub fn set_power_mode(&mut self, mode: PowerMode) {
        info!(peer_id = %self.peer_id, "Power mode changed to {:?}", mode);
        self.power_mode = mode;
        
        let mode_bytes = serde_json::to_vec(&self.power_mode).unwrap();
        self.db.insert("power_mode", mode_bytes).unwrap();
    }

    pub fn heartbeat_interval(&self) -> Duration {
        match self.power_mode {
            PowerMode::Normal => Duration::from_secs(1),
            PowerMode::LowBattery => Duration::from_secs(5),
            PowerMode::Critical => Duration::from_secs(30),
        }
    }

    /// Delegate a task to another peer using a UCAN token (Agency layer)
    pub fn delegate_task(&self, audience_did: String, resource: String) -> Result<String, Box<dyn Error>> {
        // In a real implementation, we'd use the node's signing key.
        // UCAN 0.4.0 API might require a specific KeyMaterial implementation.
        // For now, we simulate the token creation logic.
        let token = format!("UCAN:{}:{}:{}", self.peer_id, audience_did, resource);
        info!(to = %audience_did, resource = %resource, "Created UCAN delegation token");
        Ok(token)
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
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        info!(peer_id = %self.peer_id, "Starting Spore node with persistence");

        loop {
            tokio::select! {
                event = swarm.select_next_some() => match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!(%address, "Local node is listening");
                    }
                    SwarmEvent::Behaviour(SporeBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                        propagation_source: peer_id,
                        message_id: id,
                        message,
                    })) => {
                        let key = format!("msg_{}", id);
                        self.db.insert(key, &message.data).unwrap();
                        info!(%peer_id, %id, "Viral message persisted to LSM-tree");
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turmoil;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_node_persistence() {
        let tmp = tempdir().unwrap();
        let storage_path = tmp.path().to_path_buf();
        
        {
            let mut node = SporeNode::new(&storage_path).unwrap();
            node.set_power_mode(PowerMode::LowBattery);
        }
        
        let node2 = SporeNode::new(&storage_path).unwrap();
        let stored_mode = node2.db.get("power_mode").unwrap().unwrap();
        let mode: PowerMode = serde_json::from_slice(&stored_mode).unwrap();
        assert_eq!(mode, PowerMode::LowBattery);
    }

    #[test]
    fn test_simulation_power_drain_viral_death() {
        let mut sim = turmoil::Builder::new().build();
        let tmp = tempdir().unwrap();
        let storage_path = tmp.path().to_path_buf();

        sim.host("node-a", move || {
            let path = storage_path.clone();
            async move {
                let mut node = SporeNode::new(&path).unwrap();
                
                // Simulate time passing and power draining
                tokio::time::sleep(Duration::from_secs(10)).await;
                node.set_power_mode(PowerMode::LowBattery);
                
                tokio::time::sleep(Duration::from_secs(50)).await;
                node.set_power_mode(PowerMode::Critical);
                
                Ok(())
            }
        });

        // Review: Turmoil allows us to observe how the node's heartbeat
        // would theoretically slow down if we had the swarm integrated
        // into the host task.
        sim.run().unwrap();
    }
}
