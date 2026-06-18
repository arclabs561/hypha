use hypha::{Capability, EnergyFacts, EnergyStatus, Task};
use serde_json::json;

#[test]
fn test_energy_status_schema_lock() {
    // This test ensures backward compatibility.
    // If you change the struct, this JSON might fail to deserialize.
    // Think carefully before breaking this contract.
    let legacy_json = json!({
        "source_id": "node-123",
        "energy_score": 0.85
    });

    let status: EnergyStatus =
        serde_json::from_value(legacy_json).expect("Schema break: EnergyStatus");
    assert_eq!(status.source_id, "node-123");
    assert!((status.energy_score - 0.85).abs() < f32::EPSILON);
    assert!(status.facts.is_none());
}

#[test]
fn test_energy_status_omits_absent_facts() {
    let value = serde_json::to_value(EnergyStatus::new("node-123".to_string(), 0.85))
        .expect("EnergyStatus should serialize");

    assert_eq!(value["source_id"], "node-123");
    assert!((value["energy_score"].as_f64().unwrap() - 0.85).abs() < 1e-6);
    assert!(value.get("facts").is_none());
}

#[test]
fn test_energy_status_accepts_optional_facts() {
    let status = EnergyStatus::new("node-123".to_string(), 0.85).with_facts(EnergyFacts {
        state_of_charge: Some(0.85),
        is_mains: Some(false),
        mah_remaining: Some(1200.0),
        projected_drain_mah_per_hour: None,
    });

    let value = serde_json::to_value(&status).expect("EnergyStatus should serialize");
    assert_eq!(value["source_id"], "node-123");
    assert!((value["energy_score"].as_f64().unwrap() - 0.85).abs() < 1e-6);
    assert!((value["facts"]["state_of_charge"].as_f64().unwrap() - 0.85).abs() < 1e-6);
    assert_eq!(value["facts"]["is_mains"], false);
    assert_eq!(value["facts"]["mah_remaining"], 1200.0);
    assert!(value["facts"].get("projected_drain_mah_per_hour").is_none());

    let roundtrip: EnergyStatus =
        serde_json::from_value(value).expect("EnergyStatus facts should deserialize");
    let facts = roundtrip.facts.expect("facts should be present");
    assert_eq!(facts.is_mains, Some(false));
    assert_eq!(facts.mah_remaining, Some(1200.0));
}

#[test]
fn test_task_schema_lock() {
    // Lock the Task schema.
    let legacy_json = json!({
        "id": "task-abc",
        "required_capability": { "Compute": 100 },
        "priority": 5,
        "reach_intensity": 1.0,
        "source_id": "origin-node",
        "auth_token": "some-token"
    });

    let task: Task = serde_json::from_value(legacy_json).expect("Schema break: Task");
    assert_eq!(task.id, "task-abc");
    assert_eq!(task.priority, 5);
    // Ensure Enum representation is correct (External tagging is default)
    match task.required_capability {
        Capability::Compute(v) => assert_eq!(v, 100),
        _ => panic!("Wrong capability variant"),
    }
}
