//! OTA protocol: manifest verification, chunk framing, image hash.
//!
//! All functions are pure (no hardware deps) and work on both host and device.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::Digest;

/// Chunk size for OTA transfers. Must produce ESP-NOW frames < 250 bytes after
/// base64 encoding + JSON framing: base64(128) = 172 chars, JSON overhead ~32
/// chars, total ~204 bytes — safely under the 250-byte ESP-NOW limit.
pub const CHUNK_SIZE: usize = 128;

/// Max chunks supported (OTA partition = 1MB).
pub const MAX_CHUNKS: u32 = (1024 * 1024 / CHUNK_SIZE) as u32;

// ---------------------------------------------------------------------------
// Manifest: sign/verify
// ---------------------------------------------------------------------------

/// Build the signing payload from manifest fields: len-prefixed version + hash + n_chunks.
pub fn build_signing_payload(version: &str, hash: &[u8], n_chunks: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    let vbytes = version.as_bytes();
    payload.extend_from_slice(&(vbytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(vbytes);
    payload.extend_from_slice(hash);
    payload.extend_from_slice(&n_chunks.to_be_bytes());
    payload
}

/// Verify a manifest's Ed25519 signature against the given public key.
fn verify_manifest(
    version: &str,
    hash_hex: &str,
    n_chunks: u32,
    sig_b64: &str,
    pubkey: &[u8; 32],
) -> bool {
    let Ok(hash) = hex::decode(hash_hex) else {
        return false;
    };
    if hash.len() != 32 {
        return false;
    }
    let Ok(sig_bytes) = B64.decode(sig_b64) else {
        return false;
    };
    if sig_bytes.len() != 64 {
        return false;
    }
    let sig = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let verifying_key = match VerifyingKey::from_bytes(pubkey) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let payload = build_signing_payload(version, &hash, n_chunks);
    verifying_key.verify(&payload, &sig).is_ok()
}

/// Parse JSON manifest and verify with the given public key.
/// Returns `Some((version, n_chunks))` if valid.
pub fn verify_manifest_json(
    json_bytes: &[u8],
    pubkey: &[u8; 32],
) -> Option<(String, u32)> {
    let text = core::str::from_utf8(json_bytes).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("ota")?.as_str()? != "manifest" {
        return None;
    }
    let version = v.get("v")?.as_str()?.to_string();
    let hash_hex = v.get("h")?.as_str()?;
    let n = v.get("n")?.as_u64()? as u32;
    let sig_b64 = v.get("sig")?.as_str()?;
    if verify_manifest(&version, hash_hex, n, sig_b64, pubkey) {
        Some((version, n))
    } else {
        None
    }
}

/// Parse JSON manifest and return all fields (version, n_chunks, hash_hex, sig_b64).
pub fn verify_manifest_json_full(
    json_bytes: &[u8],
    pubkey: &[u8; 32],
) -> Option<(String, u32, String, String)> {
    let text = core::str::from_utf8(json_bytes).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("ota")?.as_str()? != "manifest" {
        return None;
    }
    let version: String = v.get("v")?.as_str()?.to_string();
    let hash_hex: String = v.get("h")?.as_str()?.to_string();
    let n = v.get("n")?.as_u64()? as u32;
    let sig_b64: String = v.get("sig")?.as_str()?.to_string();
    if verify_manifest(&version, &hash_hex, n, &sig_b64, pubkey) {
        Some((version, n, hash_hex, sig_b64))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Chunk framing: build / parse request and response JSON
// ---------------------------------------------------------------------------

/// Build chunk request JSON: `{"ota":"req","i":N}`.
pub fn build_chunk_request(index: u32) -> String {
    alloc::format!(r#"{{"ota":"req","i":{}}}"#, index)
}

/// Parse chunk request JSON. Returns `Some(chunk_index)` if valid.
pub fn parse_chunk_request(data: &[u8]) -> Option<u32> {
    let text = core::str::from_utf8(data).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("ota")?.as_str()? != "req" {
        return None;
    }
    Some(v.get("i")?.as_u64()? as u32)
}

/// Build chunk response JSON: `{"ota":"chunk","i":N,"b":"<base64>"}`.
pub fn build_chunk_response(index: u32, chunk_data: &[u8]) -> String {
    let b64 = B64.encode(chunk_data);
    alloc::format!(r#"{{"ota":"chunk","i":{},"b":"{}"}}"#, index, b64)
}

/// Parse chunk response JSON. Returns `Some((index, chunk_bytes))` if valid.
pub fn parse_chunk_response(data: &[u8]) -> Option<(u32, Vec<u8>)> {
    let text = core::str::from_utf8(data).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("ota")?.as_str()? != "chunk" {
        return None;
    }
    let i = v.get("i")?.as_u64()? as u32;
    let b64 = v.get("b")?.as_str()?;
    let chunk = B64.decode(b64).ok()?;
    Some((i, chunk))
}

// ---------------------------------------------------------------------------
// Manifest JSON construction (for sender / test harness)
// ---------------------------------------------------------------------------

/// Build manifest JSON string from fields.
pub fn build_manifest_json(
    version: &str,
    hash_hex: &str,
    n_chunks: u32,
    sig_b64: &str,
) -> String {
    alloc::format!(
        r#"{{"ota":"manifest","v":"{}","h":"{}","n":{},"sig":"{}"}}"#,
        version, hash_hex, n_chunks, sig_b64
    )
}

// ---------------------------------------------------------------------------
// Image hash verification
// ---------------------------------------------------------------------------

/// Verify that assembled image bytes match the expected SHA256 hash.
pub fn verify_image_hash(image: &[u8], hash_hex: &str) -> bool {
    let expected = match hex::decode(hash_hex) {
        Ok(v) if v.len() == 32 => v,
        _ => return false,
    };
    sha2::Sha256::digest(image).as_slice() == expected.as_slice()
}

/// Compute SHA256 hex digest of data.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(data))
}

/// Read a chunk from an image buffer by index. Returns `None` if out of range.
pub fn image_chunk(image: &[u8], index: u32) -> Option<&[u8]> {
    let start = index as usize * CHUNK_SIZE;
    if start >= image.len() {
        return None;
    }
    let end = core::cmp::min(start + CHUNK_SIZE, image.len());
    Some(&image[start..end])
}

/// Compute the number of chunks needed for an image of the given length.
pub fn n_chunks_for_len(image_len: usize) -> u32 {
    ((image_len + CHUNK_SIZE - 1) / CHUNK_SIZE) as u32
}
