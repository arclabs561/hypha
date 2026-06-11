//! WS2812 status LED (GPIO8 on the C6 devkit) — whisper semantics.
//!
//! Modes (design: dim sign-of-life, brighter = something to say):
//! - Locate: first 15 min after boot, 1 Hz magenta pulse (val 40) so the
//!   operator can identify a freshly powered board across a room.
//! - Idle: slow whisper breath (4 s period, val 0..8, calm cyan-green).
//! - Fault: amber breath (1 s, val 24) while MQTT has been down >2 min.
//! Build with LED_OFF=1 to keep the LED fully dark (room boards).

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

// Short: with the cmd-topic toggle available, the boot window only needs to
// say "I just rebooted", not carry the whole locating workflow.
const LOCATE_WINDOW_S: u64 = 120;
const MQTT_FAULT_AFTER_S: u64 = 120;

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
    let mut last_connected = Instant::now();
    loop {
        let up = boot.elapsed().as_secs();
        let connected = stats.mqtt_connected.load(Ordering::Relaxed);
        if connected {
            last_connected = Instant::now();
        }
        let t_ms = boot.elapsed().as_millis() as u64;

        let (r, g, b) = if stats.locate.load(Ordering::Relaxed) {
            // operator locate toggle: same magenta blink, runs until toggled off
            if t_ms % 1_000 < 200 {
                (40, 0, 40)
            } else {
                (0, 0, 0)
            }
        } else if last_connected.elapsed().as_secs() > MQTT_FAULT_AFTER_S {
            // fault: amber breath, 1 s period
            let v = breath(t_ms, 1_000, 24);
            (v, v / 2, 0)
        } else if up < LOCATE_WINDOW_S {
            // locate: 1 Hz magenta blink (200 ms on)
            if t_ms % 1_000 < 200 {
                (40, 0, 40)
            } else {
                (0, 0, 0)
            }
        } else {
            // idle: whisper breath, 4 s period, val<=8, cyan-green
            let v = breath(t_ms, 4_000, 8);
            (0, v, v / 2)
        };

        if let Err(e) = write_rgb(&mut tx, r, g, b) {
            warn!("LED write failed: {:?}", e);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Triangle-wave brightness in 0..=max over `period_ms`.
fn breath(t_ms: u64, period_ms: u64, max: u32) -> u32 {
    let phase = (t_ms % period_ms) as u32;
    let half = (period_ms / 2) as u32;
    let tri = if phase < half { phase } else { (period_ms as u32) - phase };
    tri * max / half
}

/// WS2812 bit timing over RMT (GRB order, ~350/800ns zero, ~700/600ns one).
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
