//! Mesh OTA: thin firmware shim over `hypha_ota::protocol`.
//!
//! Adds embedded-key wrappers and access to the build-time embedded image.

#![cfg(feature = "mesh_ota")]

// Re-export protocol constants and functions so existing call sites don't change.
pub use hypha_ota::protocol::{
    CHUNK_SIZE, MAX_CHUNKS,
    build_chunk_request, build_chunk_response,
    parse_chunk_request, parse_chunk_response,
    verify_image_hash,
};

// ---------------------------------------------------------------------------
// Embedded-key wrappers (delegate to hypha_ota with the build-time pubkey)
// ---------------------------------------------------------------------------

/// Verify manifest JSON using the embedded public key.
pub fn verify_manifest_json_embedded(
    json_bytes: &[u8],
) -> Option<(alloc::string::String, u32)> {
    hypha_ota::protocol::verify_manifest_json(
        json_bytes,
        &crate::mesh_ota_pubkey::MESH_OTA_PUBKEY,
    )
}

/// Verify manifest JSON and return all fields, using the embedded public key.
pub fn verify_manifest_json_embedded_full(
    json_bytes: &[u8],
) -> Option<(alloc::string::String, u32, alloc::string::String, alloc::string::String)> {
    hypha_ota::protocol::verify_manifest_json_full(
        json_bytes,
        &crate::mesh_ota_pubkey::MESH_OTA_PUBKEY,
    )
}

// ---------------------------------------------------------------------------
// Embedded image access
// ---------------------------------------------------------------------------

/// True if this build has an embedded manifest + image and can act as OTA sender.
pub fn has_embedded_manifest() -> bool {
    crate::mesh_ota_manifest::MESH_OTA_N_CHUNKS > 0
        && crate::mesh_ota_manifest::MESH_OTA_HAS_IMAGE
}

/// Embedded manifest chunk count (0 if no manifest).
pub fn embedded_n_chunks() -> u32 {
    crate::mesh_ota_manifest::MESH_OTA_N_CHUNKS
}

/// Total embedded image length in bytes.
pub fn embedded_image_len() -> usize {
    crate::mesh_ota_image::MESH_OTA_IMAGE_LEN
}

/// Read a chunk from the embedded firmware image by index.
pub fn embedded_image_chunk(index: u32) -> Option<&'static [u8]> {
    hypha_ota::protocol::image_chunk(
        crate::mesh_ota_image::MESH_OTA_IMAGE,
        index,
    )
}

/// Build manifest JSON string for broadcast from embedded constants.
pub fn manifest_json_for_broadcast() -> Option<alloc::string::String> {
    if crate::mesh_ota_manifest::MESH_OTA_N_CHUNKS == 0 {
        return None;
    }
    Some(hypha_ota::protocol::build_manifest_json(
        crate::mesh_ota_manifest::MESH_OTA_VERSION,
        crate::mesh_ota_manifest::MESH_OTA_HASH_HEX,
        crate::mesh_ota_manifest::MESH_OTA_N_CHUNKS,
        crate::mesh_ota_manifest::MESH_OTA_SIG_B64,
    ))
}
