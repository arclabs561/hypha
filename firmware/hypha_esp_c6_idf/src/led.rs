//! WS2812 status LED — a small, distinct vocabulary; dark when healthy.
//!
//! Grounding: people reliably decode only a handful of single-LED behaviors
//! and bring learned conventions to them (Harrison et al., "Unlocking the
//! Expressivity of Point Lights", CHI 2012); even mass-market light languages
//! are correctly identified only ~37% of the time (Kunchay & Abdullah, CUI
//! 2021). So: ONE meaning per signal, every state distinct in hue AND rhythm
//! AND brightness (redundant coding reads peripherally, at night, and for
//! color-blind viewers), and the healthy common case is DARK (calm-technology:
//! light means something changed). Hue meanings follow IEC 60073 indicator
//! conventions where one exists (amber = abnormal, red reserved for fault,
//! blue = informational, green = OK).
//!
//! Vocabulary (priority order; magenta means locate and nothing else):
//!   locate     magenta  hard 1.5 Hz blink  bright (fixed)  operator find-me
//!   boot       rainbow  3 s ignition bloom scaled          once per boot
//!   update-ok  green    triple blink ~2.4s scaled          first boot after OTA
//!   off        --       dark               0               {"led":"off"} (locate still wins)
//!   ota        blue     steady             dim             download in progress
//!   bus down   amber    slow breath        dim             MQTT unreachable > 120 s
//!   healthy    --       dark               0               the default ("auto")
//!
//! Diagnostic pages (metabolism / link / version, or the legacy rotating
//! carousel with the firefly wink) render only when pinned via
//! {"led":"metabolism"|"link"|"version"|"carousel"}; they are operator tools,
//! not ambient defaults. {"led_max":0..255} scales every signal except locate
//! at runtime (night use: 0 = silence unless an operator asks the board to
//! identify itself).
//!
//! Implementation invariant: every animation phase is a dt-accumulator wrapped
//! per cycle, never wall-time division. f32 wall-clock seconds lose sub-tick
//! resolution within days of uptime, and t/period scrambles phase whenever the
//! period changes — both render as erratic flicker on long-uptime boards.
//!
//! Rendering is f32, temporally dithered to 8-bit at 50 Hz (the WS2812's 8
//! bits give ~8 visible steps at whisper brightness; error diffusion recovers
//! the sub-LSB levels).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::{Output, OutputPin, PinDriver};
use esp_idf_svc::hal::peripheral::Peripheral;
use esp_idf_svc::hal::rmt::config::TransmitConfig;
use esp_idf_svc::hal::rmt::{FixedLengthSignal, PinState, Pulse, RmtChannel, TxRmtDriver};
use log::warn;

use crate::Stats;

const TICK_MS: u64 = 20; // 50 Hz: smooth + dithers, bounded soft-float load
const FLASH_TAU: f32 = 0.13; // firefly flash decay time constant (s)
const BOOT_BLOOM_S: f32 = 3.0;
const BOOT_REVS: f32 = 6.0; // rainbow revolutions during boot
const MQTT_FAULT_AFTER_S: u64 = 120;
const UPDATE_BLINKS: u32 = 3; // green "update applied" blinks after an OTA boot
const UPDATE_BLINK_S: f32 = 0.8; // per on+off cycle
const LOCATE_HZ: f32 = 1.5;
const LOCATE_DUTY: f32 = 0.35;
const LOCATE_VAL: f32 = 48.0; // fixed: locate must be findable even at night cap 0

pub const MODE_AUTO: u8 = 0; // dark default + event overlays
pub const MODE_METABOLISM: u8 = 1;
pub const MODE_LINK: u8 = 2;
pub const MODE_VERSION: u8 = 3;
pub const MODE_OFF: u8 = 4; // dark even for events; locate still wins
pub const MODE_CAROUSEL: u8 = 5; // legacy rotating pages + firefly wink

/// What the LED is rendering right now; health reports it so "why is it that
/// color" is answerable from telemetry instead of a live broker probe.
pub const STATE_NAMES: [&str; 11] = [
    "dark",
    "locate",
    "boot",
    "updated",
    "off",
    "ota",
    "fault",
    "metabolism",
    "link",
    "version",
    "carousel",
];
const ST_DARK: u8 = 0;
const ST_LOCATE: u8 = 1;
const ST_BOOT: u8 = 2;
const ST_UPDATED: u8 = 3;
const ST_OFF: u8 = 4;
const ST_OTA: u8 = 5;
const ST_FAULT: u8 = 6;
const ST_METABOLISM: u8 = 7;
const ST_LINK: u8 = 8;
const ST_VERSION: u8 = 9;
const ST_CAROUSEL: u8 = 10;

const PAGE_SECS: f32 = 6.0;
const FADE_SECS: f32 = 1.0;

type Rgb = (f32, f32, f32); // 0..255 per channel, full resolution

pub fn default_max() -> u32 {
    option_env!("LED_MAX_VAL")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
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
    updated: bool,
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
        .spawn(move || led_loop(driver, stats, updated));
}

pub fn spawn_xiao_user_led(
    pin: impl Peripheral<P = impl OutputPin> + 'static,
    stats: Arc<Stats>,
    updated: bool,
) {
    if option_env!("LED_OFF") == Some("1") {
        return;
    }
    let driver = match PinDriver::output(pin) {
        Ok(d) => d,
        Err(e) => {
            warn!("LED disabled (GPIO init failed: {:?})", e);
            return;
        }
    };
    let _ = thread::Builder::new()
        .name("led".into())
        .stack_size(4096)
        .spawn(move || led_loop(driver, stats, updated));
}

trait LedOutput {
    fn write(&mut self, rgb: Rgb, dith: &mut [f32; 3]) -> anyhow::Result<()>;
}

impl LedOutput for TxRmtDriver<'static> {
    fn write(&mut self, rgb: Rgb, dith: &mut [f32; 3]) -> anyhow::Result<()> {
        write_dithered(self, rgb, dith)
    }
}

impl<T: OutputPin> LedOutput for PinDriver<'static, T, Output> {
    fn write(&mut self, rgb: Rgb, _dith: &mut [f32; 3]) -> anyhow::Result<()> {
        let lit = rgb.0.max(rgb.1).max(rgb.2) >= 1.0;
        if lit {
            self.set_high()?;
        } else {
            self.set_low()?;
        }
        Ok(())
    }
}

/// Per-cycle phase accumulators (all wrap to stay small: no f32 erosion).
struct Phases {
    boot: f32,   // 0..1 once over BOOT_BLOOM_S, then pegged
    locate: f32, // locate blink cycles
    breath: f32, // metabolism breath cycles
    pulse: f32,  // version double-pulse cycles (4 s)
    amber: f32,  // fault breath cycles (3 s)
    wheel: f32,  // carousel position in seconds, wrapped per full rotation
    update: f32, // seconds into the green update-ok blinks
}

fn led_loop(mut tx: impl LedOutput, stats: Arc<Stats>, updated: bool) {
    let ver_hue = version_hue();
    let mut last_tick = Instant::now();
    let mut last_connected = Instant::now();

    let mut ph = Phases {
        boot: 0.0,
        locate: 0.0,
        breath: 0.0,
        pulse: 0.0,
        amber: 0.0,
        wheel: 0.0,
        update: 0.0,
    };
    let mut update_pending = updated;

    let mut rate: f32 = 0.0;
    let mut last_adverts = stats.adverts_seen.load(Ordering::Relaxed);
    let mut last_rate_t = Instant::now();
    let mut last_fire = stats.fire.load(Ordering::Relaxed);
    let mut flash: f32 = 0.0;
    let mut dith = [0.0f32; 3]; // temporal-dither error accumulators

    loop {
        let now = Instant::now();
        let dt = last_tick.elapsed().as_secs_f32();
        last_tick = now;
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
        flash *= (-dt / FLASH_TAU).exp();

        // Advance all phases by measured dt; wrap per cycle.
        ph.boot = (ph.boot + dt / BOOT_BLOOM_S).min(1.0);
        ph.locate = (ph.locate + dt * LOCATE_HZ).fract();
        let busy = (rate / 30.0).clamp(0.0, 1.0);
        let breath_period = 4.0 - busy * 2.4;
        ph.breath = (ph.breath + dt / breath_period).fract();
        ph.pulse = (ph.pulse + dt / 4.0).fract();
        ph.amber = (ph.amber + dt / 3.0).fract();
        let cycle = PAGE_SECS * 3.0;
        ph.wheel = (ph.wheel + dt) % cycle;
        if update_pending && ph.boot >= 1.0 {
            ph.update += dt;
            if ph.update >= UPDATE_BLINKS as f32 * UPDATE_BLINK_S {
                update_pending = false;
            }
        }

        // Runtime brightness ceiling; scales everything except locate.
        let cap = (stats.led_max.load(Ordering::Relaxed).min(255)) as f32;
        let scale = cap / 24.0; // legacy magnitudes were tuned against cap 24

        let mode = stats.led_mode.load(Ordering::Relaxed);
        let ctx = PageCtx {
            breath: ph.breath,
            pulse: ph.pulse,
            cap,
            ver_hue,
            busy,
            flash,
            rssi: stats.wifi_rssi.load(Ordering::Relaxed),
        };

        let (state, rgb): (u8, Rgb) = if stats.locate.load(Ordering::Relaxed) {
            // Operator find-me: outranks everything, including "off" and a
            // night cap of 0 — it is the one explicitly requested signal.
            let on = ph.locate < LOCATE_DUTY;
            (
                ST_LOCATE,
                if on {
                    (LOCATE_VAL, 0.0, LOCATE_VAL)
                } else {
                    (0.0, 0.0, 0.0)
                },
            )
        } else if ph.boot < 1.0 {
            // Boot: an exponential "ignition" — accelerating hue spin, spark
            // envelope. Deliberately unique; the one loud "I (re)started".
            let p = ph.boot;
            let warp = p * p * p;
            let hue = (warp * 360.0 * BOOT_REVS) % 360.0;
            let env = (1.0 - (-p * 10.0).exp()) * (-p * 2.2).exp();
            (ST_BOOT, hsv(hue, 1.0, env * 50.0 * scale))
        } else if update_pending {
            // First boot after an OTA install: green = "update applied OK"
            // (replaces the ambient version-hue wave, which hashed into
            // arbitrary hues and collided with locate's magenta on 0.15.0).
            let on = (ph.update / UPDATE_BLINK_S).fract() < 0.5;
            (
                ST_UPDATED,
                if on {
                    hsv(120.0, 0.9, 24.0 * scale)
                } else {
                    (0.0, 0.0, 0.0)
                },
            )
        } else if mode == MODE_OFF {
            (ST_OFF, (0.0, 0.0, 0.0))
        } else if crate::OTA_ACTIVE.load(Ordering::Relaxed) {
            // Blue steady: informational, rare, brief (download in flight).
            (ST_OTA, hsv(225.0, 0.9, 10.0 * scale))
        } else if last_connected.elapsed().as_secs() > MQTT_FAULT_AFTER_S {
            // Amber breath: abnormal-condition convention; board is alive but
            // the bus is unreachable.
            let v = sin01(ph.amber) * 24.0 * scale;
            (ST_FAULT, (v, v * 0.4, 0.0))
        } else {
            match mode {
                MODE_METABOLISM => (ST_METABOLISM, page(Page::Metabolism, &ctx)),
                MODE_LINK => (ST_LINK, page(Page::Link, &ctx)),
                MODE_VERSION => (ST_VERSION, page(Page::Version, &ctx)),
                MODE_CAROUSEL => (ST_CAROUSEL, carousel(ph.wheel, &ctx)),
                _ => (ST_DARK, (0.0, 0.0, 0.0)), // healthy = silent
            }
        };

        // Telemetry mirrors: rendered colour (ground truth) + which state
        // produced it (the "why is it that colour" answer).
        let pack = ((rgb.0 as u32).min(255) << 16)
            | ((rgb.1 as u32).min(255) << 8)
            | (rgb.2 as u32).min(255);
        stats.led_rgb.store(pack, Ordering::Relaxed);
        stats.led_state.store(state, Ordering::Relaxed);

        if let Err(e) = tx.write(rgb, &mut dith) {
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
    breath: f32, // metabolism breath phase 0..1
    pulse: f32,  // version double-pulse phase 0..1 (4 s cycle)
    cap: f32,
    ver_hue: f32,
    busy: f32,
    flash: f32,
    rssi: i32,
}

/// Legacy rotating diagnostic carousel (pin via {"led":"carousel"}). The
/// firefly wink renders here and on the pinned metabolism page only.
fn carousel(wheel: f32, c: &PageCtx) -> Rgb {
    const PAGES: [Page; 3] = [Page::Metabolism, Page::Link, Page::Version];
    let idx = (wheel / PAGE_SECS) as usize % PAGES.len();
    let into = wheel - idx as f32 * PAGE_SECS;
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
            let act_hue = 130.0 + c.busy * 75.0;
            let hue = lerp_hue(act_hue, c.ver_hue, c.flash);
            let base = 2.0 + sin01(c.breath) * (c.cap * 0.35);
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
            let ph = c.pulse * 4.0; // seconds into the 4 s cycle
            let pulse = bump((ph / 0.5).min(1.0))
                + if ph > 0.7 && ph < 1.2 {
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
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
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
    if p < 0.5 {
        smootherstep(p * 2.0)
    } else {
        smootherstep((1.0 - p) * 2.0)
    }
}

/// HSV (h deg, s 0..1, v scaled to 0..255 here) -> f32 RGB 0..255.
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
    let extra = if *acc >= 1.0 {
        *acc -= 1.0;
        1.0
    } else {
        0.0
    };
    (base + extra) as u32
}

fn write_dithered(
    tx: &mut TxRmtDriver<'static>,
    rgb: Rgb,
    acc: &mut [f32; 3],
) -> anyhow::Result<()> {
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
