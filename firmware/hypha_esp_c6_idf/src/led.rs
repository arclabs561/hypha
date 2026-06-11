//! WS2812 status LED — a carousel of bio-inspired metabolic signals.
//!
//! One RGB LED carries ~3 preattentive channels at once (hue, brightness,
//! motion), so to show MORE state we time-multiplex: the display rotates
//! through a few "pages," each a glanceable signal, with a smooth crossfade
//! between them. Smooth (~33 Hz) and whisper-dim; the information lives in
//! hue + motion, not brightness.
//!
//! Pages (auto-rotate by default; pin or disable via hypha/<board>/cmd
//! {"led":"auto"|"metabolism"|"link"|"version"|"off"}, private design note):
//!   - Metabolism: hue from BLE advert activity (per-room signature), breath
//!     period from activity, firefly heartbeat flash on each publish that
//!     winks the firmware VERSION colour (version-as-colour during rollout).
//!   - Link: hue green->red from WiFi RSSI (uplink health at a glance).
//!   - Version: steady firmware-version hue (pin the fleet here during an OTA
//!     rollout to watch the version wave move room to room).
//! Locate (operator identify) and fault (MQTT down) override every page.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::OutputPin;
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::rmt::config::TransmitConfig;
use esp_idf_svc::hal::rmt::{FixedLengthSignal, PinState, Pulse, RmtChannel, TxRmtDriver};
use log::warn;

use crate::Stats;

const TICK_MS: u64 = 30; // ~33 Hz
// Boot shows a calm version-coloured BLOOM (a single fade-in/out), never the
// locate blink: the high-salience magenta blink is reserved for operator
// "find me" so it never cries wolf on a routine reboot/upgrade. Upgrades are
// read passively from the version hue, not from any blink.
const BOOT_BLOOM_S: f32 = 4.0;
const MQTT_FAULT_AFTER_S: u64 = 120;
const PAGE_SECS: f32 = 6.0; // dwell per page in auto-rotate
const FADE_SECS: f32 = 1.0; // crossfade between pages

// led_mode atomic values (set from the cmd topic, read here).
pub const MODE_AUTO: u8 = 0;
pub const MODE_METABOLISM: u8 = 1;
pub const MODE_LINK: u8 = 2;
pub const MODE_VERSION: u8 = 3;
pub const MODE_OFF: u8 = 4;

fn max_val() -> u32 {
    option_env!("LED_MAX_VAL")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
}

/// Deterministic heartbeat/identity hue per firmware version.
fn version_hue() -> f32 {
    let mut h: u32 = 2166136261;
    for b in env!("CARGO_PKG_VERSION").bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (h % 360) as f32
}

pub fn spawn(
    channel: impl Peripheral<P = impl RmtChannel> + 'static,
    pin: impl Peripheral<P = impl OutputPin> + 'static,
    stats: Arc<Stats>,
) {
    if option_env!("LED_OFF") == Some("1") {
        return;
    }
    let driver = match TxRmtDriver::new(channel, pin, &TransmitConfig::new().clock_divider(1)) {
        Ok(d) => d,
        Err(e) => {
            warn!("LED disabled (RMT init failed: {:?})", e);
            return;
        }
    };
    let _ = thread::Builder::new()
        .name("led".into())
        .stack_size(4096)
        .spawn(move || led_loop(driver, stats));
}

fn led_loop(mut tx: TxRmtDriver<'static>, stats: Arc<Stats>) {
    let boot = Instant::now();
    let cap = max_val();
    let ver_hue = version_hue();
    let mut last_connected = Instant::now();

    let mut rate: f32 = 0.0;
    let mut last_adverts = stats.adverts_seen.load(Ordering::Relaxed);
    let mut last_rate_t = boot;
    let mut last_fire = stats.fire.load(Ordering::Relaxed);
    let mut flash: f32 = 0.0;

    loop {
        let now = Instant::now();
        let up = boot.elapsed().as_secs();
        if stats.mqtt_connected.load(Ordering::Relaxed) {
            last_connected = now;
        }
        if now.duration_since(last_rate_t).as_millis() >= 1000 {
            let cur = stats.adverts_seen.load(Ordering::Relaxed);
            let dt = now.duration_since(last_rate_t).as_secs_f32().max(0.001);
            rate = 0.4 * (cur.wrapping_sub(last_adverts) as f32 / dt) + 0.6 * rate;
            last_adverts = cur;
            last_rate_t = now;
        }
        // heartbeat flash on each firefly fire (synchronized across boards)
        let fires = stats.fire.load(Ordering::Relaxed);
        if fires != last_fire {
            last_fire = fires;
            flash = 1.0;
        }
        flash *= 0.82;

        let t = boot.elapsed().as_millis() as f32 / 1000.0;
        let mode = stats.led_mode.load(Ordering::Relaxed);

        let (r, g, b) = if mode == MODE_OFF {
            (0, 0, 0)
        } else if stats.locate.load(Ordering::Relaxed) {
            // locate: operator "find me" only — unmistakable magenta blink,
            // never fired by boot/upgrade (those bloom in the version hue below)
            if (t * 1.5).fract() < 0.35 { (48, 0, 48) } else { (0, 0, 0) }
        } else if (up as f32) < BOOT_BLOOM_S {
            // boot bloom: a rainbow hue sweep that fades in and out — a fun,
            // unmistakable "I just (re)started" distinct from the locate blink;
            // the version is read from the carousel, not from boot
            let p = (up as f32 + (t % 1.0)) / BOOT_BLOOM_S; // 0..1 over the window
            let hue = (p * 360.0) % 360.0;
            let v = sin01_half(p) * (cap as f32 * 0.7);
            hsv(hue, 0.95, v / 255.0)
        } else if last_connected.elapsed().as_secs() > MQTT_FAULT_AFTER_S {
            let v = (sin01(t) * 24.0) as u32;
            (v, v * 2 / 5, 0) // amber breath: MQTT down
        } else {
            // pick the page (pinned, or the auto-rotation's current+next blend)
            let ctx = PageCtx { t, cap, ver_hue, rate, flash, rssi: stats.wifi_rssi.load(Ordering::Relaxed) };
            match mode {
                MODE_METABOLISM => page(Page::Metabolism, &ctx),
                MODE_LINK => page(Page::Link, &ctx),
                MODE_VERSION => page(Page::Version, &ctx),
                _ => auto_rotate(&ctx), // MODE_AUTO
            }
        };

        if let Err(e) = write_rgb(&mut tx, r, g, b) {
            warn!("LED write failed: {:?}", e);
        }
        thread::sleep(Duration::from_millis(TICK_MS));
    }
}

#[derive(Clone, Copy)]
enum Page {
    Metabolism,
    Link,
    Version,
}

struct PageCtx {
    t: f32,
    cap: u32,
    ver_hue: f32,
    rate: f32,
    flash: f32,
    rssi: i32,
}

/// Auto-rotation: crossfade through the three pages on a PAGE_SECS cadence.
fn auto_rotate(c: &PageCtx) -> (u32, u32, u32) {
    const PAGES: [Page; 3] = [Page::Metabolism, Page::Link, Page::Version];
    let cycle = PAGE_SECS * PAGES.len() as f32;
    let phase = c.t % cycle;
    let idx = (phase / PAGE_SECS) as usize % PAGES.len();
    let into = phase - idx as f32 * PAGE_SECS; // seconds into current page
    let cur = page(PAGES[idx], c);
    if into > PAGE_SECS - FADE_SECS {
        let nxt = page(PAGES[(idx + 1) % PAGES.len()], c);
        let f = (into - (PAGE_SECS - FADE_SECS)) / FADE_SECS;
        blend(cur, nxt, f)
    } else {
        cur
    }
}

fn page(p: Page, c: &PageCtx) -> (u32, u32, u32) {
    match p {
        Page::Metabolism => {
            let busy = (c.rate / 30.0).clamp(0.0, 1.0);
            let act_hue = 130.0 + busy * 75.0;
            let hue = lerp_hue(act_hue, c.ver_hue, c.flash); // heartbeat winks version colour
            let period = 4.0 - busy * 2.4;
            let base = 2.0 + sin01(c.t / period) * (c.cap as f32 * 0.35);
            let val = (base + c.flash * (c.cap as f32 * 0.6)).min(c.cap as f32);
            hsv(hue, 0.85, val / 255.0)
        }
        Page::Link => {
            // RSSI -30 (strong, hue 120 green) .. -90 (weak, hue 0 red)
            let q = ((c.rssi as f32 + 90.0) / 60.0).clamp(0.0, 1.0);
            let hue = q * 120.0;
            let val = 2.0 + sin01(c.t / 3.5) * (c.cap as f32 * 0.4);
            hsv(hue, 0.9, val / 255.0)
        }
        Page::Version => {
            let val = 3.0 + sin01(c.t / 3.0) * (c.cap as f32 * 0.4);
            hsv(c.ver_hue, 0.9, val / 255.0)
        }
    }
}

fn blend(a: (u32, u32, u32), b: (u32, u32, u32), f: f32) -> (u32, u32, u32) {
    let f = f.clamp(0.0, 1.0);
    let mix = |x: u32, y: u32| (x as f32 * (1.0 - f) + y as f32 * f) as u32;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 { d -= 360.0; } else if d < -180.0 { d += 360.0; }
    a + d * t.clamp(0.0, 1.0)
}

fn sin01(phase: f32) -> f32 {
    0.5 - 0.5 * (phase * core::f32::consts::TAU).cos()
}

/// 0..1..0 single hump over phase 0..1 (one bloom, not a repeating breath).
fn sin01_half(phase: f32) -> f32 {
    (phase.clamp(0.0, 1.0) * core::f32::consts::PI).sin()
}

fn hsv(h: f32, s: f32, v: f32) -> (u32, u32, u32) {
    let h = (h % 360.0 + 360.0) % 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (q(r), q(g), q(b))
}

fn write_rgb(tx: &mut TxRmtDriver<'static>, r: u32, g: u32, b: u32) -> anyhow::Result<()> {
    let color: u32 = (g << 16) | (r << 8) | b;
    let ticks_hz = tx.counter_clock()?;
    let (t0h, t0l, t1h, t1l) = (
        Pulse::new_with_duration(ticks_hz, PinState::High, &Duration::from_nanos(350))?,
        Pulse::new_with_duration(ticks_hz, PinState::Low, &Duration::from_nanos(800))?,
        Pulse::new_with_duration(ticks_hz, PinState::High, &Duration::from_nanos(700))?,
        Pulse::new_with_duration(ticks_hz, PinState::Low, &Duration::from_nanos(600))?,
    );
    let mut signal = FixedLengthSignal::<24>::new();
    for i in (0..24).rev() {
        let bit = (color >> i) & 1 != 0;
        let (high, low) = if bit { (t1h, t1l) } else { (t0h, t0l) };
        signal.set(23 - i, &(high, low))?;
    }
    tx.start_blocking(&signal)?;
    Ok(())
}
