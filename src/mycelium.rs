//! Mycelium Layer: The bio-inspired networking fabric of Hypha.
//!
//! Separates the network behavior (GossipSub, bio-inspired mesh) from the
//! agentic Spore logic.

use crate::eval::MetricsCollector;
use crate::mesh::TopicMesh;
use libp2p::{gossipsub, identity, noise, swarm::NetworkBehaviour, tcp, yamux, Multiaddr, Swarm};
use std::error::Error;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetProfile {
    /// TCP + Noise + Yamux
    #[default]
    Tcp,
    /// TCP + Noise + Yamux, plus QUIC (UDP-based).
    TcpQuic,
    /// Low-power mobile profile: prefers QUIC + Relay
    Mobile,
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "MyceliumEvent")]
pub struct MyceliumBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub identify: libp2p::identify::Behaviour,
    pub relay_client: libp2p::relay::client::Behaviour,
    pub dcutr: libp2p::dcutr::Behaviour,
}

#[derive(Debug)]
pub enum MyceliumEvent {
    Gossipsub(gossipsub::Event),
    Identify(Box<libp2p::identify::Event>),
    RelayClient(libp2p::relay::client::Event),
    Dcutr(libp2p::dcutr::Event),
}

impl From<gossipsub::Event> for MyceliumEvent {
    fn from(event: gossipsub::Event) -> Self {
        MyceliumEvent::Gossipsub(event)
    }
}

impl From<libp2p::identify::Event> for MyceliumEvent {
    fn from(event: libp2p::identify::Event) -> Self {
        MyceliumEvent::Identify(Box::new(event))
    }
}

impl From<libp2p::relay::client::Event> for MyceliumEvent {
    fn from(event: libp2p::relay::client::Event) -> Self {
        MyceliumEvent::RelayClient(event)
    }
}

impl From<libp2p::dcutr::Event> for MyceliumEvent {
    fn from(event: libp2p::dcutr::Event) -> Self {
        MyceliumEvent::Dcutr(event)
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
    pub status_topic: gossipsub::IdentTopic,
    pub control_topic: gossipsub::IdentTopic,
    pub task_topic: gossipsub::IdentTopic,
    pub spike_topic: gossipsub::IdentTopic,
    pub shared_state_topic: gossipsub::IdentTopic,
}

impl Mycelium {
    pub fn new(
        keypair: identity::Keypair,
        mesh: Arc<Mutex<TopicMesh>>,
        metrics: Arc<Mutex<MetricsCollector>>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::new_with_profile(keypair, mesh, metrics, NetProfile::default())
    }

    pub fn new_with_profile(
        keypair: identity::Keypair,
        mesh: Arc<Mutex<TopicMesh>>,
        metrics: Arc<Mutex<MetricsCollector>>,
        profile: NetProfile,
    ) -> Result<Self, Box<dyn Error>> {
        let swarm = match profile {
            NetProfile::Tcp => libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
                .with_tokio()
                .with_tcp(
                    tcp::Config::default(),
                    noise::Config::new,
                    yamux::Config::default,
                )?
                // Use SwarmBuilder's relay-client wiring (transport + behaviour) to
                // ensure `/p2p-circuit` addresses actually work and reservations are made.
                .with_relay_client(noise::Config::new, yamux::Config::default)?
                .with_behaviour(|key, relay_client| {
                    let gossipsub_config = gossipsub::ConfigBuilder::default()
                        .validation_mode(gossipsub::ValidationMode::Strict)
                        .build()?;

                    Ok(MyceliumBehaviour {
                        gossipsub: gossipsub::Behaviour::new(
                            gossipsub::MessageAuthenticity::Signed(key.clone()),
                            gossipsub_config,
                        )?,
                        identify: libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                            "/hypha/1.0.0".to_string(),
                            key.public(),
                        )),
                        relay_client,
                        dcutr: libp2p::dcutr::Behaviour::new(key.public().to_peer_id()),
                    })
                })?
                .build(),
            NetProfile::TcpQuic | NetProfile::Mobile => {
                libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
                    .with_tokio()
                    .with_tcp(
                        tcp::Config::default(),
                        noise::Config::new,
                        yamux::Config::default,
                    )?
                    .with_quic()
                    .with_relay_client(noise::Config::new, yamux::Config::default)?
                    .with_behaviour(|key, relay_client| {
                        let gossipsub_config = gossipsub::ConfigBuilder::default()
                            .validation_mode(gossipsub::ValidationMode::Strict)
                            .build()?;

                        Ok(MyceliumBehaviour {
                            gossipsub: gossipsub::Behaviour::new(
                                gossipsub::MessageAuthenticity::Signed(key.clone()),
                                gossipsub_config,
                            )?,
                            identify: libp2p::identify::Behaviour::new(
                                libp2p::identify::Config::new(
                                    "/hypha/1.0.0".to_string(),
                                    key.public(),
                                ),
                            ),
                            relay_client,
                            dcutr: libp2p::dcutr::Behaviour::new(key.public().to_peer_id()),
                        })
                    })?
                    .build()
            }
        };

        let status_topic = gossipsub::IdentTopic::new("hypha_energy_status");
        let control_topic = gossipsub::IdentTopic::new("hypha_mesh_control");
        let task_topic = gossipsub::IdentTopic::new("hypha_task_stream");
        let spike_topic = gossipsub::IdentTopic::new("hypha_spikes");
        let shared_state_topic = gossipsub::IdentTopic::new("hypha_global_state");

        Ok(Self {
            swarm,
            mesh,
            metrics,
            status_topic,
            control_topic,
            task_topic,
            spike_topic,
            shared_state_topic,
        })
    }

    pub fn subscribe_all(&mut self) -> Result<(), Box<dyn Error>> {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.status_topic)?;
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
        self.swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&self.shared_state_topic)?;
        Ok(())
    }

    pub fn listen_on(&mut self, addr: Multiaddr) -> Result<(), Box<dyn Error>> {
        self.swarm.listen_on(addr)?;
        Ok(())
    }

    pub fn dial(&mut self, addr: Multiaddr) -> Result<(), Box<dyn Error>> {
        self.swarm.dial(addr)?;
        Ok(())
    }
}
