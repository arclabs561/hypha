use libp2p::{
    futures::StreamExt,
    gossipsub, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux, Multiaddr, PeerId,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PowerMode {
    Normal,
    LowBattery,
    Critical,
}

#[derive(NetworkBehaviour)]
struct SporeBehaviour {
    gossipsub: gossipsub::Behaviour,
}

pub struct SporeNode {
    peer_id: PeerId,
    power_mode: PowerMode,
}

impl SporeNode {
    pub fn new() -> Self {
        let peer_id = PeerId::random();
        Self {
            peer_id,
            power_mode: PowerMode::Normal,
        }
    }

    pub fn set_power_mode(&mut self, mode: PowerMode) {
        info!(peer_id = %self.peer_id, "Power mode changed to {:?}", mode);
        self.power_mode = mode;
        // Logic to adjust gossip frequency or duty cycle would go here
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
                    .heartbeat_interval(Duration::from_secs(10))
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

        info!(peer_id = %self.peer_id, "Starting Spore node");

        // Simple loop to handle events
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
                        info!(%peer_id, %id, "Got message: {:?}", String::from_utf8_lossy(&message.data));
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

    #[tokio::test]
    async fn test_node_creation() {
        let node = SporeNode::new();
        assert_eq!(node.power_mode, PowerMode::Normal);
    }

    #[test]
    fn test_simulation() {
        let mut sim = turmoil::Builder::new().build();

        sim.host("server", || async {
            // Server logic
            Ok(())
        });

        sim.client("client", async {
            // Client logic
            Ok(())
        });

        sim.run().unwrap();
    }
}
