//! WS2812 status LED — a high-resolution carousel of bio-inspired signals.
//!
//! One RGB LED carries ~3 preattentive channels at once (hue, brightness,
//! motion), so to show MORE state we time-multiplex pages with a crossfade.
//! Rendering is done in full f32 and TEMPORALLY DITHERED to 8-bit at 125 Hz:
//! the WS2812's 8 bits give only ~8 visible steps at whisper brightness (hard
//! banding), so the fractional remainder is error-diffused across frames to
//! recover effectively ~12-bit smoothness. Whisper-dim; info lives in hue +
//! motion, not brightness.
//!
//! Pages (auto-rotate by default; pin/disable via hypha/<board>/cmd
//! {"led":"auto"|"metabolism"|"link"|"version"|"off"}, private design note):
//!   - Metabolism: hue from BLE advert activity (per-room signature), breath
//!     period from activity, firefly heartbeat flash (synchronized across
//!     boards, firefly.rs) that winks the firmware VERSION colour.
//!   - Link: hue green->red from WiFi RSSI.
//!   - Version: steady firmware-version hue (watch an OTA wave room to room).
//! Locate (operator find-me, magenta blink) and fault (MQTT down) override
//! every page. Boot shows a fast rainbow spin, distinct from both.

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

const TICK_MS: u64 = 8; // 125 Hz: smooth motion + headroom for temporal dither
const FLASH_TAU: f32 = 0.13; // firefly flash decay time constant (s)
const BOOT_BLOOM_S: f32 = 3.0;
const BOOT_REVS: f32 = 3.0; // rainbow revolutions during boot
const MQTT_FAULT_AFTER_S: u64 = 120;

pub const MODE_AUTO: u8 = 0;
pub const MODE_METABOLISM: u8 = 1;
pub const MODE_LINK: u8 = 2;
pub const MODE_VERSION: u8 = 3;
pub const MODE_OFF: u8 = 4;

const PAGE_SECS: f32 = 6.0;
const FADE_SECS: f32 = 1.0;

type Rgb = (f32, f32, f32); // 0..255 per channel, full resolution

fn max_val() -> f32 {
    option_env!("LED_MAX_VAL")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24.0)
}

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
    let dt = TICK_MS as f32 / 1000.0;
    let flash_decay = (-dt / FLASH_TAU).exp();
    let mut last_connected = Instant::now();

    let mut rate: f32 = 0.0;
    let mut last_adverts = stats.adverts_seen.load(Ordering::Relaxed);
    let mut last_rate_t = boot;
    let mut last_fire = stats.fire.load(Ordering::Relaxed);
    let mut flash: f32 = 0.0;
    let mut dith = [0.0f32; 3]; // temporal-dither error accumulators

    loop {
        let now = Instant::now();
        let t = boot.elapsed().as_secs_f32();
        if stats.mqtt_connected.load(Ordering::Relaxed) {
            last_connected = now;
        }
        if now.duration_since(last_rate_t).as_millis() >= 1000 {
            let cur = stats.adverts_seen.load(Ordering::Relaxed);
            let d = now.duration_since(last_rate_t).as_secs_f32().max(0.001);
            rate = 0.4 * (cur.wrapping_sub(last_adverts) as f32 / d) + 0.6 * rate;
            last_adverts = cur;
            last_rate_t = now;
        }
        let fires = stats.fire.load(Ordering::Relaxed);
        if fires != last_fire {
            last_fire = fires;
            flash = 1.0;
        }
        flash *= flash_decay;

        let mode = stats.led_mode.load(Ordering::Relaxed);
        let rgb: Rgb = if mode == MODE_OFF {
            (0.0, 0.0, 0.0)
        } else if stats.locate.load(Ordering::Relaxed) {
            // locate: operator find-me, magenta blink (never fired by boot)
            if (t * 1.5).fract() < 0.35 { (48.0, 0.0, 48.0) } else { (0.0, 0.0, 0.0) }
        } else if t < BOOT_BLOOM_S {
            // boot: an EXPONENTIAL "ignition" -- the hue spin accelerates
            // (warp-up, cubic progress) and the brightness flares then decays
            // exponentially (a spark, not a sinusoidal breath). Deliberately
            // unique vs every other page; brighter than the whisper cap because
            // boot is the one loud "I just (re)started" moment.
            let p = (t / BOOT_BLOOM_S).clamp(0.0, 1.0);
            let warp = p * p * p; // accelerating spin
            let hue = (warp * 360.0 * BOOT_REVS * 2.0) % 360.0;
            let env = (1.0 - (-p * 10.0).exp()) * (-p * 2.2).exp(); // sharp attack, exp tail
            hsv(hue, 1.0, env * 50.0)
        } else if last_connected.elapsed().as_secs() > MQTT_FAULT_AFTER_S {
            let v = sin01(t) * 24.0;
            (v, v * 0.4, 0.0) // amber breath: MQTT down
        } else {
            let ctx = PageCtx { t, cap, ver_hue, rate, flash, rssi: stats.wifi_rssi.load(Ordering::Relaxed) };
            match mode {
                MODE_METABOLISM => page(Page::Metabolism, &ctx),
                MODE_LINK => page(Page::Link, &ctx),
                MODE_VERSION => page(Page::Version, &ctx),
                _ => auto_rotate(&ctx),
            }
        };

        if let Err(e) = write_dithered(&mut tx, rgb, &mut dith) {
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
    cap: f32,
    ver_hue: f32,
    rate: f32,
    flash: f32,
    rssi: i32,
}

fn auto_rotate(c: &PageCtx) -> Rgb {
    const PAGES: [Page; 3] = [Page::Metabolism, Page::Link, Page::Version];
    let cycle = PAGE_SECS * PAGES.len() as f32;
    let phase = c.t % cycle;
    let idx = (phase / PAGE_SECS) as usize % PAGES.len();
    let into = phase - idx as f32 * PAGE_SECS;
    let cur = page(PAGES[idx], c);
    if into > PAGE_SECS - FADE_SECS {
        let nxt = page(PAGES[(idx + 1) % PAGES.len()], c);
        blend(cur, nxt, (into - (PAGE_SECS - FADE_SECS)) / FADE_SECS)
    } else {
        cur
    }
}

fn page(p: Page, c: &PageCtx) -> Rgb {
    match p {
        Page::Metabolism => {
            let busy = (c.rate / 30.0).clamp(0.0, 1.0);
            let act_hue = 130.0 + busy * 75.0;
            let hue = lerp_hue(act_hue, c.ver_hue, c.flash);
            let period = 4.0 - busy * 2.4;
            let base = 2.0 + sin01(c.t / period) * (c.cap * 0.35);
            let val = (base + c.flash * (c.cap * 0.6)).min(c.cap);
            hsv(hue, 0.85, val)
        }
        Page::Link => {
            // rhythm signature: STEADY (no breath) -- "stillness" is its motion.
            // Hue green->red from RSSI; brightness redundantly encodes strength
            // (stronger link = brighter) so it reads without colour (CVD-safe).
            let q = ((c.rssi as f32 + 90.0) / 60.0).clamp(0.0, 1.0);
            hsv(q * 120.0, 0.9, c.cap * (0.35 + 0.35 * q))
        }
        Page::Version => {
            // rhythm signature: a slow DOUBLE-PULSE (blip-blip ... pause), so
            // the version page is identifiable by motion, not just by hue.
            let ph = c.t % 4.0;
            let pulse = bump((ph / 0.5).min(1.0)) + if ph > 0.7 && ph < 1.2 {
                bump(((ph - 0.7) / 0.5).min(1.0))
            } else {
                0.0
            };
            hsv(c.ver_hue, 0.9, 2.0 + pulse.min(1.0) * (c.cap * 0.7))
        }
    }
}

fn blend(a: Rgb, b: Rgb, f: f32) -> Rgb {
    let f = f.clamp(0.0, 1.0);
    (
        a.0 * (1.0 - f) + b.0 * f,
        a.1 * (1.0 - f) + b.1 * f,
        a.2 * (1.0 - f) + b.2 * f,
    )
}

fn lerp_hue(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 { d -= 360.0; } else if d < -180.0 { d += 360.0; }
    a + d * t.clamp(0.0, 1.0)
}

fn sin01(phase: f32) -> f32 {
    0.5 - 0.5 * (phase * core::f32::consts::TAU).cos()
}

fn smootherstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// 0..1..0 smooth hump (eased rise and fall).
fn bump(p: f32) -> f32 {
    if p < 0.5 { smootherstep(p * 2.0) } else { smootherstep((1.0 - p) * 2.0) }
}

/// HSV (h deg, s/v 0..1 with v scaled to 0..255 here) -> f32 RGB 0..255.
fn hsv(h: f32, s: f32, v255: f32) -> Rgb {
    let v = (v255 / 255.0).clamp(0.0, 1.0);
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
    ((r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0)
}

/// Temporal-dither one f32 channel to u8: floor most frames, floor+1 often
/// enough that the time-average equals the f32 value (recovers sub-LSB levels).
fn dither(v: f32, acc: &mut f32) -> u32 {
    let v = v.clamp(0.0, 255.0);
    let base = v.floor();
    *acc += v - base;
    let extra = if *acc >= 1.0 { *acc -= 1.0; 1.0 } else { 0.0 };
    (base + extra) as u32
}

fn write_dithered(tx: &mut TxRmtDriver<'static>, rgb: Rgb, acc: &mut [f32; 3]) -> anyhow::Result<()> {
    let r = dither(rgb.0, &mut acc[0]);
    let g = dither(rgb.1, &mut acc[1]);
    let b = dither(rgb.2, &mut acc[2]);
    write_rgb(tx, r, g, b)
}

/// WS2812 bit timing over RMT (GRB order).
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
