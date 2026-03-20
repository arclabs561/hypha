//! OTA protocol simulation tests.
//!
//! Tests the full OTA lifecycle — manifest signing/verification, chunk framing,
//! streaming hash, flash alignment, receiver state machine, and multi-node
//! propagation — all on host without any hardware.

use hypha_ota::flash_fmt::{self, OtaFlash};
use hypha_ota::protocol::{self, CHUNK_SIZE, MAX_CHUNKS};
use hypha_ota::receiver::{OtaAction, OtaEvent, OtaState};

use ed25519_dalek::{Signer, SigningKey};
use sha2::Digest;

// ---------------------------------------------------------------------------
// Cross-validation: signing payload format matches between tools
// ---------------------------------------------------------------------------

/// Reproduce the signing tool's `build_payload` function independently.
/// If this ever diverges from `hypha_ota::protocol::build_signing_payload`,
/// manifests signed by the tool won't verify on the receiver.
fn signing_tool_build_payload(version: &str, hash: &[u8], n_chunks: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    let vbytes = version.as_bytes();
    payload.extend_from_slice(&(vbytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(vbytes);
    payload.extend_from_slice(hash);
    payload.extend_from_slice(&n_chunks.to_be_bytes());
    payload
}

#[test]
fn signing_payload_format_matches_protocol() {
    // If this test fails, the signing tool and the receiver are using different
    // payload formats — manifests will fail to verify.
    let version = "1.2.3-rc1";
    let hash = [0xABu8; 32];
    let n_chunks = 42u32;

    let tool_payload = signing_tool_build_payload(version, &hash, n_chunks);
    let lib_payload = protocol::build_signing_payload(version, &hash, n_chunks);

    assert_eq!(
        tool_payload, lib_payload,
        "signing tool and hypha_ota::protocol must produce identical payloads"
    );
}

#[test]
fn signing_payload_format_edge_cases() {
    // Empty version
    assert_eq!(
        signing_tool_build_payload("", &[0u8; 32], 0),
        protocol::build_signing_payload("", &[0u8; 32], 0),
    );
    // Long version
    let long_v = "x".repeat(100);
    assert_eq!(
        signing_tool_build_payload(&long_v, &[0xFF; 32], u32::MAX),
        protocol::build_signing_payload(&long_v, &[0xFF; 32], u32::MAX),
    );
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Generate a test Ed25519 keypair.
fn gen_keypair() -> (SigningKey, [u8; 32]) {
    let signing_key = SigningKey::generate(&mut rand_core::OsRng);
    let pubkey_bytes: [u8; 32] = signing_key.verifying_key().to_bytes();
    (signing_key, pubkey_bytes)
}

/// Create a dummy firmware image of the given size with deterministic content.
fn dummy_image(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Sign a manifest for the given image. Returns (manifest_json, hash_hex).
fn sign_manifest(image: &[u8], version: &str, signing_key: &SigningKey) -> (String, String) {
    let hash_hex = protocol::sha256_hex(image);
    let n_chunks = protocol::n_chunks_for_len(image.len());
    let payload =
        protocol::build_signing_payload(version, &hex::decode(&hash_hex).unwrap(), n_chunks);
    let sig = signing_key.sign(&payload);
    let sig_b64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, sig.to_bytes());
    let manifest = protocol::build_manifest_json(version, &hash_hex, n_chunks, &sig_b64);
    (manifest, hash_hex)
}

// ---------------------------------------------------------------------------
// MockFlash: simulates SPI NOR flash with erase/write/read semantics
// ---------------------------------------------------------------------------

struct MockFlash {
    storage: Vec<u8>,
    erase_count: u32,
    write_count: u32,
    /// If set, inject an error at this write offset.
    fault_at_write: Option<u32>,
}

impl MockFlash {
    fn new(size: usize) -> Self {
        MockFlash {
            storage: vec![0xFF; size],
            erase_count: 0,
            write_count: 0,
            fault_at_write: None,
        }
    }

    /// Read a slice of flash (convenience for tests).
    fn read_slice(&self, offset: u32, len: usize) -> &[u8] {
        let o = offset as usize;
        &self.storage[o..o + len]
    }
}

#[derive(Debug)]
struct MockFlashError(&'static str);

impl core::fmt::Display for MockFlashError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "MockFlashError: {}", self.0)
    }
}

impl OtaFlash for MockFlash {
    type Error = MockFlashError;

    fn erase(&mut self, start_offset: u32, len: u32) -> Result<(), Self::Error> {
        let s = start_offset as usize;
        let e = s + len as usize;
        if e > self.storage.len() {
            return Err(MockFlashError("erase out of bounds"));
        }
        for b in &mut self.storage[s..e] {
            *b = 0xFF;
        }
        self.erase_count += 1;
        Ok(())
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Self::Error> {
        if let Some(fault_at) = self.fault_at_write {
            if offset == fault_at {
                return Err(MockFlashError("injected write fault"));
            }
        }
        let o = offset as usize;
        if o + data.len() > self.storage.len() {
            return Err(MockFlashError("write out of bounds"));
        }
        // Flash write constraint: can only clear bits (AND operation)
        for (i, &byte) in data.iter().enumerate() {
            let current = self.storage[o + i];
            if current != 0xFF && current != byte {
                return Err(MockFlashError("write to unerased byte"));
            }
            self.storage[o + i] = byte;
        }
        self.write_count += 1;
        Ok(())
    }

    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), Self::Error> {
        let o = offset as usize;
        if o + buf.len() > self.storage.len() {
            return Err(MockFlashError("read out of bounds"));
        }
        buf.copy_from_slice(&self.storage[o..o + buf.len()]);
        Ok(())
    }
}

// ===========================================================================
// Protocol tests: chunk framing
// ===========================================================================

#[test]
fn chunk_response_fits_250_bytes() {
    // Worst case: max chunk size, max index
    let data = vec![0xFF_u8; CHUNK_SIZE];
    let frame = protocol::build_chunk_response(9999, &data);
    assert!(
        frame.len() <= 250,
        "chunk frame {} bytes > 250 byte ESP-NOW limit",
        frame.len()
    );
}

#[test]
fn chunk_request_fits_250_bytes() {
    let frame = protocol::build_chunk_request(9999);
    assert!(
        frame.len() <= 250,
        "chunk request {} bytes > 250 byte ESP-NOW limit",
        frame.len()
    );
}

#[test]
fn manifest_fits_250_bytes() {
    let (sk, _pk) = gen_keypair();
    let image = dummy_image(1024);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    assert!(
        manifest.len() <= 250,
        "manifest {} bytes > 250 byte ESP-NOW limit",
        manifest.len()
    );
}

#[test]
fn chunk_roundtrip_all_byte_values() {
    let data: Vec<u8> = (0..=255_u8).collect();
    for chunk_start in (0..256).step_by(CHUNK_SIZE) {
        let end = (chunk_start + CHUNK_SIZE).min(256);
        let chunk = &data[chunk_start..end];
        let idx = (chunk_start / CHUNK_SIZE) as u32;
        let frame = protocol::build_chunk_response(idx, chunk);
        let (got_idx, got_data) = protocol::parse_chunk_response(frame.as_bytes()).unwrap();
        assert_eq!(got_idx, idx);
        assert_eq!(got_data.as_slice(), chunk);
    }
}

#[test]
fn chunk_request_roundtrip() {
    for idx in [0, 1, 127, 128, 255, 1000, MAX_CHUNKS - 1] {
        let frame = protocol::build_chunk_request(idx);
        let got = protocol::parse_chunk_request(frame.as_bytes()).unwrap();
        assert_eq!(got, idx);
    }
}

#[test]
fn empty_chunk_roundtrip() {
    let frame = protocol::build_chunk_response(0, &[]);
    let (idx, data) = protocol::parse_chunk_response(frame.as_bytes()).unwrap();
    assert_eq!(idx, 0);
    assert!(data.is_empty());
}

#[test]
fn parse_invalid_json_returns_none() {
    assert!(protocol::parse_chunk_response(b"not json").is_none());
    assert!(protocol::parse_chunk_request(b"not json").is_none());
    assert!(protocol::parse_chunk_response(b"{}").is_none());
    assert!(protocol::parse_chunk_request(b"{}").is_none());
    // Wrong "ota" type
    assert!(protocol::parse_chunk_response(br#"{"ota":"req","i":0}"#).is_none());
    assert!(protocol::parse_chunk_request(br#"{"ota":"chunk","i":0,"b":"AA=="}"#).is_none());
}

// ===========================================================================
// Protocol tests: manifest signing and verification
// ===========================================================================

#[test]
fn sign_verify_manifest_roundtrip() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300); // 300 bytes → 3 chunks (128+128+44)
    let (manifest, _hash_hex) = sign_manifest(&image, "1.0.0", &sk);

    let result = protocol::verify_manifest_json(manifest.as_bytes(), &pk);
    assert!(result.is_some(), "valid manifest should verify");
    let (version, n_chunks) = result.unwrap();
    assert_eq!(version, "1.0.0");
    assert_eq!(n_chunks, 3);
}

#[test]
fn manifest_tampered_version_rejected() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);

    let tampered = manifest.replace("1.0.0", "9.9.9");
    assert!(
        protocol::verify_manifest_json(tampered.as_bytes(), &pk).is_none(),
        "tampered version must not verify"
    );
}

#[test]
fn manifest_wrong_pubkey_rejected() {
    let (sk, _pk) = gen_keypair();
    let (_, wrong_pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);

    assert!(
        protocol::verify_manifest_json(manifest.as_bytes(), &wrong_pk).is_none(),
        "wrong pubkey must not verify"
    );
}

#[test]
fn manifest_full_returns_all_fields() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(500);
    let (manifest, hash_hex) = sign_manifest(&image, "2.1.0", &sk);

    let result = protocol::verify_manifest_json_full(manifest.as_bytes(), &pk);
    assert!(result.is_some());
    let (version, n, h, _sig) = result.unwrap();
    assert_eq!(version, "2.1.0");
    assert_eq!(n, protocol::n_chunks_for_len(500));
    assert_eq!(h, hash_hex);
}

// ===========================================================================
// Protocol tests: image hash
// ===========================================================================

#[test]
fn streaming_hash_matches_whole_image() {
    let image = dummy_image(1000);
    let expected = protocol::sha256_hex(&image);

    // Simulate chunk-by-chunk hashing (as the receiver does)
    let mut hasher = sha2::Sha256::new();
    for chunk in image.chunks(CHUNK_SIZE) {
        hasher.update(chunk);
    }
    let streaming = hex::encode(hasher.finalize());

    assert_eq!(streaming, expected);
    assert!(protocol::verify_image_hash(&image, &expected));
}

#[test]
fn image_hash_wrong_hex_rejected() {
    let image = dummy_image(100);
    assert!(!protocol::verify_image_hash(&image, &"0".repeat(64)));
    assert!(!protocol::verify_image_hash(&image, "not hex"));
    assert!(!protocol::verify_image_hash(&image, ""));
}

#[test]
fn image_chunk_slicing() {
    let image = dummy_image(300);
    // Chunk 0: 0..128
    let c0 = protocol::image_chunk(&image, 0).unwrap();
    assert_eq!(c0.len(), CHUNK_SIZE);
    assert_eq!(c0[0], 0);
    assert_eq!(c0[127], 127);
    // Chunk 1: 128..256
    let c1 = protocol::image_chunk(&image, 1).unwrap();
    assert_eq!(c1.len(), CHUNK_SIZE);
    assert_eq!(c1[0], 128);
    // Chunk 2: 256..300 (last chunk, short)
    let c2 = protocol::image_chunk(&image, 2).unwrap();
    assert_eq!(c2.len(), 44);
    assert_eq!(c2[0], 0); // 256 % 256 = 0
                          // Chunk 3: out of range
    assert!(protocol::image_chunk(&image, 3).is_none());
}

#[test]
fn n_chunks_for_len_edge_cases() {
    assert_eq!(protocol::n_chunks_for_len(0), 0);
    assert_eq!(protocol::n_chunks_for_len(1), 1);
    assert_eq!(protocol::n_chunks_for_len(CHUNK_SIZE), 1);
    assert_eq!(protocol::n_chunks_for_len(CHUNK_SIZE + 1), 2);
    assert_eq!(protocol::n_chunks_for_len(CHUNK_SIZE * 3), 3);
    assert_eq!(protocol::n_chunks_for_len(CHUNK_SIZE * 3 + 1), 4);
}

// ===========================================================================
// Flash format tests: CRC32 and otadata
// ===========================================================================

#[test]
fn crc32_known_vectors() {
    // Empty input
    assert_eq!(flash_fmt::crc32(&[]), 0x0000_0000);
    // "123456789" — standard CRC32 test vector
    assert_eq!(flash_fmt::crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn otadata_entry_layout() {
    let entry = flash_fmt::build_otadata_entry();
    assert_eq!(entry.len(), 32);

    // ota_seq = 1 at offset 0..4 (little-endian)
    assert_eq!(u32::from_le_bytes(entry[0..4].try_into().unwrap()), 1);
    // seq_label: 20 bytes of zeros at offset 4..24
    assert_eq!(&entry[4..24], &[0u8; 20]);
    // ota_state = 0 (ESP_OTA_IMG_NEW) at offset 24..28
    assert_eq!(u32::from_le_bytes(entry[24..28].try_into().unwrap()), 0);
    // CRC32 at offset 28..32 must match CRC of first 28 bytes
    let stored_crc = u32::from_le_bytes(entry[28..32].try_into().unwrap());
    let computed_crc = flash_fmt::crc32(&entry[..28]);
    assert_eq!(stored_crc, computed_crc);
}

#[test]
fn otadata_different_seq_produces_different_crc() {
    let e1 = flash_fmt::build_otadata_entry_with_seq(1, 0);
    let e2 = flash_fmt::build_otadata_entry_with_seq(2, 0);
    // Different seq → different CRC
    assert_ne!(e1[28..32], e2[28..32]);
    // Both have valid CRCs
    let crc1 = u32::from_le_bytes(e1[28..32].try_into().unwrap());
    assert_eq!(crc1, flash_fmt::crc32(&e1[..28]));
    let crc2 = u32::from_le_bytes(e2[28..32].try_into().unwrap());
    assert_eq!(crc2, flash_fmt::crc32(&e2[..28]));
}

// ===========================================================================
// Flash alignment tests (write_aligned with MockFlash)
// ===========================================================================

#[test]
fn write_aligned_exact_multiple_of_4() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    let data = vec![0x42u8; 128]; // 128 = 32 * 4, perfectly aligned
    flash_fmt::write_aligned(&mut flash, 0, &data).unwrap();
    assert_eq!(flash.read_slice(0, 128), data.as_slice());
}

#[test]
fn write_aligned_trailing_bytes() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    // 5 bytes: 4 aligned + 1 trailing
    let data = [0x01, 0x02, 0x03, 0x04, 0x05];
    flash_fmt::write_aligned(&mut flash, 0, &data).unwrap();
    // First 5 bytes match
    assert_eq!(flash.read_slice(0, 5), &data);
    // Trailing 3 bytes should be 0xFF (erased)
    assert_eq!(flash.read_slice(5, 3), &[0xFF, 0xFF, 0xFF]);
}

#[test]
fn write_aligned_1_byte() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    flash_fmt::write_aligned(&mut flash, 0, &[0xAB]).unwrap();
    assert_eq!(flash.read_slice(0, 1), &[0xAB]);
    assert_eq!(flash.read_slice(1, 3), &[0xFF, 0xFF, 0xFF]);
}

#[test]
fn write_aligned_2_bytes() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    flash_fmt::write_aligned(&mut flash, 0, &[0xAB, 0xCD]).unwrap();
    assert_eq!(flash.read_slice(0, 2), &[0xAB, 0xCD]);
}

#[test]
fn write_aligned_3_bytes() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    flash_fmt::write_aligned(&mut flash, 0, &[0x11, 0x22, 0x33]).unwrap();
    assert_eq!(flash.read_slice(0, 3), &[0x11, 0x22, 0x33]);
}

#[test]
fn write_aligned_empty() {
    let mut flash = MockFlash::new(4096);
    flash_fmt::write_aligned(&mut flash, 0, &[]).unwrap();
    assert_eq!(flash.write_count, 0);
}

#[test]
fn write_to_unerased_flash_fails() {
    let mut flash = MockFlash::new(4096);
    // Don't erase — storage is 0xFF by default, but write 0x42 first
    flash.erase(0, 4096).unwrap();
    flash_fmt::write_aligned(&mut flash, 0, &[0x42, 0x00, 0x00, 0x00]).unwrap();
    // Second write to same location should fail (already written, not 0xFF)
    let result = flash_fmt::write_aligned(&mut flash, 0, &[0x43, 0x00, 0x00, 0x00]);
    assert!(result.is_err(), "write to unerased byte should fail");
}

#[test]
fn write_at_offset() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    let data = [0xDE, 0xAD, 0xBE, 0xEF];
    flash_fmt::write_aligned(&mut flash, 100, &data).unwrap();
    assert_eq!(flash.read_slice(100, 4), &data);
    // Before and after should still be 0xFF
    assert_eq!(flash.read_slice(96, 4), &[0xFF; 4]);
    assert_eq!(flash.read_slice(104, 4), &[0xFF; 4]);
}

// ===========================================================================
// Receiver state machine tests
// ===========================================================================

#[test]
fn receiver_idle_ignores_chunk() {
    let state = OtaState::new();
    let result = state.process(OtaEvent::Chunk {
        sender: [0; 6],
        json: b"anything",
    });
    assert!(matches!(result.state, OtaState::Idle));
    assert!(result.actions.is_empty());
}

#[test]
fn receiver_manifest_starts_transfer() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);

    let state = OtaState::new();
    let result = state.process(OtaEvent::Manifest {
        sender: [0xAA; 6],
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    assert!(matches!(result.state, OtaState::Receiving { .. }));
    assert_eq!(result.actions.len(), 2);
    assert!(matches!(
        result.actions[0],
        OtaAction::ErasePartition { .. }
    ));
    assert_eq!(result.actions[1], OtaAction::RequestChunk { index: 0 });
}

#[test]
fn receiver_invalid_manifest_stays_idle() {
    let (_, pk) = gen_keypair();
    let state = OtaState::new();
    let result = state.process(OtaEvent::Manifest {
        sender: [0xAA; 6],
        json: b"not a valid manifest",
        pubkey: &pk,
    });
    assert!(matches!(result.state, OtaState::Idle));
    assert!(result.actions.is_empty());
}

#[test]
fn receiver_full_transfer_3_chunks() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300); // 3 chunks: 128+128+44
    let n = protocol::n_chunks_for_len(image.len());
    assert_eq!(n, 3);
    let (manifest, _hash) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0x01, 0x00];

    // Start with manifest
    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    assert!(matches!(t.state, OtaState::Receiving { .. }));

    // Send chunks 0, 1, 2
    let mut state = t.state;
    for i in 0..n {
        let chunk_data = protocol::image_chunk(&image, i).unwrap();
        let chunk_frame = protocol::build_chunk_response(i, chunk_data);
        let t = state.process(OtaEvent::Chunk {
            sender: sender_mac,
            json: chunk_frame.as_bytes(),
        });
        assert!(t.chunk_data.is_some(), "chunk {} should produce data", i);
        assert_eq!(t.chunk_data.as_ref().unwrap().as_slice(), chunk_data);

        if i < n - 1 {
            // Intermediate chunk: write + request next
            assert!(matches!(t.state, OtaState::Receiving { .. }));
            assert!(t.actions.contains(&OtaAction::WriteChunk {
                index: i,
                offset: i * CHUNK_SIZE as u32,
            }));
            assert!(t
                .actions
                .contains(&OtaAction::RequestChunk { index: i + 1 }));
        } else {
            // Last chunk: write + apply
            assert!(matches!(t.state, OtaState::Verified { .. }));
            assert!(t.actions.contains(&OtaAction::WriteChunk {
                index: i,
                offset: i * CHUNK_SIZE as u32,
            }));
            assert!(t.actions.contains(&OtaAction::ApplyAndReboot));
        }
        state = t.state;
    }
}

#[test]
fn receiver_wrong_sender_chunk_ignored() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];
    let wrong_mac = [0xBB; 6];

    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    // Send chunk from wrong sender
    let chunk_data = protocol::image_chunk(&image, 0).unwrap();
    let chunk_frame = protocol::build_chunk_response(0, chunk_data);
    let t = t.state.process(OtaEvent::Chunk {
        sender: wrong_mac,
        json: chunk_frame.as_bytes(),
    });
    // Should still be receiving, waiting for chunk 0
    match t.state {
        OtaState::Receiving { next_chunk, .. } => assert_eq!(next_chunk, 0),
        _ => panic!("expected Receiving state"),
    }
}

#[test]
fn receiver_out_of_order_chunk_ignored() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    // Send chunk 1 instead of chunk 0
    let chunk_data = protocol::image_chunk(&image, 1).unwrap();
    let chunk_frame = protocol::build_chunk_response(1, chunk_data);
    let t = t.state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: chunk_frame.as_bytes(),
    });
    // Should still be waiting for chunk 0
    match t.state {
        OtaState::Receiving { next_chunk, .. } => assert_eq!(next_chunk, 0),
        _ => panic!("expected Receiving state"),
    }
}

#[test]
fn receiver_tampered_chunk_fails_hash() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    // Send correct chunk 0
    let c0 = protocol::image_chunk(&image, 0).unwrap();
    let f0 = protocol::build_chunk_response(0, c0);
    let t = t.state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: f0.as_bytes(),
    });

    // Send correct chunk 1
    let c1 = protocol::image_chunk(&image, 1).unwrap();
    let f1 = protocol::build_chunk_response(1, c1);
    let t = t.state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: f1.as_bytes(),
    });

    // Send TAMPERED chunk 2 (all zeros instead of real data)
    let bad_chunk = vec![0u8; 44];
    let f2 = protocol::build_chunk_response(2, &bad_chunk);
    let t = t.state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: f2.as_bytes(),
    });

    assert!(
        matches!(
            t.state,
            OtaState::Failed {
                reason: "hash mismatch"
            }
        ),
        "tampered chunk should fail hash verification"
    );
    assert!(t.actions.contains(&OtaAction::Abort {
        reason: "hash mismatch"
    }));
}

#[test]
fn receiver_ignores_new_manifest_during_transfer() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    // Send another manifest from a different sender
    let t = t.state.process(OtaEvent::Manifest {
        sender: [0xBB; 6],
        json: manifest.as_bytes(),
        pubkey: &pk,
    });

    // Should still be receiving from original sender
    match t.state {
        OtaState::Receiving { sender, .. } => assert_eq!(sender, sender_mac),
        _ => panic!("expected Receiving state"),
    }
}

// ===========================================================================
// End-to-end simulation: multi-node OTA propagation
// ===========================================================================

/// Simulate OTA propagation from a sender to N-1 receivers.
fn run_ota_propagation(n_nodes: usize, image_size: usize, sender_idx: usize) -> Vec<bool> {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(image_size);
    let (manifest, _hash) = sign_manifest(&image, "2.0.0", &sk);

    // Each non-sender node has an OTA state and a MockFlash
    struct Node {
        state: OtaState,
        flash: MockFlash,
        completed: bool,
    }

    let mut nodes: Vec<Node> = (0..n_nodes)
        .map(|_| Node {
            state: OtaState::new(),
            flash: MockFlash::new(1024 * 1024),
            completed: false,
        })
        .collect();

    let sender_mac = [0xAA, 0xBB, 0xCC, 0xDD, sender_idx as u8, 0x00];

    // Simulate: sender broadcasts manifest, receivers request chunks
    for node_idx in 0..n_nodes {
        if node_idx == sender_idx {
            continue;
        }

        // 1. Deliver manifest
        let t = core::mem::replace(&mut nodes[node_idx].state, OtaState::Idle).process(
            OtaEvent::Manifest {
                sender: sender_mac,
                json: manifest.as_bytes(),
                pubkey: &pk,
            },
        );

        // Process erase action
        for action in &t.actions {
            if let OtaAction::ErasePartition { image_len } = action {
                nodes[node_idx].flash.erase(0, *image_len).unwrap();
            }
        }
        nodes[node_idx].state = t.state;

        // 2. Deliver chunks sequentially
        let n_chunks = protocol::n_chunks_for_len(image.len());
        for chunk_idx in 0..n_chunks {
            let chunk_data = protocol::image_chunk(&image, chunk_idx).unwrap();
            let chunk_frame = protocol::build_chunk_response(chunk_idx, chunk_data);

            let t = core::mem::replace(&mut nodes[node_idx].state, OtaState::Idle).process(
                OtaEvent::Chunk {
                    sender: sender_mac,
                    json: chunk_frame.as_bytes(),
                },
            );

            // Process write actions
            for action in &t.actions {
                if let OtaAction::WriteChunk { offset, .. } = action {
                    if let Some(data) = &t.chunk_data {
                        flash_fmt::write_aligned(&mut nodes[node_idx].flash, *offset, data)
                            .unwrap();
                    }
                }
                if matches!(action, OtaAction::ApplyAndReboot) {
                    nodes[node_idx].completed = true;
                }
            }
            nodes[node_idx].state = t.state;
        }
    }

    nodes.iter().map(|n| n.completed || n_nodes == 1).collect()
}

#[test]
fn ota_2_nodes_sender_to_receiver() {
    let results = run_ota_propagation(2, 300, 0);
    // Node 0 is sender (always "completed")
    assert!(results[1], "receiver should complete OTA");
}

#[test]
fn ota_4_nodes_all_receivers_converge() {
    let results = run_ota_propagation(4, 1000, 0);
    for (i, completed) in results.iter().enumerate() {
        if i != 0 {
            assert!(completed, "node {} should complete OTA", i);
        }
    }
}

#[test]
fn ota_large_image_stream_to_flash() {
    // 50KB image — tests streaming (not buffered in RAM)
    let results = run_ota_propagation(2, 50 * 1024, 0);
    assert!(results[1], "receiver should complete OTA for 50KB image");
}

#[test]
fn ota_exact_chunk_boundary_image() {
    // Image size is exact multiple of CHUNK_SIZE
    let results = run_ota_propagation(2, CHUNK_SIZE * 10, 0);
    assert!(results[1]);
}

#[test]
fn ota_1_byte_image() {
    let results = run_ota_propagation(2, 1, 0);
    assert!(results[1], "1-byte image should work");
}

#[test]
fn ota_flash_write_verified() {
    // Verify that the data written to MockFlash matches the original image
    let (sk, pk) = gen_keypair();
    let image = dummy_image(500);
    let (manifest, hash_hex) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let mut flash = MockFlash::new(1024 * 1024);
    let mut state = OtaState::new();

    // Deliver manifest
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    for a in &t.actions {
        if let OtaAction::ErasePartition { image_len } = a {
            flash.erase(0, *image_len).unwrap();
        }
    }
    state = t.state;

    // Deliver all chunks
    let n_chunks = protocol::n_chunks_for_len(image.len());
    for i in 0..n_chunks {
        let chunk = protocol::image_chunk(&image, i).unwrap();
        let frame = protocol::build_chunk_response(i, chunk);
        let t = state.process(OtaEvent::Chunk {
            sender: sender_mac,
            json: frame.as_bytes(),
        });
        if let Some(data) = &t.chunk_data {
            for a in &t.actions {
                if let OtaAction::WriteChunk { offset, .. } = a {
                    flash_fmt::write_aligned(&mut flash, *offset, data).unwrap();
                }
            }
        }
        state = t.state;
    }

    // Verify flash contents match original image
    let written = flash.read_slice(0, image.len());
    assert_eq!(
        written,
        image.as_slice(),
        "flash contents should match original image"
    );

    // Verify hash of written data
    assert!(
        protocol::verify_image_hash(written, &hash_hex),
        "hash of flash contents should match manifest"
    );
}

// ===========================================================================
// Edge cases and adversarial scenarios
// ===========================================================================

#[test]
fn ota_flash_fault_during_write() {
    let mut flash = MockFlash::new(4096);
    flash.erase(0, 4096).unwrap();
    flash.fault_at_write = Some(128); // Fault at offset 128

    // First write at offset 0 succeeds
    flash_fmt::write_aligned(&mut flash, 0, &[0x42; 4]).unwrap();
    // Write at offset 128 fails
    let result = flash_fmt::write_aligned(&mut flash, 128, &[0x42; 4]);
    assert!(result.is_err());
}

#[test]
fn crc32_empty_input() {
    // CRC32 of empty input is 0x00000000 (per IEEE 802.3: ~0xFFFFFFFF = 0x00000000...
    // actually CRC32("") = 0x00000000)
    assert_eq!(flash_fmt::crc32(&[]), 0x0000_0000);
}

#[test]
fn crc32_single_byte() {
    // CRC32 of single zero byte
    let crc = flash_fmt::crc32(&[0x00]);
    // Known value: CRC32(0x00) = 0xD202EF8D
    assert_eq!(crc, 0xD202_EF8D);
}

#[test]
fn pad_to_4_align_already_aligned() {
    let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let padded = flash_fmt::pad_to_4_align(&data);
    assert_eq!(padded, data);
}

#[test]
fn pad_to_4_align_needs_padding() {
    let data = vec![1, 2, 3, 4, 5];
    let padded = flash_fmt::pad_to_4_align(&data);
    assert_eq!(padded, vec![1, 2, 3, 4, 5, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn pad_to_4_align_1_byte() {
    let padded = flash_fmt::pad_to_4_align(&[0xAB]);
    assert_eq!(padded, vec![0xAB, 0xFF, 0xFF, 0xFF]);
}

// ===========================================================================
// Multi-hop OTA propagation (mesh relay)
// ===========================================================================

/// Simulate multi-hop OTA: node 0 has the image, sends to node 1. Once node 1
/// completes, it becomes a sender for node 2, and so on (chain topology).
#[test]
fn ota_chain_propagation_3_hops() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(500);
    let (manifest, hash_hex) = sign_manifest(&image, "3.0.0", &sk);
    let n_chunks = protocol::n_chunks_for_len(image.len());

    struct ChainNode {
        mac: [u8; 6],
        state: OtaState,
        flash: MockFlash,
        completed: bool,
    }

    let mut nodes: Vec<ChainNode> = (0..4)
        .map(|i| ChainNode {
            mac: [0xAA, 0xBB, 0xCC, 0xDD, i as u8, 0x00],
            state: OtaState::new(),
            flash: MockFlash::new(1024 * 1024),
            completed: i == 0, // node 0 already has the image
        })
        .collect();

    // Each node receives from the previous one
    for hop in 0..3 {
        let sender_idx = hop;
        let receiver_idx = hop + 1;
        let sender_mac = nodes[sender_idx].mac;

        // Deliver manifest
        let t = core::mem::replace(&mut nodes[receiver_idx].state, OtaState::Idle).process(
            OtaEvent::Manifest {
                sender: sender_mac,
                json: manifest.as_bytes(),
                pubkey: &pk,
            },
        );
        for a in &t.actions {
            if let OtaAction::ErasePartition { image_len } = a {
                nodes[receiver_idx].flash.erase(0, *image_len).unwrap();
            }
        }
        nodes[receiver_idx].state = t.state;

        // Deliver chunks (sender serves from the original image)
        for i in 0..n_chunks {
            let chunk = protocol::image_chunk(&image, i).unwrap();
            let frame = protocol::build_chunk_response(i, chunk);
            let t = core::mem::replace(&mut nodes[receiver_idx].state, OtaState::Idle).process(
                OtaEvent::Chunk {
                    sender: sender_mac,
                    json: frame.as_bytes(),
                },
            );
            if let Some(data) = &t.chunk_data {
                for a in &t.actions {
                    if let OtaAction::WriteChunk { offset, .. } = a {
                        flash_fmt::write_aligned(&mut nodes[receiver_idx].flash, *offset, data)
                            .unwrap();
                    }
                    if matches!(a, OtaAction::ApplyAndReboot) {
                        nodes[receiver_idx].completed = true;
                    }
                }
            }
            nodes[receiver_idx].state = t.state;
        }

        // Verify this hop completed
        assert!(
            nodes[receiver_idx].completed,
            "node {} should complete OTA from node {}",
            receiver_idx, sender_idx
        );

        // Verify flash contents match
        let written = nodes[receiver_idx].flash.read_slice(0, image.len());
        assert!(
            protocol::verify_image_hash(written, &hash_hex),
            "node {} flash should match original image hash",
            receiver_idx
        );
    }

    assert!(nodes.iter().all(|n| n.completed));
}

// ===========================================================================
// Packet loss simulation
// ===========================================================================

/// Simulate OTA with packet loss — some chunks are dropped and must be re-requested.
/// The receiver state machine ignores wrong-index chunks, so the sender retries.
#[test]
fn ota_with_packet_loss_completes() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(640); // 5 chunks
    let (manifest, hash_hex) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];
    let n_chunks = protocol::n_chunks_for_len(image.len());
    assert_eq!(n_chunks, 5);

    let mut flash = MockFlash::new(1024 * 1024);
    let mut state = OtaState::new();

    // Deliver manifest
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    for a in &t.actions {
        if let OtaAction::ErasePartition { image_len } = a {
            flash.erase(0, *image_len).unwrap();
        }
    }
    state = t.state;

    // Deliver chunks with simulated drops: send each chunk, but drop chunks 1 and 3
    // on first attempt. Retry logic: if state machine doesn't advance, re-send.
    let drop_on_first = [1u32, 3];
    let mut attempt = vec![0u32; n_chunks as usize];
    let mut current_expected = 0u32;

    for _round in 0..20 {
        // Try to send the current expected chunk
        if current_expected >= n_chunks {
            break;
        }
        attempt[current_expected as usize] += 1;

        let should_drop =
            drop_on_first.contains(&current_expected) && attempt[current_expected as usize] == 1;

        if should_drop {
            // Packet lost — receiver never sees it, stays at same state
            // In real protocol, receiver would re-request after timeout
            // Here we just try again next round
            continue;
        }

        let chunk = protocol::image_chunk(&image, current_expected).unwrap();
        let frame = protocol::build_chunk_response(current_expected, chunk);
        let t = state.process(OtaEvent::Chunk {
            sender: sender_mac,
            json: frame.as_bytes(),
        });

        if let Some(data) = &t.chunk_data {
            for a in &t.actions {
                if let OtaAction::WriteChunk { offset, .. } = a {
                    flash_fmt::write_aligned(&mut flash, *offset, data).unwrap();
                }
            }
        }

        // Check what the next expected chunk is
        match &t.state {
            OtaState::Receiving { next_chunk, .. } => {
                current_expected = *next_chunk;
            }
            OtaState::Verified { .. } => {
                // Done
                let written = flash.read_slice(0, image.len());
                assert!(protocol::verify_image_hash(written, &hash_hex));
                return;
            }
            _ => {}
        }
        state = t.state;
    }

    panic!("OTA should have completed within 20 rounds");
}

// ===========================================================================
// Duplicate delivery resilience
// ===========================================================================

#[test]
fn ota_duplicate_chunks_ignored() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(300);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let mut state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    state = t.state;

    // Send chunk 0
    let c0 = protocol::image_chunk(&image, 0).unwrap();
    let f0 = protocol::build_chunk_response(0, c0);
    let t = state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: f0.as_bytes(),
    });
    assert!(t.chunk_data.is_some());
    state = t.state;

    // Send chunk 0 again (duplicate) — should be ignored (next_chunk is now 1)
    let t = state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: f0.as_bytes(),
    });
    assert!(t.chunk_data.is_none(), "duplicate chunk should be ignored");
    // State should still expect chunk 1
    match &t.state {
        OtaState::Receiving { next_chunk, .. } => assert_eq!(*next_chunk, 1),
        _ => panic!("expected Receiving"),
    }
}

#[test]
fn ota_duplicate_manifest_during_idle_accepted() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(128);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);

    // First manifest starts transfer
    let state = OtaState::new();
    let t = state.process(OtaEvent::Manifest {
        sender: [0xAA; 6],
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    assert!(matches!(t.state, OtaState::Receiving { .. }));
}

// ===========================================================================
// Concurrent senders
// ===========================================================================

#[test]
fn ota_second_sender_ignored_during_transfer() {
    let (sk1, pk1) = gen_keypair();
    let (sk2, pk2) = gen_keypair();
    let image1 = dummy_image(300);
    let image2 = dummy_image(400);
    let (manifest1, _) = sign_manifest(&image1, "1.0.0", &sk1);
    let (manifest2, _) = sign_manifest(&image2, "2.0.0", &sk2);
    let sender1 = [0xAA; 6];
    let sender2 = [0xBB; 6];

    let state = OtaState::new();

    // First sender starts
    let t = state.process(OtaEvent::Manifest {
        sender: sender1,
        json: manifest1.as_bytes(),
        pubkey: &pk1,
    });
    let state = t.state;

    // Second sender tries (should be ignored)
    let t = state.process(OtaEvent::Manifest {
        sender: sender2,
        json: manifest2.as_bytes(),
        pubkey: &pk2,
    });

    // Should still be receiving from sender1
    match &t.state {
        OtaState::Receiving {
            sender, version, ..
        } => {
            assert_eq!(*sender, sender1);
            assert_eq!(version, "1.0.0");
        }
        _ => panic!("expected Receiving from sender1"),
    }
}

// ===========================================================================
// Image size edge cases
// ===========================================================================

#[test]
fn ota_exact_one_chunk_image() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(CHUNK_SIZE); // exactly 1 chunk
    let (manifest, hash_hex) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];

    let mut flash = MockFlash::new(1024 * 1024);
    let state = OtaState::new();

    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    for a in &t.actions {
        if let OtaAction::ErasePartition { image_len } = a {
            flash.erase(0, *image_len).unwrap();
        }
    }

    let chunk = protocol::image_chunk(&image, 0).unwrap();
    let frame = protocol::build_chunk_response(0, chunk);
    let t = t.state.process(OtaEvent::Chunk {
        sender: sender_mac,
        json: frame.as_bytes(),
    });

    // Single chunk → immediately verified
    assert!(matches!(t.state, OtaState::Verified { .. }));
    assert!(t.actions.contains(&OtaAction::ApplyAndReboot));

    // Write and verify
    if let Some(data) = &t.chunk_data {
        flash_fmt::write_aligned(&mut flash, 0, data).unwrap();
    }
    let written = flash.read_slice(0, image.len());
    assert!(protocol::verify_image_hash(written, &hash_hex));
}

#[test]
fn ota_chunk_size_plus_one_image() {
    // CHUNK_SIZE + 1 → 2 chunks, last is 1 byte
    let results = run_ota_propagation(2, CHUNK_SIZE + 1, 0);
    assert!(results[1]);
}

#[test]
fn ota_127_byte_image() {
    // CHUNK_SIZE - 1 → 1 chunk of 127 bytes
    let results = run_ota_propagation(2, CHUNK_SIZE - 1, 0);
    assert!(results[1]);
}

// ===========================================================================
// Flash integrity: verify written data byte-by-byte across chunk boundaries
// ===========================================================================

#[test]
fn ota_flash_byte_perfect_across_boundaries() {
    let (sk, pk) = gen_keypair();
    // 5 full chunks + 17 byte tail (non-4-aligned)
    let image = dummy_image(CHUNK_SIZE * 5 + 17);
    let (manifest, _) = sign_manifest(&image, "1.0.0", &sk);
    let sender_mac = [0xAA; 6];
    let n_chunks = protocol::n_chunks_for_len(image.len());

    let mut flash = MockFlash::new(1024 * 1024);
    let mut state = OtaState::new();

    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    for a in &t.actions {
        if let OtaAction::ErasePartition { image_len } = a {
            flash.erase(0, *image_len).unwrap();
        }
    }
    state = t.state;

    for i in 0..n_chunks {
        let chunk = protocol::image_chunk(&image, i).unwrap();
        let frame = protocol::build_chunk_response(i, chunk);
        let t = state.process(OtaEvent::Chunk {
            sender: sender_mac,
            json: frame.as_bytes(),
        });
        if let Some(data) = &t.chunk_data {
            for a in &t.actions {
                if let OtaAction::WriteChunk { offset, .. } = a {
                    flash_fmt::write_aligned(&mut flash, *offset, data).unwrap();
                }
            }
        }
        state = t.state;
    }

    // Byte-by-byte comparison
    for (i, &expected) in image.iter().enumerate() {
        let actual = flash.storage[i];
        assert_eq!(
            actual, expected,
            "mismatch at byte {}: expected 0x{:02X}, got 0x{:02X}",
            i, expected, actual
        );
    }

    // Bytes after image should be 0xFF (erased)
    for i in image.len()..image.len() + 16 {
        assert_eq!(
            flash.storage[i], 0xFF,
            "byte {} after image should be 0xFF (erased), got 0x{:02X}",
            i, flash.storage[i]
        );
    }
}

// ===========================================================================
// Otadata write simulation
// ===========================================================================

#[test]
fn otadata_write_to_mock_flash() {
    let mut flash = MockFlash::new(8192); // 2 sectors for otadata
    flash.erase(0, 4096).unwrap(); // erase first sector

    let entry = flash_fmt::build_otadata_entry();
    flash_fmt::write_aligned(&mut flash, 0, &entry).unwrap();

    // Read back and verify
    let mut readback = [0u8; 32];
    flash.read(0, &mut readback).unwrap();
    assert_eq!(readback, entry);

    // Verify CRC is valid
    let crc = u32::from_le_bytes(readback[28..32].try_into().unwrap());
    assert_eq!(crc, flash_fmt::crc32(&readback[..28]));
}

#[test]
fn otadata_seq2_overwrites_seq1() {
    let mut flash = MockFlash::new(8192);

    // Write seq=1 to first sector
    flash.erase(0, 4096).unwrap();
    let entry1 = flash_fmt::build_otadata_entry_with_seq(1, 0);
    flash_fmt::write_aligned(&mut flash, 0, &entry1).unwrap();

    // Write seq=2 to second sector
    flash.erase(4096, 4096).unwrap();
    let entry2 = flash_fmt::build_otadata_entry_with_seq(2, 0);
    flash_fmt::write_aligned(&mut flash, 4096, &entry2).unwrap();

    // Bootloader picks higher seq — verify both are valid
    let mut r1 = [0u8; 32];
    let mut r2 = [0u8; 32];
    flash.read(0, &mut r1).unwrap();
    flash.read(4096, &mut r2).unwrap();

    let seq1 = u32::from_le_bytes(r1[0..4].try_into().unwrap());
    let seq2 = u32::from_le_bytes(r2[0..4].try_into().unwrap());
    assert_eq!(seq1, 1);
    assert_eq!(seq2, 2);

    // Both CRCs valid
    assert_eq!(
        u32::from_le_bytes(r1[28..32].try_into().unwrap()),
        flash_fmt::crc32(&r1[..28])
    );
    assert_eq!(
        u32::from_le_bytes(r2[28..32].try_into().unwrap()),
        flash_fmt::crc32(&r2[..28])
    );
}

// ===========================================================================
// Full round-trip: sign → manifest → chunks → flash → verify → otadata
// ===========================================================================

#[test]
fn ota_full_lifecycle_roundtrip() {
    let (sk, pk) = gen_keypair();
    let image = dummy_image(1500); // ~12 chunks, last short
    let (manifest, hash_hex) = sign_manifest(&image, "4.2.0", &sk);
    let n_chunks = protocol::n_chunks_for_len(image.len());
    let sender_mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];

    // 1MB flash for OTA partition + 8KB for otadata
    let mut ota_flash = MockFlash::new(1024 * 1024);
    let mut otadata_flash = MockFlash::new(8192);

    let mut state = OtaState::new();

    // Step 1: Manifest
    let t = state.process(OtaEvent::Manifest {
        sender: sender_mac,
        json: manifest.as_bytes(),
        pubkey: &pk,
    });
    assert!(matches!(t.state, OtaState::Receiving { .. }));

    // Step 2: Erase
    for a in &t.actions {
        if let OtaAction::ErasePartition { image_len } = a {
            ota_flash.erase(0, *image_len).unwrap();
        }
    }
    state = t.state;

    // Step 3: Stream chunks
    let mut got_apply = false;
    for i in 0..n_chunks {
        let chunk = protocol::image_chunk(&image, i).unwrap();
        let frame = protocol::build_chunk_response(i, chunk);
        let t = state.process(OtaEvent::Chunk {
            sender: sender_mac,
            json: frame.as_bytes(),
        });
        if let Some(data) = &t.chunk_data {
            for a in &t.actions {
                if let OtaAction::WriteChunk { offset, .. } = a {
                    flash_fmt::write_aligned(&mut ota_flash, *offset, data).unwrap();
                }
                if matches!(a, OtaAction::ApplyAndReboot) {
                    got_apply = true;
                }
            }
        }
        state = t.state;
    }

    assert!(got_apply, "should get ApplyAndReboot action");
    assert!(matches!(state, OtaState::Verified { .. }));

    // Step 4: Verify flash
    let written = ota_flash.read_slice(0, image.len());
    assert!(protocol::verify_image_hash(written, &hash_hex));

    // Step 5: Write otadata
    otadata_flash.erase(0, 4096).unwrap();
    let entry = flash_fmt::build_otadata_entry();
    flash_fmt::write_aligned(&mut otadata_flash, 0, &entry).unwrap();

    // Erase second sector
    otadata_flash.erase(4096, 4096).unwrap();

    // Verify otadata
    let mut readback = [0u8; 32];
    otadata_flash.read(0, &mut readback).unwrap();
    assert_eq!(u32::from_le_bytes(readback[0..4].try_into().unwrap()), 1); // seq=1 → ota_0
    let crc = u32::from_le_bytes(readback[28..32].try_into().unwrap());
    assert_eq!(crc, flash_fmt::crc32(&readback[..28]));

    // Second sector should be erased (all 0xFF)
    let mut sector2 = [0u8; 32];
    otadata_flash.read(4096, &mut sector2).unwrap();
    assert!(sector2.iter().all(|&b| b == 0xFF));
}
