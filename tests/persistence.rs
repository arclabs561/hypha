use hypha::SporeNode;
use tempfile::tempdir;

#[test]
fn test_identity_and_messages_persist_across_restart() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let p = tmp.path().join("node");
    std::fs::create_dir_all(&p)?;

    // First run: create identity + persist a message.
    let n0 = SporeNode::new(&p)?;
    let peer0 = n0.peer_id;
    n0.simulate_receive("m1", b"hello")?;
    assert_eq!(n0.message_count(), 1);

    // Drop and "restart" using same storage path.
    drop(n0);
    let n1 = SporeNode::new(&p)?;

    // Identity is persisted.
    assert_eq!(n1.peer_id, peer0);

    // Message persisted under msg_ prefix survives restart.
    assert_eq!(n1.message_count(), 1);
    let ids = n1.message_ids();
    assert!(
        ids.iter().any(|k| k.ends_with("msg_m1") || k == "msg_m1"),
        "expected msg key to survive restart; ids={ids:?}"
    );

    // Payload survives restart.
    let bytes = n1.db.get("msg_m1")?.ok_or("expected msg_m1 value")?;
    assert_eq!(bytes.as_ref(), b"hello");

    Ok(())
}
