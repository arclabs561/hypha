use hypha::{Capability, EnergyStatus, Task};
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
