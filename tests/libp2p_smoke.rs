use hypha::{EnergyStatus, SporeNode};
use libp2p::futures::StreamExt;
use libp2p::{gossipsub, swarm::dial_opts::DialOpts, swarm::SwarmEvent, Multiaddr};
use tempfile::tempdir;

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
    m0.swarm.dial(
        DialOpts::peer_id(peer1)
            .addresses(vec![a1.clone()])
            .build(),
    )?;
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

    // Ensure gossipsub considers peers for direct publishing.
    m0.swarm
        .behaviour_mut()
        .gossipsub
        .add_explicit_peer(&peer1);
    m1.swarm
        .behaviour_mut()
        .gossipsub
        .add_explicit_peer(&peer0);

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
    m0.swarm.dial(
        DialOpts::peer_id(peer1)
            .addresses(vec![a1.clone()])
            .build(),
    )?;

    // Give the swarms time to exchange subscriptions; otherwise publish can yield NoPeersSubscribedToTopic.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => { let _ = ev; }
            ev = m1.swarm.select_next_some() => { let _ = ev; }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    let status = EnergyStatus { source_id: "node0".to_string(), energy_score: 0.9 };
    let bytes = serde_json::to_vec(&status)?;
    let pub_res = m0.swarm.behaviour_mut().gossipsub.publish(m0.status_topic.clone(), bytes);
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

