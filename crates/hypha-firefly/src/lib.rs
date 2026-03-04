//! Pure `no_std` logic for Hypha firefly synchronization, LED state machine,
//! and mesh coherence metrics.
//!
//! This crate contains zero hardware dependencies. It compiles on any target
//! (RISC-V firmware, x86_64 host tests, WASM). The firmware binary
//! (`hypha_esp_c6`) depends on this crate for its LED/oscillator logic, and
//! host-side tests (`validate_esp_logic`, `mesh_sim`) import it directly —
//! eliminating the need to duplicate functions across targets.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// LED state machine — pure functions
// ---------------------------------------------------------------------------

/// All node state needed to compute the LED color.
#[derive(Debug, Clone)]
pub struct NodeState {
    pub peer_count: usize,
    pub energy_score: f32,
    /// Smoothed energy delta over ~10s.  Positive = charging/stable, negative = draining.
    pub energy_delta: f32,
    pub tx_ok: u32,
    pub tx_err: u32,
    /// Last RSSI in dBm (-128 = unknown).
    pub last_rssi: i16,
    /// Milliseconds since last RX from *any* peer.
    pub ms_since_last_rx: u64,
    /// Activity rate in [0.0, 1.0] -- recent (TX+RX) / 20.
    pub activity_rate: f32,
    /// Milliseconds since boot.
    pub uptime_ms: u64,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            peer_count: 0,
            energy_score: 0.5,
            energy_delta: 0.0,
            tx_ok: 0,
            tx_err: 0,
            last_rssi: -128,
            ms_since_last_rx: 0,
            activity_rate: 0.0,
            uptime_ms: 10_000,
        }
    }
}

/// Computed LED colour (before temporal overlays).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedOutput {
    pub hue: u8,
    pub sat: u8,
    pub val: u8,
}

// --- constants (public so tests can reference them) ---

pub const HUE_ISOLATED: u8 = 10;
pub const HUE_SEARCHING: u8 = 25;
pub const HUE_ONE_PEER: u8 = 85;
pub const HUE_TWO_PEERS: u8 = 128;
pub const HUE_THREE_PLUS: u8 = 170;

pub const RSSI_STRONG: i16 = -35;
pub const RSSI_WEAK: i16 = -80;

pub const BRIGHTNESS_MIN: u8 = 30;
pub const BRIGHTNESS_MAX: u8 = 140;
pub const BREATHING_FLOOR: u8 = 25;

pub const BREATH_PERIOD_MIN_MS: u64 = 1_500;
pub const BREATH_PERIOD_MAX_MS: u64 = 4_000;

pub const PEER_TIMEOUT_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// Firefly oscillator — Mirollo-Strogatz integrate-and-fire model
// ---------------------------------------------------------------------------

/// Mirollo-Strogatz-inspired integrate-and-fire oscillator for firefly sync.
///
/// Phase phi in [0, 1) advances linearly at rate 1/period. At phi >= 1.0 the
/// oscillator "fires" (peak brightness) and resets to 0. On receiving a peer's
/// pulse, the internal state x = f(phi) is advanced by epsilon, where f is
/// concave-down -- this amplifies coupling near threshold (the key property
/// that guarantees synchronization for identical oscillators).
///
/// State function: f(phi) = 2*phi - phi*phi  (concave-down parabola, no libm)
/// Inverse:        g(x)   = 1 - sqrt(1 - x)
///
/// Brightness follows the concave-up curve phi^2, creating the characteristic
/// firefly pattern: slow build-up -> accelerating glow -> flash -> dark -> repeat.
#[derive(Debug, Clone)]
pub struct FireflyOscillator {
    phase: f32,
    epsilon: f32,
    refractory_end: f32,
    just_fired: bool,
}

/// Software sqrt for no_std (riscv32imac has no FPU).
/// Five Newton-Raphson iterations give ~7 digits of precision.
/// When the `std` feature is enabled, delegates to `f32::sqrt()`.
pub fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    #[cfg(feature = "std")]
    {
        x.sqrt()
    }
    #[cfg(not(feature = "std"))]
    {
        let mut g = x;
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g = 0.5 * (g + x / g);
        g
    }
}

/// Default coupling strength (epsilon). 0.15 converges in ~10-15 cycles for N=2.
pub const FIREFLY_EPSILON: f32 = 0.15;
/// Refractory threshold: ignore incoming pulses when phase < this value.
/// Biological fireflies are unresponsive for ~60-80% of their cycle; we use 30%
/// since our cycle is shorter and we want visible convergence speed.
pub const FIREFLY_REFRACTORY: f32 = 0.3;
/// Duration of the "fire flash" visual in ms (peak brightness hold).
pub const FIRE_FLASH_MS: u64 = 150;

impl FireflyOscillator {
    /// Create a new oscillator with given coupling strength and refractory threshold.
    pub fn new(epsilon: f32, refractory_end: f32) -> Self {
        Self {
            phase: 0.0,
            epsilon: epsilon.clamp(0.001, 0.5),
            refractory_end: refractory_end.clamp(0.0, 0.8),
            just_fired: false,
        }
    }

    /// Advance phase by dt_ms / period_ms. Returns true if the oscillator fired.
    pub fn advance(&mut self, dt_ms: u64, period_ms: u64) -> bool {
        let period = if period_ms == 0 { 1 } else { period_ms };
        self.phase += dt_ms as f32 / period as f32;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            // If we overshot by more than a full period, clamp
            if self.phase >= 1.0 {
                self.phase = 0.0;
            }
            self.just_fired = true;
            true
        } else {
            self.just_fired = false;
            false
        }
    }

    /// Receive a pulse from a peer. Advances the internal state by epsilon
    /// using the concave state function f(phi) = 2*phi - phi^2, which amplifies
    /// the phase advance near threshold (Mirollo-Strogatz synchronization).
    /// Returns true if the pulse caused a fire (absorption).
    pub fn receive_pulse(&mut self) -> bool {
        if self.phase < self.refractory_end {
            return false; // refractory: ignore
        }
        // Map to state space: x = f(phi) = 2*phi - phi*phi
        let x = 2.0 * self.phase - self.phase * self.phase;
        let x_new = x + self.epsilon;
        if x_new >= 1.0 {
            // Absorption: pulse pushes us past threshold
            self.phase = 0.0;
            self.just_fired = true;
            true
        } else {
            // Inverse map: g(x) = 1 - sqrt(1 - x)
            self.phase = 1.0 - sqrt_f32(1.0 - x_new);
            false
        }
    }

    /// Brightness factor in [0, 1]. Concave-up (quadratic): slow build-up
    /// that accelerates toward the flash, like a real firefly's glow charge-up.
    pub fn brightness_factor(&self) -> f32 {
        self.phase * self.phase
    }

    /// Current phase in [0, 1).
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Whether the oscillator fired on the last advance() or receive_pulse().
    pub fn just_fired(&self) -> bool {
        self.just_fired
    }

    /// Set phase directly (for initialization from MAC/uptime).
    pub fn set_phase(&mut self, phase: f32) {
        self.phase = phase.clamp(0.0, 0.9999);
        self.just_fired = false;
    }
}

/// Compute the breathing period from activity_rate in [0, 1].
/// Busy -> fast (1.5 s), idle -> slow (4 s).
pub fn compute_breath_period_ms(activity_rate: f32) -> u64 {
    let ar = activity_rate.clamp(0.0, 1.0);
    let range = (BREATH_PERIOD_MAX_MS - BREATH_PERIOD_MIN_MS) as f32;
    BREATH_PERIOD_MAX_MS - (ar * range) as u64
}

/// Triangle-wave breathing modulation of `base_val`.
/// Returns a value that oscillates +/-40% of base but never drops below [`BREATHING_FLOOR`].
pub fn compute_breathing_val(base_val: u8, uptime_ms: u64, breath_period_ms: u64) -> u8 {
    compute_breathing_val_with_offset(base_val, uptime_ms, breath_period_ms, 0)
}

/// Like [`compute_breathing_val`] but with a phase offset in ms for firefly sync.
/// Both boards converge on the same `phase_offset_ms` so they breathe together.
pub fn compute_breathing_val_with_offset(
    base_val: u8,
    uptime_ms: u64,
    breath_period_ms: u64,
    phase_offset_ms: u64,
) -> u8 {
    let period = if breath_period_ms == 0 { 1 } else { breath_period_ms };
    let adjusted = uptime_ms.wrapping_add(phase_offset_ms);
    let phase = (adjusted % period) as f32 / period as f32;
    // triangle: 0->1->0 over one period
    let tri = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
    // modulate: 60% at trough, 100% at peak
    let factor = 0.6 + 0.4 * tri;
    let v = (base_val as f32 * factor) as u8;
    v.max(BREATHING_FLOOR)
}

/// Compute the steady-state LED colour from node telemetry (no temporal overlays).
pub fn compute_led_steady(state: &NodeState) -> LedOutput {
    // --- hue: topology + RSSI shift + energy drift ---
    let base_hue: i16 = match state.peer_count {
        0 => HUE_ISOLATED as i16,
        1 => HUE_ONE_PEER as i16,
        2 => HUE_TWO_PEERS as i16,
        _ => HUE_THREE_PLUS as i16,
    };

    let rssi_shift: i16 = if state.peer_count > 0 && state.last_rssi > -128 {
        let clamped = state.last_rssi.clamp(RSSI_WEAK, RSSI_STRONG);
        let norm = (clamped - RSSI_WEAK) as f32 / (RSSI_STRONG - RSSI_WEAK) as f32;
        ((norm * 16.0) - 8.0) as i16
    } else {
        0
    };

    let drift_shift: i16 = if state.energy_delta < -0.05 {
        -5 // draining -> shift toward red
    } else if state.energy_delta > 0.02 {
        3 // stable/charging -> shift toward blue
    } else {
        0
    };

    let hue = (base_hue + rssi_shift + drift_shift).clamp(0, 255) as u8;

    // --- saturation: link freshness + error rate ---
    let freshness_sat: f32 = if state.peer_count == 0 {
        255.0
    } else {
        let t = (state.ms_since_last_rx as f32 / PEER_TIMEOUT_MS as f32).clamp(0.0, 1.0);
        255.0 - (t * 135.0) // 255 -> 120 over 30 s
    };

    let error_desat: f32 = if state.tx_ok + state.tx_err > 10 {
        let rate = state.tx_err as f32 / (state.tx_ok + state.tx_err) as f32;
        if rate > 0.20 { (rate * 350.0).min(175.0) } else { 0.0 }
    } else {
        0.0
    };

    let sat = ((freshness_sat - error_desat).clamp(80.0, 255.0)) as u8;

    // --- brightness: energy level ---
    let val = BRIGHTNESS_MIN as f32
        + state.energy_score.clamp(0.0, 1.0) * (BRIGHTNESS_MAX - BRIGHTNESS_MIN) as f32;
    let val = (val as u8).max(BRIGHTNESS_MIN);

    LedOutput { hue, sat, val }
}

// ---------------------------------------------------------------------------
// Kuramoto order parameter — phase coherence metric
// ---------------------------------------------------------------------------

/// Compute the Kuramoto order parameter R for a set of oscillator phases.
/// R = |mean(e^{i * 2pi * phase})|, where each phase is in [0, 1).
/// R = 1.0 means perfect synchronization, R = 0.0 means uniform spread.
/// Uses no_std-compatible sin/cos approximation (Bhaskara I formula).
pub fn kuramoto_order_parameter(phases: &[f32]) -> f32 {
    if phases.is_empty() {
        return 0.0;
    }
    let n = phases.len() as f32;
    let mut sum_cos = 0.0f32;
    let mut sum_sin = 0.0f32;
    for &p in phases {
        let theta = p * core::f32::consts::TAU; // phase [0,1) -> angle [0, 2pi)
        sum_cos += cos_approx(theta);
        sum_sin += sin_approx(theta);
    }
    let avg_cos = sum_cos / n;
    let avg_sin = sum_sin / n;
    sqrt_f32(avg_cos * avg_cos + avg_sin * avg_sin)
}

/// Bhaskara I sine approximation: accurate to ~0.2% for any angle.
/// Maps angle to [0, pi] range then uses 16x(pi-x) / (5pi^2 - 4x(pi-x)).
pub fn sin_approx(x: f32) -> f32 {
    use core::f32::consts::PI;
    // Normalize to [0, 2*PI)
    let x = x % (2.0 * PI);
    let x = if x < 0.0 { x + 2.0 * PI } else { x };
    // sin is negative in [PI, 2*PI)
    let (x, sign) = if x > PI { (x - PI, -1.0f32) } else { (x, 1.0f32) };
    // Bhaskara I formula for [0, PI]
    let num = 16.0 * x * (PI - x);
    let den = 5.0 * PI * PI - 4.0 * x * (PI - x);
    sign * num / den
}

/// Cosine via sin(x + pi/2).
pub fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::FRAC_PI_2)
}

// ---------------------------------------------------------------------------
// Perceptual brightness — gamma correction for CIE L* uniformity
// ---------------------------------------------------------------------------

/// Apply approximate gamma 2.2 correction to a linear brightness value.
/// Maps linear [0, 255] to perceptually uniform [0, 255].
/// Without this, dim LED fades show visible stepping (Weber-Fechner law).
/// Uses a piecewise quadratic approximation (no pow/exp in no_std).
pub fn gamma_correct(linear: u8) -> u8 {
    // Approximate x^2.2 ~ x^2 * x^0.2 ~ x^2 * (1 + 0.2*ln(x))
    // Simpler: use x^2 / 255 which is gamma 2.0 (close enough for 8-bit)
    let x = linear as u16;
    ((x * x + 127) / 255) as u8
}

// ---------------------------------------------------------------------------
// Temperature-to-energy mapping — pure function
// ---------------------------------------------------------------------------

/// Map chip temperature (Celsius) to energy score [0.05, 1.0].
/// Cooler = more energy headroom. Linear mapping: 0C -> 1.0, 80C -> 0.0.
/// Clamped so the score never reaches exactly zero (always visible on LED).
pub fn temp_to_energy(celsius: f32) -> f32 {
    ((80.0 - celsius) / 80.0).clamp(0.05, 1.0)
}

// ---------------------------------------------------------------------------
// Peer table — extracted from firmware main loop
// ---------------------------------------------------------------------------

/// Maximum number of tracked peers.
pub const MAX_PEERS: usize = 6;

/// A tracked peer in the mesh.
#[derive(Debug, Clone)]
pub struct PeerEntry {
    pub mac: [u8; 6],
    pub last_seen_ms: u64,
    pub last_rssi: i16,
}

/// Peer table: fixed-size array of optional peer slots.
/// Manages add/refresh/prune with configurable timeout.
#[derive(Debug, Clone)]
pub struct PeerTable {
    peers: [Option<PeerEntry>; MAX_PEERS],
}

impl Default for PeerTable {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerTable {
    pub fn new() -> Self {
        Self {
            peers: [const { None }; MAX_PEERS],
        }
    }

    /// Number of active peers.
    pub fn count(&self) -> usize {
        self.peers.iter().filter(|p| p.is_some()).count()
    }

    /// Add or refresh a peer. Returns `Ok(true)` if new, `Ok(false)` if refreshed,
    /// `Err(())` if table is full.
    pub fn add_or_refresh(&mut self, mac: [u8; 6], now_ms: u64, rssi: i16) -> Result<bool, ()> {
        // Refresh existing
        for slot in self.peers.iter_mut() {
            if let Some(ref mut entry) = slot {
                if entry.mac == mac {
                    entry.last_seen_ms = now_ms;
                    entry.last_rssi = rssi;
                    return Ok(false);
                }
            }
        }
        // Insert new
        for slot in self.peers.iter_mut() {
            if slot.is_none() {
                *slot = Some(PeerEntry { mac, last_seen_ms: now_ms, last_rssi: rssi });
                return Ok(true);
            }
        }
        Err(()) // full
    }

    /// Prune peers not seen within `timeout_ms`. Returns MACs of pruned peers.
    pub fn prune(&mut self, now_ms: u64, timeout_ms: u64) -> Vec<[u8; 6]> {
        let mut pruned = Vec::new();
        for slot in self.peers.iter_mut() {
            let should_prune = if let Some(ref entry) = slot {
                now_ms.saturating_sub(entry.last_seen_ms) > timeout_ms
            } else {
                false
            };
            if should_prune {
                if let Some(ref entry) = slot {
                    pruned.push(entry.mac);
                }
                *slot = None;
            }
        }
        pruned
    }

    /// Most recent RSSI across all peers, or -128 if no peers.
    pub fn best_rssi(&self) -> i16 {
        self.peers
            .iter()
            .filter_map(|p| p.as_ref())
            .map(|p| p.last_rssi)
            .max()
            .unwrap_or(-128)
    }

    /// Milliseconds since last RX from any peer (relative to `now_ms`).
    /// Returns `u64::MAX` if no peers.
    pub fn ms_since_last_rx(&self, now_ms: u64) -> u64 {
        self.peers
            .iter()
            .filter_map(|p| p.as_ref())
            .map(|p| now_ms.saturating_sub(p.last_seen_ms))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Iterator over active peers.
    pub fn iter(&self) -> impl Iterator<Item = &PeerEntry> {
        self.peers.iter().filter_map(|p| p.as_ref())
    }

    /// Get all current phases (placeholder -- for simulation, phases are tracked externally).
    pub fn macs(&self) -> Vec<[u8; 6]> {
        self.peers.iter().filter_map(|p| p.as_ref().map(|e| e.mac)).collect()
    }
}

// ---------------------------------------------------------------------------
// Energy smoother — exponential moving average
// ---------------------------------------------------------------------------

/// EMA-based energy tracker with trend (delta) computation.
#[derive(Debug, Clone)]
pub struct EnergySmoother {
    pub smoothed: f32,
    pub delta: f32,
    prev_for_trend: f32,
    smooth_alpha: f32,
    trend_alpha: f32,
}

impl EnergySmoother {
    pub fn new(initial: f32, smooth_alpha: f32, trend_alpha: f32) -> Self {
        Self {
            smoothed: initial,
            delta: 0.0,
            prev_for_trend: initial,
            smooth_alpha,
            trend_alpha,
        }
    }

    /// Feed a new raw sample. Call every TX interval.
    pub fn update(&mut self, raw: f32) {
        self.smoothed = self.smooth_alpha * raw + (1.0 - self.smooth_alpha) * self.smoothed;
    }

    /// Update trend (delta). Call less frequently (e.g., every 10s).
    pub fn update_trend(&mut self) {
        let raw_delta = self.smoothed - self.prev_for_trend;
        self.delta = self.trend_alpha * raw_delta + (1.0 - self.trend_alpha) * self.delta;
        self.prev_for_trend = self.smoothed;
    }
}

// ---------------------------------------------------------------------------
// Overlay state machine — temporal LED effects
// ---------------------------------------------------------------------------

/// Timing constants for temporal overlays (in ms).
pub const TX_BUMP_MS: u64 = 200;
pub const ERROR_FLASH_MS: u64 = 150;
pub const ERROR_FLASH_INTERVAL_MS: u64 = 5_000;
pub const TX_INTERVAL_MS: u64 = 2_000;
pub const BOOT_GRACE_MS: u64 = 2_500;

/// Which overlay is currently driving the LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedMode {
    Firefly,
    Fire,
    TxBump,
    ErrorFlash,
}

/// Temporal overlay state machine. Tracks active overlays and resolves priority.
///
/// Priority: error_flash > fire_flash > tx_bump > firefly (steady).
#[derive(Debug, Clone)]
pub struct OverlayState {
    /// Millisecond timestamp when each overlay expires (0 = inactive).
    pub fire_flash_until: u64,
    pub tx_bump_until: u64,
    pub error_flash_until: u64,
    pub last_error_flash: u64,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayState {
    pub fn new() -> Self {
        Self {
            fire_flash_until: 0,
            tx_bump_until: 0,
            error_flash_until: 0,
            last_error_flash: 0,
        }
    }

    /// Trigger a fire flash at `now_ms`.
    pub fn trigger_fire(&mut self, now_ms: u64) {
        self.fire_flash_until = now_ms + FIRE_FLASH_MS;
    }

    /// Trigger a TX bump at `now_ms`.
    pub fn trigger_tx_bump(&mut self, now_ms: u64) {
        self.tx_bump_until = now_ms + TX_BUMP_MS;
    }

    /// Conditionally trigger an error flash based on TX error rate.
    pub fn maybe_trigger_error(&mut self, now_ms: u64, tx_ok: u32, tx_err: u32) {
        let total = tx_ok.saturating_add(tx_err);
        if total > 10 {
            let rate = tx_err as f32 / total as f32;
            if rate > 0.10 && now_ms.saturating_sub(self.last_error_flash) >= ERROR_FLASH_INTERVAL_MS {
                self.last_error_flash = now_ms;
                self.error_flash_until = now_ms + ERROR_FLASH_MS;
            }
        }
    }

    /// Resolve the current LED output given steady-state HSV, oscillator state,
    /// and current time. Returns `(hue, sat, val, mode)`.
    pub fn resolve(
        &self,
        steady: &LedOutput,
        osc_val: u8,
        now_ms: u64,
    ) -> (u8, u8, u8, LedMode) {
        if now_ms < self.error_flash_until {
            (0, 255, 120, LedMode::ErrorFlash)
        } else if now_ms < self.fire_flash_until {
            (steady.hue, steady.sat, steady.val, LedMode::Fire)
        } else {
            let mut val_boost: u16 = 0;
            let mut mode = LedMode::Firefly;
            if now_ms < self.tx_bump_until {
                let remaining = self.tx_bump_until - now_ms;
                let progress = remaining as f32 / TX_BUMP_MS as f32;
                val_boost = (20.0 * progress) as u16;
                mode = LedMode::TxBump;
            }
            let final_val = (osc_val as u16 + val_boost).min(BRIGHTNESS_MAX as u16) as u8;
            (steady.hue, steady.sat, final_val, mode)
        }
    }
}

// ---------------------------------------------------------------------------
// MeshNode — complete node state (oscillator + peers + energy + overlays)
// ---------------------------------------------------------------------------

/// Complete mesh node state, suitable for simulation or firmware use.
/// Ties together all sub-components and implements the full tick/receive protocol.
#[derive(Debug, Clone)]
pub struct MeshNode {
    pub mac: [u8; 6],
    pub oscillator: FireflyOscillator,
    pub peer_table: PeerTable,
    pub energy: EnergySmoother,
    pub overlays: OverlayState,
    /// Oscillator period (derived from activity_rate).
    pub period_ms: u64,
    /// TX counters.
    pub tx_ok: u32,
    pub tx_err: u32,
    /// RX counter (for activity rate).
    pub rx_count: u32,
    /// Time of last RX.
    pub last_rx_ms: u64,
    /// Activity rate [0, 1].
    pub activity_rate: f32,
    /// Simulated temperature (Celsius).
    pub temperature: f32,
    /// Current local time (ms).
    pub local_ms: u64,
    /// Boot grace: suppress sync reactions until this time.
    pub boot_grace_until: u64,
}

/// Result of a `MeshNode::tick()` call.
#[derive(Debug, Clone)]
pub struct TickResult {
    /// Whether the oscillator fired this tick.
    pub fired: bool,
    /// Current LED output (after overlays).
    pub hue: u8,
    pub sat: u8,
    pub val: u8,
    pub mode: LedMode,
    /// Oscillator phase after this tick.
    pub phase: f32,
}

/// Result of receiving a pulse.
#[derive(Debug, Clone, Copy)]
pub struct ReceiveResult {
    /// Whether this was a new peer (vs refresh).
    pub new_peer: bool,
    /// Whether the pulse caused absorption (immediate fire).
    pub absorbed: bool,
    /// Whether the peer table overflowed.
    pub overflow: bool,
}

impl MeshNode {
    pub fn new(mac: [u8; 6], temperature: f32) -> Self {
        let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        // Seed phase from last byte of MAC for initial diversity
        osc.set_phase(mac[5] as f32 / 255.0);
        Self {
            mac,
            oscillator: osc,
            peer_table: PeerTable::new(),
            energy: EnergySmoother::new(temp_to_energy(temperature), 0.2, 0.3),
            overlays: OverlayState::new(),
            period_ms: BREATH_PERIOD_MAX_MS,
            tx_ok: 0,
            tx_err: 0,
            rx_count: 0,
            last_rx_ms: 0,
            activity_rate: 0.0,
            temperature,
            local_ms: 0,
            boot_grace_until: BOOT_GRACE_MS,
        }
    }

    /// Advance the node by `dt_ms` milliseconds. Returns tick result with LED output.
    pub fn tick(&mut self, dt_ms: u64) -> TickResult {
        self.local_ms += dt_ms;

        // Update period from activity rate
        self.period_ms = compute_breath_period_ms(self.activity_rate);

        // Advance oscillator
        let fired = self.oscillator.advance(dt_ms, self.period_ms);
        if fired {
            self.overlays.trigger_fire(self.local_ms);
        }

        // Build node state for LED computation
        let ms_since_rx = if self.peer_table.count() == 0 {
            0
        } else {
            self.local_ms.saturating_sub(self.last_rx_ms)
        };
        let state = NodeState {
            peer_count: self.peer_table.count(),
            energy_score: self.energy.smoothed,
            energy_delta: self.energy.delta,
            tx_ok: self.tx_ok,
            tx_err: self.tx_err,
            last_rssi: self.peer_table.best_rssi(),
            ms_since_last_rx: ms_since_rx,
            activity_rate: self.activity_rate,
            uptime_ms: self.local_ms,
        };
        let steady = compute_led_steady(&state);

        // Oscillator brightness modulation
        let fire_factor = self.oscillator.brightness_factor();
        let osc_val = ((steady.val as f32) * (0.6 + 0.4 * fire_factor)) as u8;
        let osc_val = osc_val.max(BREATHING_FLOOR);

        // Check for error flash
        self.overlays.maybe_trigger_error(self.local_ms, self.tx_ok, self.tx_err);

        // Resolve overlays
        let (hue, sat, val, mode) = self.overlays.resolve(&steady, osc_val, self.local_ms);

        TickResult { fired, hue, sat, val, mode, phase: self.oscillator.phase() }
    }

    /// Receive a pulse from a peer. Call after boot grace period.
    pub fn receive_pulse(&mut self, from_mac: [u8; 6], rssi: i16) -> ReceiveResult {
        // Peer tracking (always)
        let peer_result = self.peer_table.add_or_refresh(from_mac, self.local_ms, rssi);
        let new_peer = matches!(peer_result, Ok(true));
        let overflow = peer_result.is_err();

        // Update RX tracking
        self.last_rx_ms = self.local_ms;
        self.rx_count += 1;

        // Firefly coupling (only after boot grace)
        let absorbed = if self.local_ms > self.boot_grace_until {
            let abs = self.oscillator.receive_pulse();
            if abs {
                self.overlays.trigger_fire(self.local_ms);
            }
            abs
        } else {
            false
        };

        ReceiveResult { new_peer, absorbed, overflow }
    }

    /// Prune stale peers. Call periodically (e.g., every 1s).
    pub fn prune_peers(&mut self) -> Vec<[u8; 6]> {
        self.peer_table.prune(self.local_ms, PEER_TIMEOUT_MS)
    }

    /// Update energy from temperature. Call periodically (e.g., every TX interval).
    pub fn update_energy(&mut self) {
        self.energy.update(temp_to_energy(self.temperature));
    }

    /// Update energy trend. Call less frequently (e.g., every 10s).
    pub fn update_energy_trend(&mut self) {
        self.energy.update_trend();
    }

    /// Update activity rate. Call periodically (e.g., every 10s).
    pub fn update_activity_rate(&mut self) {
        self.activity_rate = ((self.tx_ok + self.rx_count) as f32 / 20.0).min(1.0);
    }
}
