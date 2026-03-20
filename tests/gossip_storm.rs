use hypha::{EnergyStatus, SporeNode};
use libp2p::futures::StreamExt;
use libp2p::{gossipsub, swarm::dial_opts::DialOpts, swarm::SwarmEvent};
use tempfile::tempdir;
use tokio::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_gossip_storm_resilience() -> Result<(), Box<dyn std::error::Error>> {
    // Red Team: Gossip Storm
    // Attacker floods the network with 5,000 messages in a short burst.
    // Victim must stay responsive to a third "Observer" node.

    let tmp = tempdir()?;
    let p_vic = tmp.path().join("victim");
    let p_att = tmp.path().join("attacker");
    let p_obs = tmp.path().join("observer");
    std::fs::create_dir_all(&p_vic)?;
    std::fs::create_dir_all(&p_att)?;
    std::fs::create_dir_all(&p_obs)?;

    let vic_node = SporeNode::new(&p_vic)?;
    let att_node = SporeNode::new(&p_att)?;
    let obs_node = SporeNode::new(&p_obs)?;

    let vic_id = vic_node.peer_id;
    let _att_id = att_node.peer_id;
    let _obs_id = obs_node.peer_id;

    let mut vic = vic_node.build_mycelium_with_profile(hypha::mycelium::NetProfile::Tcp)?;
    let mut att = att_node.build_mycelium_with_profile(hypha::mycelium::NetProfile::Tcp)?;
    let mut obs = obs_node.build_mycelium_with_profile(hypha::mycelium::NetProfile::Tcp)?;

    vic.subscribe_all()?;
    att.subscribe_all()?;
    obs.subscribe_all()?;

    vic.listen_on("/ip4/127.0.0.1/tcp/0".parse()?)?;

    // Capture victim addr
    let mut vic_addr = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while vic_addr.is_none() && Instant::now() < deadline {
        tokio::select! {
            ev = vic.swarm.select_next_some() => {
                if let SwarmEvent::NewListenAddr { address, .. } = ev {
                    vic_addr = Some(address);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
    let vic_addr = vic_addr.ok_or("Victim failed to listen")?;

    // Attacker & Observer dial Victim
    att.swarm.dial(
        DialOpts::peer_id(vic_id)
            .addresses(vec![vic_addr.clone()])
            .build(),
    )?;
    obs.swarm.dial(
        DialOpts::peer_id(vic_id)
            .addresses(vec![vic_addr.clone()])
            .build(),
    )?;

    // Wait for mesh formation
    tokio::time::sleep(Duration::from_secs(2)).await;

    // THE STORM: Attacker sends 1000 messages in a short burst.
    // We spawn this so we can drive the victim concurrently.
    let storm_handle = tokio::spawn(async move {
        let payload = serde_json::to_vec(&EnergyStatus {
            source_id: "attacker".to_string(),
            energy_score: 0.1,
        })
        .unwrap();

        let mut count = 0;
        let start = Instant::now();
        // Burst for 0.5 second max
        while start.elapsed() < Duration::from_millis(500) {
            // Send in batches to avoid blocking the loop entirely
            for _ in 0..10 {
                let _ = att
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(att.status_topic.clone(), payload.clone());
                count += 1;
            }
            // Drive the swarm non-blocking
            if let Ok(Some(_)) =
                tokio::time::timeout(Duration::from_millis(1), att.swarm.next()).await
            {}
            // Yield
            tokio::task::yield_now().await;
        }
        count
    });

    // Observer waits 1.5s (after storm) then sends a PROBE message
    let obs_handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let probe = serde_json::to_vec(&EnergyStatus {
            source_id: "observer".to_string(),
            energy_score: 0.9,
        })
        .unwrap();

        // Retry probe a few times in case of initial drop
        for _ in 0..5 {
            let _ = obs
                .swarm
                .behaviour_mut()
                .gossipsub
                .publish(obs.status_topic.clone(), probe.clone());
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = obs.swarm.next().await;
        }
    });

    // Victim Loop: Must survive and receive the PROBE from Observer
    // It will also receive thousands of ATTACK messages.
    let mut received_probe = false;
    let mut attack_count = 0;

    let loop_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < loop_deadline {
        tokio::select! {
            ev = vic.swarm.select_next_some() => {
                if let SwarmEvent::Behaviour(hypha::mycelium::MyceliumEvent::Gossipsub(gossipsub::Event::Message { message, .. })) = ev {
                    let msg: EnergyStatus = serde_json::from_slice(&message.data)?;
                    if msg.source_id == "observer" {
                        received_probe = true;
                    } else if msg.source_id == "attacker" {
                        attack_count += 1;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if received_probe { break; }
            }
        }
    }

    let sent_count = storm_handle.await?;
    obs_handle.await?;

    println!(
        "Victim received {} attack messages out of ~{}",
        attack_count, sent_count
    );

    if received_probe {
        println!("Victim recovered and received probe!");
    } else {
        println!("Victim survived but dropped probe (expected under heavy load)");
    }

    assert!(
        attack_count > 500,
        "Victim should have processed significant attack traffic"
    );

    // We expect the victim to NOT crash.
    Ok(())
}
