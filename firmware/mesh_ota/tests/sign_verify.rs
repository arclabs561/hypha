//! Host test: sign a firmware image then verify the manifest with the pubkey.

use std::process::Command;

#[test]
fn sign_then_verify_succeeds() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("firmware.bin");
    let key_path = tmp.path().join("key.hex");
    let out_dir = tmp.path().join("out");

    // Small fake firmware (1 KB)
    std::fs::write(&bin_path, vec![0u8; 1024]).expect("write bin");

    // 32-byte Ed25519 seed as hex (deterministic test key)
    let seed_hex = "0".repeat(64);
    std::fs::write(&key_path, seed_hex).expect("write key");

    let sign_bin = std::env::var("CARGO_BIN_EXE_mesh_ota_sign").unwrap_or_else(|_| {
        let target = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        format!("{}/target/{}/mesh_ota_sign", manifest_dir, target)
    });
    let status = Command::new(&sign_bin)
        .args([
            "--bin",
            bin_path.to_str().unwrap(),
            "--version",
            "test-1.0",
            "--key",
            key_path.to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .status()
        .expect("run sign");
    assert!(status.success(), "sign must succeed");

    let manifest = out_dir.join("manifest.json");
    let pubkey = out_dir.join("pubkey.hex");
    assert!(manifest.exists(), "manifest.json must exist");
    assert!(pubkey.exists(), "pubkey.hex must exist");

    let status = Command::new(&sign_bin)
        .args([
            "--verify",
            "--manifest",
            manifest.to_str().unwrap(),
            "--pubkey",
            pubkey.to_str().unwrap(),
        ])
        .status()
        .expect("run verify");
    assert!(status.success(), "verify must succeed");
}
