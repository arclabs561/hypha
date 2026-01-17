use hypha::{EnergyStatus, SporeNode};
use libp2p::futures::StreamExt;
use libp2p::{
    gossipsub, multiaddr::Protocol, noise, relay, swarm::dial_opts::DialOpts, swarm::SwarmEvent,
    tcp, yamux, Multiaddr,
};
use tempfile::tempdir;

#[derive(libp2p::swarm::NetworkBehaviour)]
#[behaviour(to_swarm = "RelayOnlyEvent")]
struct RelayOnlyBehaviour {
    relay_client: relay::client::Behaviour,
}

#[derive(Debug)]
enum RelayOnlyEvent {
    RelayClient(relay::client::Event),
}

impl From<relay::client::Event> for RelayOnlyEvent {
    fn from(e: relay::client::Event) -> Self {
        RelayOnlyEvent::RelayClient(e)
    }
}

/// End-to-end smoke test:
/// - start 2 real libp2p swarms
/// - connect over localhost TCP
/// - publish `EnergyStatus` on the status topic
/// - assert the receiver learns the sender as a peer (via `update_peer_score`)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_status_gossip_adds_peer() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let p0 = tmp.path().join("n0");
    let p1 = tmp.path().join("n1");
    std::fs::create_dir_all(&p0)?;
    std::fs::create_dir_all(&p1)?;

    let n0 = SporeNode::new(&p0)?;
    let n1 = SporeNode::new(&p1)?;

    let sender_peer = n0.peer_id.to_string();
    let peer0 = n0.peer_id;
    let peer1 = n1.peer_id;

    let mut m0 = n0.build_mycelium()?;
    let mut m1 = n1.build_mycelium()?;
    m0.subscribe_all()?;
    m1.subscribe_all()?;

    m0.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>()?)?;
    m1.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>()?)?;

    // Capture listen addresses by polling swarms briefly.
    let mut a0: Option<Multiaddr> = None;
    let mut a1: Option<Multiaddr> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);

    while (a0.is_none() || a1.is_none()) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                if let SwarmEvent::NewListenAddr { address, .. } = ev {
                    a0.get_or_insert(address);
                }
            }
            ev = m1.swarm.select_next_some() => {
                if let SwarmEvent::NewListenAddr { address, .. } = ev {
                    a1.get_or_insert(address);
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    let _a0 = a0.ok_or("node0 did not obtain listen addr")?;
    let a1 = a1.ok_or("node1 did not obtain listen addr")?;

    // Dial using explicit peer_id + address. This avoids multiaddr parsing edge cases.
    m0.swarm
        .dial(DialOpts::peer_id(peer1).addresses(vec![a1.clone()]).build())?;
    // One direction is sufficient for a connection.

    // Wait for connection establishment on both sides.
    let mut c0 = false;
    let mut c1 = false;
    let mut last0: Option<SwarmEvent<hypha::mycelium::MyceliumEvent>> = None;
    let mut last1: Option<SwarmEvent<hypha::mycelium::MyceliumEvent>> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !(c0 && c1) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::ConnectionEstablished { .. }) {
                    c0 = true;
                }
                last0 = Some(ev);
            }
            ev = m1.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::ConnectionEstablished { .. }) {
                    c1 = true;
                }
                last1 = Some(ev);
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    assert!(
        c0 && c1,
        "connection not established: last0={:?} last1={:?}",
        last0,
        last1
    );

    // Coverage: ensure Identify actually runs and emits events.
    // Without this, it's easy to accidentally mis-wire Identify and still have
    // the gossipsub-only smoke test pass.
    let mut id0 = false;
    let mut id1 = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !(id0 && id1) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Identify(_))) {
                    id0 = true;
                }
            }
            ev = m1.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Identify(_))) {
                    id1 = true;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    assert!(
        id0 && id1,
        "identify did not emit on both peers: id0={id0} id1={id1}"
    );

    // Ensure gossipsub considers peers for direct publishing.
    m0.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer1);
    m1.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer0);

    // Allow subscription gossip to propagate before publishing.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => { let _ = ev; }
            ev = m1.swarm.select_next_some() => { let _ = ev; }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    // Publish one status from node0.
    let status = EnergyStatus {
        source_id: "node0".to_string(),
        energy_score: 0.9,
    };
    let bytes = serde_json::to_vec(&status)?;
    let pub_res = m0
        .swarm
        .behaviour_mut()
        .gossipsub
        .publish(m0.status_topic.clone(), bytes);
    assert!(pub_res.is_ok(), "publish failed: {:?}", pub_res);

    // Poll swarms until node1 receives it (or time out).
    let mut received = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);

    while !received && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                // keep network progressing
                let _ = ev;
            }
            ev = m1.swarm.select_next_some() => {
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source,
                    message,
                    ..
                })) = ev {
                    if message.topic == m1.status_topic.hash() {
                        let p: EnergyStatus = serde_json::from_slice(&message.data)?;
                        let mut mesh = n1.mesh.lock().unwrap();
                        mesh.update_peer_score(&propagation_source.to_string(), p.energy_score);
                        received = true;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    assert!(received, "node1 did not receive status gossip");
    {
        let mesh = n1.mesh.lock().unwrap();
        assert!(
            mesh.known_peers.contains_key(&sender_peer),
            "node1 did not learn sender peer_id"
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_status_gossip_over_quic() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let p0 = tmp.path().join("n0");
    let p1 = tmp.path().join("n1");
    std::fs::create_dir_all(&p0)?;
    std::fs::create_dir_all(&p1)?;

    let n0 = SporeNode::new(&p0)?;
    let n1 = SporeNode::new(&p1)?;

    let sender_peer = n0.peer_id.to_string();
    let peer1 = n1.peer_id;

    let mut m0 = n0.build_mycelium_with_profile(hypha::mycelium::NetProfile::TcpQuic)?;
    let mut m1 = n1.build_mycelium_with_profile(hypha::mycelium::NetProfile::TcpQuic)?;
    m0.subscribe_all()?;
    m1.subscribe_all()?;

    m0.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse::<Multiaddr>()?)?;
    m1.listen_on("/ip4/127.0.0.1/udp/0/quic-v1".parse::<Multiaddr>()?)?;

    // Capture node1's listen address.
    let mut a1: Option<Multiaddr> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while a1.is_none() && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m1.swarm.select_next_some() => {
                if let SwarmEvent::NewListenAddr { address, .. } = ev {
                    a1.get_or_insert(address);
                }
            }
            ev = m0.swarm.select_next_some() => { let _ = ev; }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    let a1 = a1.ok_or("node1 did not obtain QUIC listen addr")?;

    // Dial node1.
    m0.swarm
        .dial(DialOpts::peer_id(peer1).addresses(vec![a1.clone()]).build())?;

    // Coverage: ensure Identify runs over QUIC too.
    let mut id0 = false;
    let mut id1 = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !(id0 && id1) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Identify(_))) {
                    id0 = true;
                }
            }
            ev = m1.swarm.select_next_some() => {
                if matches!(ev, SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Identify(_))) {
                    id1 = true;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    assert!(
        id0 && id1,
        "identify did not emit on both peers over QUIC: id0={id0} id1={id1}"
    );

    // Give the swarms time to exchange subscriptions; otherwise publish can yield NoPeersSubscribedToTopic.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => { let _ = ev; }
            ev = m1.swarm.select_next_some() => { let _ = ev; }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    let status = EnergyStatus {
        source_id: "node0".to_string(),
        energy_score: 0.9,
    };
    let bytes = serde_json::to_vec(&status)?;
    let pub_res = m0
        .swarm
        .behaviour_mut()
        .gossipsub
        .publish(m0.status_topic.clone(), bytes);
    assert!(pub_res.is_ok(), "publish failed: {:?}", pub_res);

    let mut received = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !received && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => { let _ = ev; }
            ev = m1.swarm.select_next_some() => {
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Message { propagation_source, message, .. })) = ev {
                    if message.topic == m1.status_topic.hash() {
                        let p: EnergyStatus = serde_json::from_slice(&message.data)?;
                        let mut mesh = n1.mesh.lock().unwrap();
                        mesh.update_peer_score(&propagation_source.to_string(), p.energy_score);
                        received = true;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    assert!(received, "node1 did not receive QUIC status gossip");
    let mesh = n1.mesh.lock().unwrap();
    assert!(mesh.known_peers.contains_key(&sender_peer));
    Ok(())
}

/// Coverage test: exercise circuit relay v2 reservation using Hypha's `Mycelium` wiring.
///
/// This does NOT simulate NAT, but it does verify that:
/// - we can connect to a relay server
/// - `relay::client::Behaviour` emits `ReservationReqAccepted`
/// - our Mycelium transport stack can `listen_on` a `/p2p-circuit` address
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_circuit_relay_reservation_tcp() -> Result<(), Box<dyn std::error::Error>> {
    // Relay server (Circuit Relay v2).
    let mut relay_swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            relay::Behaviour::new(key.public().to_peer_id(), relay::Config::default())
        })?
        .build();

    relay_swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>()?)?;
    let relay_peer = *relay_swarm.local_peer_id();

    let mut relay_listen: Option<Multiaddr> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while relay_listen.is_none() && tokio::time::Instant::now() < deadline {
        if let SwarmEvent::NewListenAddr { address, .. } = relay_swarm.select_next_some().await {
            relay_listen.get_or_insert(address);
        }
    }
    let relay_listen = relay_listen.ok_or("relay did not obtain listen addr")?;
    // In libp2p relay setups, the relay typically needs to advertise its reachable address.
    relay_swarm.add_external_address(relay_listen.clone());
    relay_swarm.add_external_address(relay_listen.clone().with(Protocol::P2p(relay_peer)));

    // Client
    let tmp = tempdir()?;
    let p0 = tmp.path().join("client0");
    std::fs::create_dir_all(&p0)?;
    let n0 = SporeNode::new(&p0)?;
    let mut m0 = n0.build_mycelium_with_profile(hypha::mycelium::NetProfile::Tcp)?;
    m0.subscribe_all()?;

    // Ensure the client has at least one direct listen address before attempting relay listen.
    // Some stacks only trigger external address confirmation / listener plumbing after a base listener exists.
    m0.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>()?)?;

    // Include `/p2p/<relay_peer>` so the relay subsystem has a canonical address.
    let relay_dial = relay_listen.clone().with(Protocol::P2p(relay_peer));

    // Listen via relay (reservation triggers here).
    // For Circuit Relay v2, the listen addr is rooted at the relay, then `/p2p-circuit`.
    let relay_circuit = relay_dial.clone().with(Protocol::P2pCircuit);
    m0.listen_on(relay_circuit)?;

    // Drive until reservation accepted.
    let mut reservation_ok = false;
    let mut relay_saw_accept = false;
    let mut client_connected = false;
    let mut client_closed = false;
    let mut last_client: Option<String> = None;
    let mut last_relay: Option<String> = None;
    let mut last_relay_client_event: Option<String> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !(reservation_ok && relay_saw_accept) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => {
                last_client = Some(format!("{ev:?}"));
                if let SwarmEvent::ConnectionEstablished { peer_id, .. } = ev {
                    if peer_id == relay_peer {
                        client_connected = true;
                    }
                }
                if let SwarmEvent::ConnectionClosed { peer_id, .. } = ev {
                    if peer_id == relay_peer {
                        client_closed = true;
                    }
                }
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::RelayClient(e)) = ev {
                    last_relay_client_event = Some(format!("{e:?}"));
                    if let relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } = e {
                        if relay_peer_id == relay_peer {
                            reservation_ok = true;
                        }
                    }
                }
            }
            ev = relay_swarm.select_next_some() => {
                last_relay = Some(format!("{ev:?}"));
                if let SwarmEvent::Behaviour(relay::Event::ReservationReqAccepted { src_peer_id, .. }) = ev {
                    if src_peer_id == *m0.swarm.local_peer_id() {
                        relay_saw_accept = true;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    assert!(
        client_connected,
        "client never connected to relay: last_client={:?} last_relay={:?}",
        last_client, last_relay
    );
    assert!(
        !client_closed,
        "client connection to relay closed before reservation: last_client={:?} last_relay_client={:?} last_relay={:?}",
        last_client, last_relay_client_event, last_relay
    );
    assert!(
        reservation_ok,
        "client did not obtain relay reservation: last_client={:?} last_relay_client={:?} last_relay={:?}",
        last_client, last_relay_client_event, last_relay
    );
    assert!(
        relay_saw_accept,
        "relay did not observe reservation acceptance: last_client={:?} last_relay={:?}",
        last_client, last_relay
    );
    Ok(())
}

/// Sanity check: prove that circuit relay reservation works in this environment
/// using a minimal relay-only client behaviour. This is a debugging guardrail
/// for the more integrated Hypha test above.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_relay_reservation_minimal_tcp() -> Result<(), Box<dyn std::error::Error>> {
    // Relay server (relay-only is sufficient here).
    let mut relay_swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            relay::Behaviour::new(key.public().to_peer_id(), relay::Config::default())
        })?
        .build();

    relay_swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse::<Multiaddr>()?)?;
    let relay_peer = *relay_swarm.local_peer_id();

    let mut relay_listen: Option<Multiaddr> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while relay_listen.is_none() && tokio::time::Instant::now() < deadline {
        if let SwarmEvent::NewListenAddr { address, .. } = relay_swarm.select_next_some().await {
            relay_listen.get_or_insert(address);
        }
    }
    let relay_listen = relay_listen.ok_or("relay did not obtain listen addr")?;
    relay_swarm.add_external_address(relay_listen.clone());
    relay_swarm.add_external_address(relay_listen.clone().with(Protocol::P2p(relay_peer)));

    // Minimal client with relay-client only.
    let mut client = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|_, relay_client| Ok(RelayOnlyBehaviour { relay_client }))?
        .build();

    let relay_circuit = relay_listen
        .clone()
        .with(Protocol::P2p(relay_peer))
        .with(Protocol::P2pCircuit);
    // In the upstream relay tests, `listen_on` triggers the dial/reservation flow.
    client.listen_on(relay_circuit)?;

    // Wait for reservation accepted.
    let mut ok = false;
    let mut last_client: Option<String> = None;
    let mut last_relay: Option<String> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ok && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = client.select_next_some() => {
                last_client = Some(format!("{ev:?}"));
                if let SwarmEvent::Behaviour(RelayOnlyEvent::RelayClient(relay::client::Event::ReservationReqAccepted { relay_peer_id, .. })) = ev {
                    if relay_peer_id == relay_peer {
                        ok = true;
                    }
                }
            }
            ev = relay_swarm.select_next_some() => {
                last_relay = Some(format!("{ev:?}"));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    assert!(
        ok,
        "minimal relay client did not obtain reservation: last_client={:?} last_relay={:?}",
        last_client, last_relay
    );
    Ok(())
}
