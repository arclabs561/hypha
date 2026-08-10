use hypha::sync::{SharedState, SyncMessage};
use yrs::{GetString, ReadTxn, StateVector, Text, Transact};

fn full_update(state: &SharedState) -> Vec<u8> {
    state.get_update_since(&StateVector::default())
}

fn notes(state: &SharedState) -> String {
    let notes = state.doc.get_or_insert_text("notes");
    let txn = state.doc.transact();
    notes.get_string(&txn)
}

fn state_vector(state: &SharedState) -> StateVector {
    state.doc.transact().state_vector()
}

fn sync_step_1_bytes(message: SyncMessage) -> Vec<u8> {
    match message {
        SyncMessage::SyncStep1(bytes) => bytes,
        _ => panic!("expected sync step 1"),
    }
}

fn sync_step_2_bytes(message: SyncMessage) -> Vec<u8> {
    match message {
        SyncMessage::SyncStep2(bytes) => bytes,
        _ => panic!("expected sync step 2"),
    }
}

#[test]
fn remote_state_vector_does_not_acknowledge_or_mutate_local_state() {
    let requester = SharedState::new("requester");
    let responder = SharedState::new("responder");
    responder.update_peer_status("peer-a", "ready");

    let responder_before = state_vector(&responder);
    let request = sync_step_1_bytes(requester.create_sync_step_1());
    let response = responder.handle_sync_step_1(&request).unwrap();

    assert_eq!(state_vector(&responder), responder_before);
    assert_eq!(state_vector(&requester), StateVector::default());

    requester
        .handle_sync_step_2(&sync_step_2_bytes(response))
        .unwrap();
    assert_eq!(state_vector(&requester), responder_before);
}

#[test]
fn stale_duplicate_insert_cannot_resurrect_observed_removal() {
    let author = SharedState::new("author");
    {
        let notes = author.doc.get_or_insert_text("notes");
        let mut txn = author.doc.transact_mut();
        notes.push(&mut txn, "alive");
    }
    let insert = full_update(&author);

    let remover = SharedState::new("remover");
    remover.apply_update(&insert).unwrap();
    let before_removal = state_vector(&remover);
    {
        let notes = remover.doc.get_or_insert_text("notes");
        let mut txn = remover.doc.transact_mut();
        notes.remove_range(&mut txn, 0, 5);
    }
    let removal = remover.get_update_since(&before_removal);

    let replica = SharedState::new("replica");
    replica.apply_update(&removal).unwrap();
    replica.apply_update(&insert).unwrap();
    replica.apply_update(&insert).unwrap();

    assert_eq!(notes(&replica), "");
}

#[test]
fn delivery_permutation_and_duplication_converge() {
    let left = SharedState::new("left");
    let right = SharedState::new("right");
    {
        let notes = left.doc.get_or_insert_text("notes");
        let mut txn = left.doc.transact_mut();
        notes.push(&mut txn, "left");
    }
    {
        let notes = right.doc.get_or_insert_text("notes");
        let mut txn = right.doc.transact_mut();
        notes.push(&mut txn, "right");
    }

    let updates = [full_update(&left), full_update(&right)];
    let deliveries = [
        [0, 1, 0, 1],
        [0, 1, 1, 0],
        [1, 0, 0, 1],
        [1, 0, 1, 0],
        [0, 0, 1, 1],
        [1, 1, 0, 0],
    ];

    let mut results = Vec::new();
    for order in deliveries {
        let replica = SharedState::new("replica");
        for update in order {
            replica.apply_update(&updates[update]).unwrap();
        }
        results.push(notes(&replica));
    }

    assert!(results.iter().all(|result| result == &results[0]));
    assert_eq!(results[0].len(), "leftright".len());
}
