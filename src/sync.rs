use libp2p::gossipsub;
use serde::{Deserialize, Serialize};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Map, ReadTxn, StateVector, Transact, Update};

/// Distributed State synchronization via CRDTs (Yrs) over Gossipsub.
pub struct SharedState {
    pub doc: Doc,
    pub topic: gossipsub::IdentTopic,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SyncMessage {
    /// Broadcast a document update (delta)
    Update(Vec<u8>),
    /// Request missing updates (SyncStep 1)
    SyncStep1(Vec<u8>), // StateVector
    /// Reply with updates (SyncStep 2)
    SyncStep2(Vec<u8>), // Update
}

impl SharedState {
    pub fn new(topic_name: &str) -> Self {
        Self {
            doc: Doc::new(),
            topic: gossipsub::IdentTopic::new(topic_name),
        }
    }

    /// Apply an incoming update from the network
    pub fn apply_update(&self, update: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let mut txn = self.doc.transact_mut();
        let update = Update::decode_v1(update)?;
        txn.apply_update(update)?;
        Ok(())
    }

    /// Generate a local update to broadcast
    /// This should be called when local changes are made to the doc.
    pub fn get_update_since(&self, sv: &StateVector) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(sv)
    }

    /// Create a message to start a sync with a peer (send our StateVector)
    pub fn create_sync_step_1(&self) -> SyncMessage {
        let txn = self.doc.transact();
        let sv = txn.state_vector().encode_v1();
        SyncMessage::SyncStep1(sv)
    }

    /// Handle a sync step 1 message (reply with missing updates)
    pub fn handle_sync_step_1(
        &self,
        sv_bytes: &[u8],
    ) -> Result<SyncMessage, Box<dyn std::error::Error>> {
        let sv = StateVector::decode_v1(sv_bytes)?;
        let update = self.get_update_since(&sv);
        Ok(SyncMessage::SyncStep2(update))
    }

    /// Handle a sync step 2 message (apply updates)
    pub fn handle_sync_step_2(
        &self,
        update_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.apply_update(update_bytes)
    }

    /// Update a peer's status in the global "peers" map
    pub fn update_peer_status(&self, peer_id: &str, status: &str) {
        let mut txn = self.doc.transact_mut();
        let peers = self.doc.get_or_insert_map("peers");
        peers.insert(&mut txn, peer_id, status);
    }
}
