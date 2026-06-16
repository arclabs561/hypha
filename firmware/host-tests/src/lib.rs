//! Host-runnable unit tests for the firmware's pure logic.
//!
//! These modules from `hypha_esp_c6_idf` have no ESP-IDF dependencies, so they
//! compile and run on the dev host through a `#[path]` include — the firmware
//! crate's riscv32 target pin does not reach this sibling. See Cargo.toml for
//! why this lives in a separate crate. Add a module here whenever a pure piece
//! of firmware logic gains behaviour worth pinning; the led/mqtt/main pure
//! helpers (HSV, dither, `json_field`, `parse_semver`) are still entangled with
//! esp-dep imports in their files and need the `hypha-core` extraction first.

#[cfg(test)]
#[path = "../../hypha_esp_c6_idf/src/firefly.rs"]
mod firefly;

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
        assert!(!f.advance(PERIOD * 0.95), "0.95 of a period must not fire yet");
        assert!(f.couple(), "a peer pulse at phase 0.95 should push us over");
    }

    /// Firing enters a refractory window; a peer pulse inside it is ignored
    /// (anti-lockup, so a burst of peers can't ratchet a just-fired node).
    #[test]
    fn refractory_blocks_coupling_right_after_fire() {
        let mut f = Firefly::new(PERIOD);
        assert!(f.advance(PERIOD), "should fire at the period boundary");
        assert!(!f.couple(), "coupling inside the refractory window must be ignored");
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
