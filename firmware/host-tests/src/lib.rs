//! Host-runnable unit tests for the firmware's pure logic.
//!
//! These modules from `hypha_esp_c6_idf` have no ESP-IDF dependencies, so they
//! compile and run on the dev host through a `#[path]` include — the firmware
//! crate's riscv32 target pin does not reach this sibling. See Cargo.toml for
//! why this lives in a separate crate. Add a module here whenever a pure piece
//! of firmware logic gains behaviour worth pinning; the led/mqtt/main pure
//! helpers (HSV, dither, `json_field`) are still entangled with esp-dep imports
//! in their files and need the `hypha-core` extraction first.

// Boot-WiFi-delta self-flag: host-test the pure firmware module while the
// device crate itself is pinned to the ESP target.
#[path = "../../hypha_esp_c6_idf/src/placement.rs"]
pub mod placement;

// Scan-window desynchronization (DESYNC) prototype: the dual of the firefly's
// sync coupling, for spreading scan windows. Same lift-at-0.17.0 status.
pub mod desync;

#[cfg(test)]
#[path = "../../hypha_esp_c6_idf/src/ota_security.rs"]
mod ota_security;

#[cfg(test)]
#[path = "../../hypha_esp_c6_idf/src/firefly.rs"]
mod firefly;

#[cfg(test)]
mod ota_security_tests {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use hypha_ota::protocol;

    use super::ota_security;

    fn signed_manifest(image: &[u8], version: &str, key: &SigningKey) -> String {
        let hash_hex = protocol::sha256_hex(image);
        let hash = hex_to_bytes_32(&hash_hex);
        let n_chunks = protocol::n_chunks_for_len(image.len());
        let payload = protocol::build_signing_payload(version, &hash, n_chunks);
        let sig = key.sign(&payload);
        protocol::build_manifest_json(version, &hash_hex, n_chunks, &B64.encode(sig.to_bytes()))
    }

    fn hex_to_bytes_32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    fn bytes_to_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }

    #[test]
    fn manifest_url_sits_next_to_image() {
        assert_eq!(
            ota_security::manifest_url_for("http://host/firmware.bin"),
            "http://host/firmware.bin.manifest.json"
        );
    }

    #[test]
    fn version_gate_requires_strict_plain_semver_increase() {
        assert!(ota_security::is_strictly_newer("0.16.1", "0.16.0"));
        assert!(!ota_security::is_strictly_newer("0.16.0", "0.16.0"));
        assert!(!ota_security::is_strictly_newer("0.15.9", "0.16.0"));
        assert!(!ota_security::is_strictly_newer("0.17.0-rc1", "0.16.0"));
    }

    #[test]
    fn ota_state_names_are_stable_for_health() {
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_DISABLED),
            "disabled"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_NO_MANIFEST),
            "no_manifest"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_BAD_MANIFEST),
            "bad_manifest"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_BAD_KEY),
            "bad_key"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_FETCH_ERROR),
            "fetch_error"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_HASH_MISMATCH),
            "hash_mismatch"
        );
        assert_eq!(
            ota_security::ota_state_name(ota_security::OTA_APPLY_ERROR),
            "apply_error"
        );
        assert_eq!(ota_security::ota_state_name(255), "unknown");
    }

    #[test]
    fn pubkey_hex_must_decode_to_32_bytes() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pubkey_hex = bytes_to_hex(&key.verifying_key().to_bytes());
        assert_eq!(
            ota_security::decode_pubkey_hex(&pubkey_hex).unwrap(),
            key.verifying_key().to_bytes()
        );
        assert!(ota_security::decode_pubkey_hex("abcd").is_none());
        assert!(ota_security::decode_pubkey_hex("not hex").is_none());
    }

    #[test]
    fn signed_manifest_verifies_and_tampering_fails() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let image = b"test firmware image";
        let manifest = signed_manifest(image, "0.16.1", &key);
        let pubkey = key.verifying_key().to_bytes();

        let verified = ota_security::verify_signed_manifest(manifest.as_bytes(), &pubkey)
            .expect("signed manifest should verify");
        assert_eq!(verified.version, "0.16.1");
        assert_eq!(verified.n_chunks, protocol::n_chunks_for_len(image.len()));
        assert_eq!(verified.hash_hex, protocol::sha256_hex(image));

        let tampered = manifest.replace("0.16.1", "9.9.9");
        assert!(
            ota_security::verify_signed_manifest(tampered.as_bytes(), &pubkey).is_none(),
            "changing the signed version must invalidate the manifest"
        );
    }
}

#[cfg(test)]
mod firefly_tests {
    use super::firefly::Firefly;

    const PERIOD: f32 = 2.0;

    /// A free-running oscillator fires exactly when accumulated phase reaches a
    /// full period — the heartbeat clock the LED + pulse publish ride on.
    #[test]
    fn free_run_fires_after_one_period() {
        let mut f = Firefly::new(PERIOD);
        assert!(!f.advance(PERIOD * 0.5), "half a period must not fire");
        assert!(f.advance(PERIOD * 0.5), "completing the period must fire");
    }

    /// A peer pulse near the top of the cycle pushes phase over threshold and
    /// triggers a sympathetic fire — the cascade that produces synchrony.
    #[test]
    fn couple_near_threshold_triggers_fire() {
        let mut f = Firefly::new(PERIOD);
        assert!(
            !f.advance(PERIOD * 0.95),
            "0.95 of a period must not fire yet"
        );
        assert!(f.couple(), "a peer pulse at phase 0.95 should push us over");
    }

    /// Firing enters a refractory window; a peer pulse inside it is ignored
    /// (anti-lockup, so a burst of peers can't ratchet a just-fired node).
    #[test]
    fn refractory_blocks_coupling_right_after_fire() {
        let mut f = Firefly::new(PERIOD);
        assert!(f.advance(PERIOD), "should fire at the period boundary");
        assert!(
            !f.couple(),
            "coupling inside the refractory window must be ignored"
        );
    }

    /// Mirollo-Strogatz convergence (the firefly thesis): two oscillators
    /// started half a period apart, mutually coupling on each other's fire,
    /// converge toward a common phase. Black-box on fire TIMES (phase is
    /// private), so this also pins that coupling is excitatory/synchronising —
    /// the property a desync variant would deliberately invert.
    #[test]
    fn two_nodes_converge_to_sync() {
        let mut a = Firefly::new(PERIOD);
        let mut b = Firefly::new(PERIOD);
        b.advance(PERIOD * 0.5); // start b half a period ahead (max offset)

        let dt = 0.02_f32;
        let steps = (90.0 / dt) as i32;
        let (mut a_last, mut b_last) = (0.0_f32, 0.0_f32);
        let mut t = 0.0_f32;
        let mut early_offset = None;
        for i in 0..steps {
            t += dt;
            let af = a.advance(dt);
            let bf = b.advance(dt);
            // Apply each fired node's pulse to its peer (instantaneous coupling).
            if af {
                a_last = t;
                b.couple();
            }
            if bf {
                b_last = t;
                a.couple();
            }
            if i == (5.0 / dt) as i32 {
                early_offset = Some(circular_offset(a_last, b_last, PERIOD));
            }
        }
        let final_offset = circular_offset(a_last, b_last, PERIOD);
        assert!(
            final_offset < 0.2,
            "expected near-sync, got circular offset {final_offset} (early {early_offset:?})"
        );
    }

    /// Distance between two fire times on the cyclic period (0..period/2).
    fn circular_offset(x: f32, y: f32, period: f32) -> f32 {
        let d = (x - y).abs() % period;
        d.min(period - d)
    }
}
