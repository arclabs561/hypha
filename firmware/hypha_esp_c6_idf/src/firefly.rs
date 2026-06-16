//! Mirollo-Strogatz pulse-coupled oscillator for cross-board flash sync.
//!
//! Each board runs one oscillator whose phase advances 0->1 over `period`;
//! at 1.0 it FIRES (heartbeat flash) and resets. On firing a board publishes
//! a pulse to a shared MQTT topic; on hearing a peer's pulse it advances its
//! own phase along a concave-down curve (the Mirollo-Strogatz coupling that
//! provably drives identical oscillators into lockstep). The result: the
//! boards' heartbeats converge to a synchronized flash across rooms — the
//! hypha mesh thesis made visible as light, and a liveness signal (in sync =
//! the coupling channel is healthy). Mirrors the hypha-firefly crate's model;
//! this is the std-firmware port. If MQTT is down the oscillator free-runs
//! (no coupling), still flashing locally, and re-couples when the bus returns.

/// Coupling strength: how hard a peer pulse advances our phase. ~0.1-0.2
/// synchronizes a small fleet within a handful of cycles without overshoot.
pub const EPSILON: f32 = 0.15;

pub struct Firefly {
    phase: f32,      // 0..1
    period: f32,     // seconds per cycle
    refractory: f32, // seconds; ignore coupling right after a fire (anti-lockup)
}

impl Firefly {
    pub fn new(period: f32) -> Self {
        Self {
            phase: 0.0,
            period,
            refractory: 0.0,
        }
    }

    /// Advance by `dt` seconds; returns true if it fired this step.
    pub fn advance(&mut self, dt: f32) -> bool {
        if self.refractory > 0.0 {
            self.refractory -= dt;
        }
        self.phase += dt / self.period;
        if self.phase >= 1.0 {
            self.fire();
            true
        } else {
            false
        }
    }

    /// Apply a peer pulse (Mirollo-Strogatz coupling). Returns true if the
    /// nudge pushed us over threshold (we fire in sympathy — the cascade that
    /// produces synchrony). Ignored during the refractory window.
    pub fn couple(&mut self) -> bool {
        if self.refractory > 0.0 {
            return false;
        }
        // concave-down state function f(phi)=2phi-phi^2, inverse g(x)=1-sqrt(1-x)
        let x = (2.0 * self.phase - self.phase * self.phase + EPSILON).min(1.0);
        if x >= 1.0 {
            self.fire();
            true
        } else {
            self.phase = 1.0 - (1.0 - x).sqrt();
            false
        }
    }

    fn fire(&mut self) {
        self.phase = 0.0;
        self.refractory = self.period * 0.15;
    }
}
