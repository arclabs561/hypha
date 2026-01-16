//! Mycelium Layer: The bio-inspired networking fabric of Hypha.
//!
//! Separates the network behavior (GossipSub, bio-inspired mesh) from the
//! agentic Spore logic.

use crate::eval::MetricsCollector;
use crate::mesh::TopicMesh;
use libp2p::{gossipsub, identity, noise, swarm::NetworkBehaviour, tcp, yamux, Swarm};
use std::error::Error;
use std::sync::{Arc, Mutex};

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "MyceliumEvent")]
pub struct MyceliumBehaviour {
    pub gossipsub: gossipsub::Behaviour,
}

#[derive(Debug)]
pub enum MyceliumEvent {
    Gossipsub(gossipsub::Event),
}

impl From<gossipsub::Event> for MyceliumEvent {
    fn from(event: gossipsub::Event) -> Self {
        MyceliumEvent::Gossipsub(event)
    }
}

/// A rapid electrical spike signal (Adamatzky's fungal language)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Spike {
    pub source: String,
    pub intensity: u8,  // 0-255
    pub pattern_id: u8, // vocabulary index
}

pub struct Mycelium {
    pub swarm: Swarm<MyceliumBehaviour>,
    pub mesh: Arc<Mutex<TopicMesh>>,
    pub metrics: Arc<Mutex<MetricsCollector>>,
    pub pheromone_topic: gossipsub::IdentTopic,
    pub control_topic: gossipsub::IdentTopic,
    pub task_topic: gossipsub::IdentTopic,
    pub spike_topic: gossipsub::IdentTopic,
}

impl Mycelium {
    pub fn new(
        keypair: identity::Keypair,
        mesh: Arc<Mutex<TopicMesh>>,
        metrics: Arc<Mutex<MetricsCollector>>,
    ) -> Result<Self, Box<dyn Error>> {
        // IMPORTANT: Use the caller-provided identity, so network PeerId matches
        // the persisted "soul" key in `SporeNode`.
        let swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| {
                let gossipsub_config = gossipsub::ConfigBuilder::default()
                    .validation_mode(gossipsub::ValidationMode::Strict)
                    .build()?;

                Ok(MyceliumBehaviour {
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossipsub_config,
                    )?,
                })
            })?
            .build();

        let pheromone_topic = gossipsub::IdentTopic::new("hypha_energy_pulse");
        let control_topic = gossipsub::IdentTopic::new("hypha_mesh_control");
        let task_topic = gossipsub::IdentTopic::new("hypha_task_stream");
        let spike_topic = gossipsub::IdentTopic::new("hypha_spikes");

        Ok(Self {
            swarm,
            mesh,
            metrics,
            pheromone_topic,
            control_topic,
            task_topic,
            spike_topic,
        })
    }

    pub fn subscribe_all(&mut self) -> Result<(), Box<dyn Error>> {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.pheromone_topic)?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.control_topic)?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.task_topic)?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.spike_topic)?;
        Ok(())
    }
}
