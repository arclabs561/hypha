//! Network-namespace / netem harness node.
//!
//! This example is intended for CI / local Linux experiments where we create
//! isolated network namespaces and inject loss/latency via `tc netem`.
//!
//! Design:
//! - `sub`: listens, writes its listen multiaddr (with /p2p/<peerid>) to a file,
//!   then waits for an `EnergyStatus` message and exits 0 on receipt.
//! - `pub`: dials the peer multiaddr, waits briefly for subscription propagation,
//!   publishes an `EnergyStatus`, and exits 0 if publish succeeds.
//!
//! This is deliberately minimal and not a general CLI.

use hypha::mycelium::NetProfile;
use hypha::{EnergyStatus, SporeNode};
use libp2p::futures::StreamExt;
use libp2p::{gossipsub, swarm::SwarmEvent, Multiaddr, PeerId};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sub,
    Pub,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Tcp,
    Quic,
}

fn parse_mode(s: &str) -> Result<Mode, Box<dyn Error>> {
    match s {
        "sub" => Ok(Mode::Sub),
        "pub" => Ok(Mode::Pub),
        "relay" => Ok(Mode::Relay),
        _ => Err(format!("invalid mode: {s} (expected sub|pub|relay)").into()),
    }
}

fn parse_transport(s: &str) -> Result<Transport, Box<dyn Error>> {
    match s {
        "tcp" => Ok(Transport::Tcp),
        "quic" => Ok(Transport::Quic),
        _ => Err(format!("invalid transport: {s} (expected tcp|quic)").into()),
    }
}

fn net_profile(t: Transport) -> NetProfile {
    match t {
        Transport::Tcp => NetProfile::Tcp,
        Transport::Quic => NetProfile::TcpQuic,
    }
}

fn listen_addr(bind_ip: &str, t: Transport) -> Result<Multiaddr, Box<dyn Error>> {
    let a = match t {
        Transport::Tcp => format!("/ip4/{bind_ip}/tcp/0"),
        Transport::Quic => format!("/ip4/{bind_ip}/udp/0/quic-v1"),
    };
    Ok(a.parse()?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        return Err(
            "usage: netem_node <sub|pub> <tcp|quic> <bind_ip> <storage_dir> [outfile|peer_multiaddr]"
                .into(),
        );
    }

    let mode = parse_mode(&args[1])?;
    let transport = parse_transport(&args[2])?;
    let bind_ip = args[3].clone();
    let storage_dir = PathBuf::from(&args[4]);

    fs::create_dir_all(&storage_dir)?;
    let node = SporeNode::new(&storage_dir)?;
    let mut mycelium = node.build_mycelium_with_profile(net_profile(transport))?;
    mycelium.subscribe_all()?;

    match mode {
        Mode::Sub => {
            if args.len() < 6 {
                return Err("sub mode requires outfile path".into());
            }
            let outfile = PathBuf::from(&args[5]);

            mycelium.listen_on(listen_addr(&bind_ip, transport)?)?;

            // Wait for the listen address so the publisher can dial.
            let mut announced = false;
            let start = std::time::Instant::now();
            let t0 = tokio::time::Instant::now();
            let announce_deadline = t0 + Duration::from_secs(2);
            let recv_deadline = t0 + Duration::from_secs(20);

            loop {
                let ev = if announced {
                    tokio::select! {
                        _ = tokio::time::sleep_until(recv_deadline) => {
                            return Err("subscriber timed out waiting for message".into());
                        }
                        ev = mycelium.swarm.select_next_some() => ev,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep_until(announce_deadline) => {
                            return Err("subscriber did not obtain listen addr".into());
                        }
                        _ = tokio::time::sleep_until(recv_deadline) => {
                            return Err("subscriber timed out waiting for message".into());
                        }
                        ev = mycelium.swarm.select_next_some() => ev,
                    }
                };

                match ev {
                    SwarmEvent::NewListenAddr { address, .. } if !announced => {
                        // Print and persist the dial addr including /p2p/<peerid>.
                        let dial = format!("{}/p2p/{}", address, node.peer_id);
                        fs::write(&outfile, dial.as_bytes())?;
                        println!("LISTEN {}", dial);
                        announced = true;
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        mycelium
                            .swarm
                            .behaviour_mut()
                            .gossipsub
                            .add_explicit_peer(&peer_id);
                    }
                    SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(
                        gossipsub::Event::Message { message, .. },
                    )) => {
                        if message.topic == mycelium.status_topic.hash() {
                            let _p: EnergyStatus = serde_json::from_slice(&message.data)?;
                            let dt = start.elapsed();
                            println!("RECEIVED_MS {}", dt.as_millis());
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            }
        }
        Mode::Pub => {
            if args.len() < 6 {
                return Err("pub mode requires peer multiaddr".into());
            }
            let peer: Multiaddr = args[5].parse()?;

            // Dial; then give subscription gossip a moment to propagate.
            mycelium.dial(peer)?;

            let settle_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut connected: Option<PeerId> = None;
            let mut subscribed = false;
            while tokio::time::Instant::now() < settle_deadline
                && !(connected.is_some() && subscribed)
            {
                tokio::select! {
                    ev = mycelium.swarm.select_next_some() => {
                        match ev {
                            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                                connected.get_or_insert(peer_id);
                                mycelium.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                            }
                            SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic })) => {
                                // Wait until the remote peer has subscribed to our status topic.
                                if topic == mycelium.status_topic.hash() {
                                    subscribed = true;
                                    mycelium.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }

            let status = EnergyStatus {
                source_id: "publisher".to_string(),
                energy_score: 0.9,
            };
            let bytes = serde_json::to_vec(&status)?;

            // Publish retries handle the common case: NoPeersSubscribedToTopic.
            let mut last_err: Option<gossipsub::PublishError> = None;
            for _ in 0..10 {
                match mycelium
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(mycelium.status_topic.clone(), bytes.clone())
                {
                    Ok(_) => {
                        println!("PUBLISHED");
                        return Ok(());
                    }
                    Err(e) => {
                        last_err = Some(e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }

            Err(format!("publish failed after retries: {:?}", last_err).into())
        }
        Mode::Relay => {
            if args.len() < 6 {
                return Err("relay mode requires outfile path".into());
            }
            let outfile = PathBuf::from(&args[5]);
            let dial_peer: Option<Multiaddr> = if args.len() >= 7 {
                Some(args[6].parse()?)
            } else {
                None
            };
            let run_ms: u64 = if args.len() >= 8 {
                args[7].parse()?
            } else {
                5_000
            };

            mycelium.listen_on(listen_addr(&bind_ip, transport)?)?;

            let start = std::time::Instant::now();
            let t0 = tokio::time::Instant::now();
            let announce_deadline = t0 + Duration::from_secs(2);
            let run_deadline = t0 + Duration::from_millis(run_ms);

            let mut announced = false;
            let mut dialed = false;

            loop {
                if tokio::time::Instant::now() > run_deadline {
                    return Ok(());
                }

                let ev = if announced {
                    tokio::select! {
                        _ = tokio::time::sleep_until(run_deadline) => { return Ok(()); }
                        ev = mycelium.swarm.select_next_some() => ev,
                    }
                } else {
                    tokio::select! {
                        _ = tokio::time::sleep_until(announce_deadline) => {
                            return Err("relay did not obtain listen addr".into());
                        }
                        _ = tokio::time::sleep_until(run_deadline) => { return Ok(()); }
                        ev = mycelium.swarm.select_next_some() => ev,
                    }
                };

                match ev {
                    SwarmEvent::NewListenAddr { address, .. } if !announced => {
                        // Dial addr including /p2p/<peerid>.
                        let dial = format!("{}/p2p/{}", address, node.peer_id);
                        fs::write(&outfile, dial.as_bytes())?;
                        println!("LISTEN {}", dial);
                        announced = true;
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        mycelium
                            .swarm
                            .behaviour_mut()
                            .gossipsub
                            .add_explicit_peer(&peer_id);
                    }
                    SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(
                        gossipsub::Event::Message { message, .. },
                    )) => {
                        if message.topic == mycelium.status_topic.hash() {
                            // Application-level relay: re-publish once we see a status message.
                            let mut last_err: Option<gossipsub::PublishError> = None;
                            for _ in 0..10 {
                                match mycelium
                                    .swarm
                                    .behaviour_mut()
                                    .gossipsub
                                    .publish(mycelium.status_topic.clone(), message.data.clone())
                                {
                                    Ok(_) => {
                                        let dt = start.elapsed();
                                        println!("RELAYED_MS {}", dt.as_millis());
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = Some(e);
                                        tokio::time::sleep(Duration::from_millis(50)).await;
                                    }
                                }
                            }
                            if last_err.is_some() {
                                return Err(format!("relay publish failed: {:?}", last_err).into());
                            }
                        }
                    }
                    _ => {}
                }

                if announced && !dialed {
                    if let Some(peer) = dial_peer.clone() {
                        mycelium.dial(peer)?;
                        dialed = true;
                        println!("DIALED");
                    }
                }
            }
        }
    }
}
