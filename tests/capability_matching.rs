use hypha::{Bid, Capability, MockMetabolism, SporeNode, Task};
use proptest::prelude::*;
use std::sync::{Arc, Mutex};
use tempfile::{tempdir, TempDir};

fn compute_node(available: u32, energy: f32) -> (TempDir, SporeNode) {
    let tmp = tempdir().unwrap();
    let metabolism = Arc::new(Mutex::new(MockMetabolism::new(energy, false)));
    let mut node = SporeNode::new_with_metabolism(tmp.path(), metabolism).unwrap();
    node.add_capability(Capability::Compute(available));
    (tmp, node)
}

fn compute_task(required: u32) -> Task {
    Task {
        id: format!("compute-{required}"),
        required_capability: Capability::Compute(required),
        priority: 1,
        reach_intensity: 1.0,
        source_id: "test-source".to_string(),
        auth_token: None,
    }
}

fn compute_task_with_reach(required: u32, reach_intensity: f32) -> Task {
    Task {
        reach_intensity,
        ..compute_task(required)
    }
}

#[test]
fn evaluate_task_accepts_sufficient_compute_capacity() {
    let (_tmp, node) = compute_node(101, 1.0);
    let task = compute_task(50);

    let bid = node.evaluate_task(&task, 0).unwrap();

    assert_eq!(bid.task_id, "compute-50");
    assert_eq!(bid.energy_score, 1.0);
}

#[test]
fn evaluate_task_rejects_insufficient_compute_capacity() {
    let (_tmp, node) = compute_node(49, 1.0);
    let task = compute_task(50);

    assert!(node.evaluate_task(&task, 0).is_none());
}

#[test]
fn process_task_bundle_accepts_sufficient_compute_capacity() {
    let (_tmp, node) = compute_node(100, 1.0);
    let task = compute_task(50);
    let mut bids = Vec::new();

    let bid = node.process_task_bundle(&task, &mut bids).unwrap();

    assert_eq!(bid.task_id, "compute-50");
    assert_eq!(bids.len(), 1);
    assert_eq!(bids[0].task_id, "compute-50");
}

#[test]
fn process_task_bundle_rejects_insufficient_compute_capacity() {
    let (_tmp, node) = compute_node(49, 1.0);
    let task = compute_task(50);
    let mut bids = Vec::new();

    assert!(node.process_task_bundle(&task, &mut bids).is_none());
    assert!(bids.is_empty());
}

#[test]
fn process_task_bundle_still_respects_better_known_bid() {
    let (_tmp, node) = compute_node(100, 0.8);
    let task = compute_task(50);
    let mut bids = vec![Bid {
        task_id: "compute-50".to_string(),
        bidder_id: "peer-a".to_string(),
        energy_score: 0.9,
        cost_mah: 1.0,
    }];

    assert!(node.process_task_bundle(&task, &mut bids).is_none());
    assert_eq!(bids.len(), 1);
}

#[test]
fn process_task_bundle_compares_the_reach_adjusted_bid_score() {
    let (_tmp, node) = compute_node(100, 1.0);
    let task = compute_task_with_reach(50, 0.5);
    let mut bids = vec![Bid {
        task_id: "compute-50".to_string(),
        bidder_id: "peer-a".to_string(),
        energy_score: 0.9,
        cost_mah: 1.0,
    }];

    assert!(node.process_task_bundle(&task, &mut bids).is_none());
    assert_eq!(bids.len(), 1);
}

#[test]
fn process_task_bundle_ignores_non_finite_known_bid_scores() {
    let (_tmp, node) = compute_node(100, 1.0);
    let task = compute_task(50);
    let mut bids = vec![Bid {
        task_id: "compute-50".to_string(),
        bidder_id: "peer-a".to_string(),
        energy_score: f32::NAN,
        cost_mah: 1.0,
    }];

    let bid = node.process_task_bundle(&task, &mut bids).unwrap();

    assert_eq!(bid.energy_score, 1.0);
    assert_eq!(bids.len(), 2);
}

#[test]
fn evaluate_task_rejects_too_little_reach() {
    let (_tmp, node) = compute_node(100, 1.0);
    let task = compute_task_with_reach(50, 0.09);

    assert!(node.evaluate_task(&task, 0).is_none());
}

#[test]
fn process_task_bundle_rejects_low_energy_before_bidding() {
    let (_tmp, node) = compute_node(100, 0.19);
    let task = compute_task(50);
    let mut bids = Vec::new();

    assert!(node.process_task_bundle(&task, &mut bids).is_none());
    assert!(bids.is_empty());
}

#[test]
fn sensing_capabilities_remain_exact_labels() {
    let tmp = tempdir().unwrap();
    let mut node = SporeNode::new(tmp.path()).unwrap();
    node.add_capability(Capability::Sensing("thermal".to_string()));

    let matching = Task::new(
        "thermal".to_string(),
        Capability::Sensing("thermal".to_string()),
        1,
        "src".to_string(),
    );
    let synonym = Task::new(
        "temperature".to_string(),
        Capability::Sensing("temperature".to_string()),
        1,
        "src".to_string(),
    );

    assert!(node.evaluate_task(&matching, 0).is_some());
    assert!(node.evaluate_task(&synonym, 0).is_none());
}

proptest! {
    #[test]
    fn compute_satisfaction_matches_capacity_order(available in any::<u32>(), required in any::<u32>()) {
        prop_assert_eq!(
            Capability::Compute(available).satisfies(&Capability::Compute(required)),
            available >= required
        );
    }

    #[test]
    fn storage_satisfaction_matches_capacity_order(available in any::<u64>(), required in any::<u64>()) {
        prop_assert_eq!(
            Capability::Storage(available).satisfies(&Capability::Storage(required)),
            available >= required
        );
    }
}
