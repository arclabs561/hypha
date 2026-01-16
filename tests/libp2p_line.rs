use hypha::{EnergyStatus, SporeNode};
use libp2p::futures::StreamExt;
use libp2p::{gossipsub, swarm::dial_opts::DialOpts, swarm::SwarmEvent, Multiaddr, PeerId};
use tempfile::tempdir;

async fn run_line(profile: hypha::mycelium::NetProfile, listen0: &str, listen1: &str, listen2: &str) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let p0 = tmp.path().join("n0");
    let p1 = tmp.path().join("n1");
    let p2 = tmp.path().join("n2");
    std::fs::create_dir_all(&p0)?;
    std::fs::create_dir_all(&p1)?;
    std::fs::create_dir_all(&p2)?;

    let n0 = SporeNode::new(&p0)?;
    let n1 = SporeNode::new(&p1)?;
    let n2 = SporeNode::new(&p2)?;

    let peer0: PeerId = n0.peer_id;
    let peer1: PeerId = n1.peer_id;
    let peer2: PeerId = n2.peer_id;

    let mut m0 = n0.build_mycelium_with_profile(profile)?;
    let mut m1 = n1.build_mycelium_with_profile(profile)?;
    let mut m2 = n2.build_mycelium_with_profile(profile)?;
    m0.subscribe_all()?;
    m1.subscribe_all()?;
    m2.subscribe_all()?;

    m0.listen_on(listen0.parse::<Multiaddr>()?)?;
    m1.listen_on(listen1.parse::<Multiaddr>()?)?;
    m2.listen_on(listen2.parse::<Multiaddr>()?)?;

    // Capture listen addrs.
    let mut a0: Option<Multiaddr> = None;
    let mut a1: Option<Multiaddr> = None;
    let mut a2: Option<Multiaddr> = None;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(800);
    while (a0.is_none() || a1.is_none() || a2.is_none()) && tokio::time::Instant::now() < deadline {
        tokio::select! {
            ev = m0.swarm.select_next_some() => { if let SwarmEvent::NewListenAddr{address, ..} = ev { a0.get_or_insert(address); } }
            ev = m1.swarm.select_next_some() => { if let SwarmEvent::NewListenAddr{address, ..} = ev { a1.get_or_insert(address); } }
            ev = m2.swarm.select_next_some() => { if let SwarmEvent::NewListenAddr{address, ..} = ev { a2.get_or_insert(address); } }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    let a0 = a0.ok_or("n0 no listen addr")?;
    let a1 = a1.ok_or("n1 no listen addr")?;
    let a2 = a2.ok_or("n2 no listen addr")?;

    // Connect in a line: n0<->n1<->n2.
    m0.swarm.dial(DialOpts::peer_id(peer1).addresses(vec![a1.clone()]).build())?;
    m2.swarm.dial(DialOpts::peer_id(peer1).addresses(vec![a1.clone()]).build())?;
    m1.swarm.dial(DialOpts::peer_id(peer0).addresses(vec![a0.clone()]).build())?;
    m1.swarm.dial(DialOpts::peer_id(peer2).addresses(vec![a2.clone()]).build())?;

    // Encourage forwarding: make peers explicit.
    for (sw, peers) in [
        (&mut m0, vec![peer1]),
        (&mut m1, vec![peer0, peer2]),
        (&mut m2, vec![peer1]),
    ] {
        for p in peers {
            sw.swarm.behaviour_mut().gossipsub.add_explicit_peer(&p);
        }
    }

    // Let subscriptions propagate.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(800);
    while tokio::time::Instant::now() < deadline {
        tokio::select! {
            _ = m0.swarm.select_next_some() => {}
            _ = m1.swarm.select_next_some() => {}
            _ = m2.swarm.select_next_some() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }

    // Publish from n0.
    let status = EnergyStatus { source_id: "n0".to_string(), energy_score: 0.9 };
    let bytes = serde_json::to_vec(&status)?;
    let pub_res = m0.swarm.behaviour_mut().gossipsub.publish(m0.status_topic.clone(), bytes);
    assert!(pub_res.is_ok(), "publish failed: {:?}", pub_res);

    // Wait for n2 to receive. If n1 sees the message first, explicitly relay it.
    // This is intentionally an application-level relay (not "pure gossipsub forwarding"):
    // it models a nested overlay where intermediate nodes can choose to amplify
    // or suppress traffic based on local policy (energy/pressure, etc.).
    let mut relayed = false;
    let mut received = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !received && tokio::time::Instant::now() < deadline {
        tokio::select! {
            _ = m0.swarm.select_next_some() => {}
            ev = m1.swarm.select_next_some() => {
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Message { message, .. })) = ev {
                    if message.topic == m1.status_topic.hash() && !relayed {
                        // Relay once.
                        let mut last_err = None;
                        for _ in 0..10 {
                            match m1.swarm.behaviour_mut().gossipsub.publish(m1.status_topic.clone(), message.data.clone()) {
                                Ok(_) => { relayed = true; break; }
                                Err(e) => {
                                    last_err = Some(e);
                                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                }
                            }
                        }
                        assert!(relayed, "relay publish failed: {:?}", last_err);
                    }
                }
            }
            ev = m2.swarm.select_next_some() => {
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Message { message, .. })) = ev {
                    if message.topic == m2.status_topic.hash() {
                        let _p: EnergyStatus = serde_json::from_slice(&message.data)?;
                        received = true;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    assert!(received, "end node did not receive status over line");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_line_tcp() -> Result<(), Box<dyn std::error::Error>> {
    run_line(
        hypha::mycelium::NetProfile::Tcp,
        "/ip4/127.0.0.1/tcp/0",
        "/ip4/127.0.0.1/tcp/0",
        "/ip4/127.0.0.1/tcp/0",
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_line_quic() -> Result<(), Box<dyn std::error::Error>> {
    run_line(
        hypha::mycelium::NetProfile::TcpQuic,
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "/ip4/127.0.0.1/udp/0/quic-v1",
        "/ip4/127.0.0.1/udp/0/quic-v1",
    )
    .await
}

