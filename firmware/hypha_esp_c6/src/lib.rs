#![no_std]

extern crate alloc;

// Re-export everything from hypha-firefly so firmware code continues to use
// `hypha_esp_c6::NodeState`, `hypha_esp_c6::FireflyOscillator`, etc.
pub use hypha_firefly::*;

// ---------------------------------------------------------------------------
// Firmware-specific modules (feature-gated, not in the shared crate)
// ---------------------------------------------------------------------------

#[cfg(feature = "mesh_ota")]
mod mesh_ota_pubkey {
    include!(concat!(env!("OUT_DIR"), "/mesh_ota_pubkey.rs"));
}
#[cfg(feature = "mesh_ota")]
mod mesh_ota_manifest {
    include!(concat!(env!("OUT_DIR"), "/mesh_ota_manifest.rs"));
}
#[cfg(feature = "mesh_ota")]
mod mesh_ota_image {
    include!(concat!(env!("OUT_DIR"), "/mesh_ota_image.rs"));
}
#[cfg(feature = "mesh_ota")]
pub mod mesh_ota;
#[cfg(feature = "mesh_ota")]
pub mod ota_apply;
