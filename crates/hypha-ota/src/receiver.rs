//! OTA receiver state machine.
//!
//! Pure state transitions — no hardware deps. The caller handles actual
//! flash writes and ESP-NOW sends based on the returned `OtaAction`.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use sha2::Digest;

use crate::protocol::{self, CHUNK_SIZE, MAX_CHUNKS};

/// Current state of the OTA receiver.
#[derive(Debug)]
pub enum OtaState {
    /// Waiting for a manifest broadcast.
    Idle,
    /// Actively receiving chunks from a sender.
    Receiving {
        sender: [u8; 6],
        version: String,
        n_chunks: u32,
        hash_hex: String,
        next_chunk: u32,
        hasher: sha2::Sha256,
    },
    /// Transfer complete, hash verified.
    Verified {
        version: String,
    },
    /// Transfer failed.
    Failed {
        reason: &'static str,
    },
}

/// Actions the caller should take after a state transition.
#[derive(Debug, PartialEq, Eq)]
pub enum OtaAction {
    /// No action needed.
    None,
    /// Erase the OTA partition (image_len bytes).
    ErasePartition { image_len: u32 },
    /// Send a chunk request to the sender.
    RequestChunk { index: u32 },
    /// Write a chunk to flash at the given offset.
    WriteChunk { index: u32, offset: u32 },
    /// Image is complete and hash-verified. Write otadata and reboot.
    ApplyAndReboot,
    /// Transfer failed; log reason.
    Abort { reason: &'static str },
}

/// An incoming event for the receiver.
pub enum OtaEvent<'a> {
    /// Received a manifest broadcast.
    Manifest {
        sender: [u8; 6],
        json: &'a [u8],
        pubkey: &'a [u8; 32],
    },
    /// Received a chunk response.
    Chunk {
        sender: [u8; 6],
        json: &'a [u8],
    },
    /// Erase completed (caller confirms).
    EraseComplete,
    /// Write completed (caller confirms).
    WriteComplete,
}

/// Result of processing an event: new state + list of actions + optional chunk data.
pub struct OtaTransition {
    pub state: OtaState,
    pub actions: Vec<OtaAction>,
    /// Chunk data extracted from a chunk response (caller writes this to flash).
    pub chunk_data: Option<Vec<u8>>,
}

impl OtaState {
    /// Create a new idle receiver.
    pub fn new() -> Self {
        OtaState::Idle
    }

    /// Process an incoming event and return the new state + actions.
    pub fn process(self, event: OtaEvent<'_>) -> OtaTransition {
        match (self, event) {
            // Idle + Manifest → start receiving
            (OtaState::Idle, OtaEvent::Manifest { sender, json, pubkey }) => {
                match protocol::verify_manifest_json_full(json, pubkey) {
                    Some((version, n_chunks, hash_hex, _sig)) => {
                        if n_chunks == 0 || n_chunks > MAX_CHUNKS {
                            return OtaTransition {
                                state: OtaState::Idle,
                                actions: vec![OtaAction::Abort { reason: "n_chunks out of range" }],
                                chunk_data: None,
                            };
                        }
                        let image_len = n_chunks * CHUNK_SIZE as u32;
                        OtaTransition {
                            state: OtaState::Receiving {
                                sender,
                                version,
                                n_chunks,
                                hash_hex,
                                next_chunk: 0,
                                hasher: sha2::Sha256::new(),
                            },
                            actions: vec![
                                OtaAction::ErasePartition { image_len },
                                OtaAction::RequestChunk { index: 0 },
                            ],
                            chunk_data: None,
                        }
                    }
                    None => OtaTransition {
                        state: OtaState::Idle,
                        actions: vec![],
                        chunk_data: None,
                    },
                }
            }

            // Receiving + Chunk → write, hash, advance
            (
                OtaState::Receiving {
                    sender,
                    version,
                    n_chunks,
                    hash_hex,
                    next_chunk,
                    mut hasher,
                },
                OtaEvent::Chunk {
                    sender: chunk_sender,
                    json,
                },
            ) => {
                // Ignore chunks from wrong sender
                if chunk_sender != sender {
                    return OtaTransition {
                        state: OtaState::Receiving {
                            sender,
                            version,
                            n_chunks,
                            hash_hex,
                            next_chunk,
                            hasher,
                        },
                        actions: vec![],
                        chunk_data: None,
                    };
                }

                match protocol::parse_chunk_response(json) {
                    Some((idx, data)) if idx == next_chunk => {
                        hasher.update(&data);
                        let offset = idx * CHUNK_SIZE as u32;
                        let new_next = next_chunk + 1;

                        if new_next == n_chunks {
                            // Last chunk — verify hash
                            let digest = hasher.finalize();
                            let expected = hex::decode(&hash_hex).unwrap_or_default();
                            if expected.len() == 32 && digest.as_slice() == expected.as_slice() {
                                OtaTransition {
                                    state: OtaState::Verified {
                                        version: version.clone(),
                                    },
                                    actions: vec![
                                        OtaAction::WriteChunk { index: idx, offset },
                                        OtaAction::ApplyAndReboot,
                                    ],
                                    chunk_data: Some(data),
                                }
                            } else {
                                OtaTransition {
                                    state: OtaState::Failed {
                                        reason: "hash mismatch",
                                    },
                                    actions: vec![OtaAction::Abort {
                                        reason: "hash mismatch",
                                    }],
                                    chunk_data: None,
                                }
                            }
                        } else {
                            OtaTransition {
                                state: OtaState::Receiving {
                                    sender,
                                    version,
                                    n_chunks,
                                    hash_hex,
                                    next_chunk: new_next,
                                    hasher,
                                },
                                actions: vec![
                                    OtaAction::WriteChunk { index: idx, offset },
                                    OtaAction::RequestChunk { index: new_next },
                                ],
                                chunk_data: Some(data),
                            }
                        }
                    }
                    // Wrong index or parse failure — ignore (will re-request on timeout)
                    _ => OtaTransition {
                        state: OtaState::Receiving {
                            sender,
                            version,
                            n_chunks,
                            hash_hex,
                            next_chunk,
                            hasher,
                        },
                        actions: vec![],
                        chunk_data: None,
                    },
                }
            }

            // Already receiving + new manifest → ignore (don't interrupt active transfer)
            (
                state @ OtaState::Receiving { .. },
                OtaEvent::Manifest { .. },
            ) => OtaTransition {
                state,
                actions: vec![],
                chunk_data: None,
            },

            // Any other combination → no-op
            (state, _) => OtaTransition {
                state,
                actions: vec![],
                chunk_data: None,
            },
        }
    }
}

impl Default for OtaState {
    fn default() -> Self {
        Self::new()
    }
}
