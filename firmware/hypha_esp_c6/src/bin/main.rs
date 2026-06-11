#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use alloc::format;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterruptControl,
    main,
    time::{Duration, Instant},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::{
    esp_now::BROADCAST_ADDRESS,
    wifi,
};
use esp_backtrace as _;
extern crate alloc;

#[cfg(feature = "mesh_ota")]
use sha2::Digest as _;

#[cfg(feature = "adc")]
use esp_hal::{
    analog::adc::{Adc, AdcCalLine, AdcConfig, Attenuation},
    peripherals::ADC1,
    Blocking,
};
#[cfg(feature = "adc")]
use nb::block;

// Internal temperature sensor — used as energy proxy when no ADC is wired.
// Maps chip temperature to [0, 1]: cooler chip = more "energy" available.
#[cfg(not(feature = "adc"))]
use esp_hal::tsens::{TemperatureSensor, Config as TsensConfig};

#[cfg(feature = "led")]
use esp_hal::{
    rmt::Rmt,
    time::Rate,
};
#[cfg(feature = "led")]
use esp_hal_smartled::{
    color_order::Grb,
    buffer_size,
    RmtSmartLeds,
    Ws2812Timing,
};
#[cfg(feature = "led")]
use smart_leds::{brightness, gamma, hsv::Hsv, hsv::hsv2rgb, RGB8, SmartLedsWrite};

#[cfg(feature = "led-gpio")]
use esp_hal::gpio::Output;

#[cfg(feature = "led")]
use hypha_esp_c6::{
    NodeState, compute_led_steady, compute_breath_period_ms, BREATHING_FLOOR,
    FireflyOscillator, FIREFLY_EPSILON, FIREFLY_REFRACTORY, FIRE_FLASH_MS,
};

// This creates a default app-descriptor required by the esp-idf bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

// --- Timing constants ---
const TX_BUMP_MS: u64 = 200;          // gentle brightness surge on TX
const ERROR_FLASH_MS: u64 = 150;
const ERROR_FLASH_INTERVAL_MS: u64 = 5_000;

const MAX_PEERS: usize = 6;
const PEER_TIMEOUT_MS: u64 = 30_000;
const TX_INTERVAL_MS: u64 = 2_000;
const BOOT_GRACE_MS: u64 = 2_500;

// Energy trend tracking
const ENERGY_TREND_INTERVAL_MS: u64 = 10_000;
const ENERGY_TREND_ALPHA: f32 = 0.3;
// Smoothing for display energy (prevents flicker from 0.85/0.55 toggling)
const ENERGY_SMOOTH_ALPHA: f32 = 0.2;

// LED update throttle (~100 Hz — smooth enough, saves CPU vs ~10 kHz unthrottled)
const LED_UPDATE_INTERVAL_MS: u64 = 10;
// LED telemetry throttle
const LED_TELEMETRY_INTERVAL_MS: u64 = 500;

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let radio_init = esp_radio::init().expect("Failed to initialize radio");
    let (mut wifi_controller, interfaces) =
        wifi::new(&radio_init, peripherals.WIFI, Default::default()).expect("Failed to init WiFi");
    wifi_controller
        .set_mode(wifi::WifiMode::Sta)
        .expect("Failed to set WiFi mode");
    wifi_controller.start().expect("Failed to start WiFi");

    let mut esp_now = interfaces.esp_now;
    let _ = esp_now.set_channel(1);

    let boot_instant = Instant::now();
    let my_mac = wifi::sta_mac();
    let source_id = format!("esp-c6-{:02x}{:02x}", my_mac[4], my_mac[5]);
    println!(
        "EVT:BOOT source_id={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        source_id, my_mac[0], my_mac[1], my_mac[2], my_mac[3], my_mac[4], my_mac[5]
    );
    #[cfg(feature = "ble")]
    println!("BLE coex enabled (TrouBLE stack not yet integrated)");
    #[cfg(feature = "mesh_ota")]
    println!("MESH_OTA ready");

    #[cfg(feature = "adc")]
    let (mut adc_pin, mut adc1) = {
        let mut adc1_config = AdcConfig::new();
        let adc_pin = adc1_config.enable_pin_with_cal::<_, AdcCalLine<ADC1>>(
            peripherals.GPIO2,
            Attenuation::_11dB,
        );
        let adc1 = Adc::<ADC1, Blocking>::new(peripherals.ADC1, adc1_config);
        (adc_pin, adc1)
    };
    #[cfg(feature = "adc")]
    println!("ADC enabled (GPIO2 -> energy_score)");

    // Internal temperature sensor: maps chip temp to energy proxy.
    // Cooler = more energy headroom (0-40C -> 1.0-0.5), hotter = less (40-80C -> 0.5-0.0).
    #[cfg(not(feature = "adc"))]
    let temperature_sensor = TemperatureSensor::new(peripherals.TSENS, TsensConfig::default())
        .expect("TSENS init");
    #[cfg(not(feature = "adc"))]
    println!("TSENS enabled (chip temperature -> energy_score proxy)");

    // --- WS2812 LED init ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut led = {
        let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("RMT init");
        RmtSmartLeds::<{ buffer_size::<RGB8>(1) }, _, RGB8, Grb, Ws2812Timing>::new_with_memsize(
            rmt.channel0,
            peripherals.GPIO8,
            2,
        )
        .expect("WS2812 init")
    };

    // --- Boot sequence: rainbow hue sweep → identity color → fade to black ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    {
        const BOOT_STEPS: u16 = 60;
        let step_ms = 1500u64 / BOOT_STEPS as u64; // ~25ms per step
        for i in 0..=BOOT_STEPS {
            let hue = ((i as u32 * 255) / BOOT_STEPS as u32) as u8;
            let rgb = hsv2rgb(Hsv { hue, sat: 255, val: 140 });
            let _ = led.write(brightness(gamma([rgb].iter().cloned()), 140));
            let t = Instant::now();
            while t.elapsed() < Duration::from_millis(step_ms) {}
        }
        // Settle to identity color (MAC-derived hue)
        let identity_hue = my_mac[5];
        let identity_rgb = hsv2rgb(Hsv { hue: identity_hue, sat: 255, val: 120 });
        let _ = led.write(brightness(gamma([identity_rgb].iter().cloned()), 120));
        let t_id = Instant::now();
        while t_id.elapsed() < Duration::from_millis(500) {}
        let _ = led.write(brightness(gamma([RGB8 { r: 0, g: 0, b: 0 }].iter().cloned()), 255));
    }

    // --- GPIO LED init (fallback, no WS2812) ---
    #[cfg(feature = "led-gpio")]
    let mut led_pin = {
        let mut out = Output::new(peripherals.GPIO8, esp_hal::gpio::Level::Low, Default::default());
        out.set_low();
        out
    };
    #[cfg(feature = "led-gpio")]
    let mut led_blink_until = Instant::now();
    #[cfg(feature = "led-gpio")]
    let mut rx_flash_until_gpio = Instant::now();
    #[cfg(feature = "led-gpio")]
    let mut heartbeat_on_until = Instant::now();
    #[cfg(feature = "led-gpio")]
    let mut next_heartbeat_gpio = Instant::now();

    // --- Shared state ---
    let mut tx_start = Instant::now();
    let mut tx_ok: u32 = 0;
    let mut tx_err: u32 = 0;
    let mut last_rssi: i16 = -128;
    let mut seq: u32 = 0;

    // --- LED state (WS2812 only) ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_energy = 0.5f32;
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut tx_bump_until = Instant::now();
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_error_flash = Instant::now();
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut error_flash_until = Instant::now();
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_led_telemetry = Instant::now();
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_led_update = Instant::now();
    // Mirollo-Strogatz firefly oscillator: replaces the old triangle-wave
    // breathing + ad-hoc phase offset sync. Phase advances linearly, fires at
    // threshold, resets. Peer pulses advance state via concave coupling.
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut firefly_osc = {
        let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        // Initialize phase from MAC so boards start at different positions
        // (makes convergence visible rather than starting in accidental sync)
        osc.set_phase(my_mac[5] as f32 / 255.0);
        osc
    };
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut fire_flash_until = Instant::now();

    // --- Energy trend tracking ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut energy_prev = 0.5f32;
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut energy_delta = 0.0f32;
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_energy_trend_update = Instant::now();

    // --- Activity tracking ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut rx_count_recent: u32 = 0;
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_activity_reset = Instant::now();
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut activity_rate: f32 = 0.0;

    // --- Last RX time for saturation freshness ---
    #[cfg(all(feature = "led", not(feature = "led-gpio")))]
    let mut last_rx_time = Instant::now();

    // Peer tracking
    let mut peer_macs: [[u8; 6]; MAX_PEERS] = [[0; 6]; MAX_PEERS];
    let mut peer_last_seen: [Instant; MAX_PEERS] = [Instant::now(); MAX_PEERS];

    // Mesh OTA state
    #[cfg(feature = "mesh_ota")]
    let mut last_manifest_broadcast = Instant::now();
    #[cfg(feature = "mesh_ota")]
    let mut ota_sender: Option<[u8; 6]> = None;
    #[cfg(feature = "mesh_ota")]
    let mut ota_version: Option<alloc::string::String> = None;
    #[cfg(feature = "mesh_ota")]
    let mut ota_n: u32 = 0;
    #[cfg(feature = "mesh_ota")]
    let mut ota_hash_hex: Option<alloc::string::String> = None;
    #[cfg(feature = "mesh_ota")]
    let mut ota_next_chunk: u32 = 0; // next chunk index to request (stream-to-flash)
    #[cfg(feature = "mesh_ota")]
    let mut ota_hasher: Option<sha2::Sha256> = None; // running hash of received chunks
    #[cfg(feature = "mesh_ota")]
    let mut ota_erased: bool = false;

    loop {
        // --- Mesh OTA sender ---
        #[cfg(feature = "mesh_ota")]
        if hypha_esp_c6::mesh_ota::has_embedded_manifest()
            && last_manifest_broadcast.elapsed() >= Duration::from_millis(30_000)
        {
            last_manifest_broadcast = Instant::now();
            if let Some(manifest) = hypha_esp_c6::mesh_ota::manifest_json_for_broadcast() {
                if let Ok(waiter) = esp_now.send(&BROADCAST_ADDRESS, manifest.as_bytes()) {
                    let _ = waiter.wait();
                }
            }
        }

        // --- Peer pruning with telemetry ---
        for i in 0..MAX_PEERS {
            if peer_macs[i].iter().any(|&b| b != 0)
                && peer_last_seen[i].elapsed() >= Duration::from_millis(PEER_TIMEOUT_MS)
            {
                let mac = peer_macs[i];
                let timeout = peer_last_seen[i].elapsed().as_millis() as u64;
                peer_macs[i] = [0; 6];
                let remaining = peer_macs.iter().filter(|m| m.iter().any(|&b| b != 0)).count();
                println!(
                    "EVT:PEER_DROP mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} count={} timeout_ms={}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], remaining, timeout
                );
            }
        }

        // --- TX path (every 2s) ---
        if tx_start.elapsed() >= Duration::from_millis(TX_INTERVAL_MS) {
            let peer_count = peer_macs
                .iter()
                .filter(|m| m.iter().any(|&b| b != 0))
                .count();
            let energy_score = {
                #[cfg(feature = "adc")]
                {
                    let mv = block!(adc1.read_oneshot(&mut adc_pin)).unwrap_or(0) as u32;
                    (mv.min(3300) as f32 / 3300.0).min(1.0)
                }
                #[cfg(not(feature = "adc"))]
                {
                    let celsius = temperature_sensor.get_temperature().to_celsius();
                    hypha_esp_c6::temp_to_energy(celsius)
                }
            };

            #[cfg(all(feature = "led", not(feature = "led-gpio")))]
            {
                // Smooth energy for display (prevents visible flicker from toggling values)
                last_energy = ENERGY_SMOOTH_ALPHA * energy_score + (1.0 - ENERGY_SMOOTH_ALPHA) * last_energy;
                // Update energy trend every 10s
                if last_energy_trend_update.elapsed() >= Duration::from_millis(ENERGY_TREND_INTERVAL_MS) {
                    let raw_delta = energy_score - energy_prev;
                    energy_delta = ENERGY_TREND_ALPHA * raw_delta + (1.0 - ENERGY_TREND_ALPHA) * energy_delta;
                    energy_prev = energy_score;
                    last_energy_trend_update = Instant::now();
                }
                // Update activity rate every 10s
                if last_activity_reset.elapsed() >= Duration::from_millis(10_000) {
                    let total_events = tx_ok.min(5) + rx_count_recent; // cap TX contribution
                    activity_rate = (total_events as f32 / 20.0).clamp(0.0, 1.0);
                    rx_count_recent = 0;
                    last_activity_reset = Instant::now();
                }
            }

            seq += 1;
            let power_extra = option_env!("POWER_SOURCE")
                .map(|s| format!(",\"power_source\":\"{}\"", s))
                .unwrap_or_default();
            let uptime_ms = boot_instant.elapsed().as_millis() as u64;
            let payload = format!(
                r#"{{"source_id":"{}","energy_score":{:.2},"peers":{},"uptime_ms":{},"tx_ok":{},"tx_err":{},"rssi":{},"seq":{}{}}}"#,
                source_id, energy_score, peer_count, uptime_ms, tx_ok, tx_err, last_rssi, seq, power_extra
            );

            // USB serial output (for host bridge)
            println!("{}", payload);

            // Broadcast over ESP-NOW
            match esp_now.send(&BROADCAST_ADDRESS, payload.as_bytes()) {
                Ok(waiter) => {
                    if let Err(e) = waiter.wait() {
                        tx_err += 1;
                        println!("TX_ERR {:?}", e);
                    } else {
                        tx_ok += 1;
                    }
                }
                Err(e) => {
                    tx_err += 1;
                    println!("TX_ERR {:?}", e);
                }
            }

            println!(
                "EVT:TX seq={} ok={} err={} energy={:.2} peers={}",
                seq, tx_ok, tx_err, energy_score, peer_count
            );

            #[cfg(feature = "led-gpio")]
            {
                led_blink_until = Instant::now() + Duration::from_millis(150);
                led_pin.set_high();
            }
            // TX brightness bump (gentle surge in current hue, no color change)
            #[cfg(all(feature = "led", not(feature = "led-gpio")))]
            {
                tx_bump_until = Instant::now() + Duration::from_millis(TX_BUMP_MS);
            }
            tx_start = Instant::now();
        }

        // =====================================================================
        // LED UPDATE (WS2812) — Mirollo-Strogatz firefly oscillator
        //
        // The oscillator phase [0,1) advances linearly. Brightness follows a
        // concave-up curve (phase^2): slow build-up -> accelerating glow ->
        // flash at threshold -> dark -> repeat. Peer pulses advance our state
        // via concave coupling, naturally synchronizing fire events.
        //
        // Overlays: fire flash (peak brightness hold), TX bump (additive),
        // error flash (rare, breaks color). Throttled to ~100 Hz.
        // =====================================================================
        #[cfg(all(feature = "led", not(feature = "led-gpio")))]
        if last_led_update.elapsed() >= Duration::from_millis(LED_UPDATE_INTERVAL_MS)
        {
            let dt = last_led_update.elapsed().as_millis() as u64;
            last_led_update = Instant::now();
            let peer_count = peer_macs.iter().filter(|m| m.iter().any(|&b| b != 0)).count();
            let now = Instant::now();
            let uptime_ms = boot_instant.elapsed().as_millis() as u64;
            let ms_since_rx = last_rx_time.elapsed().as_millis() as u64;

            // Advance the firefly oscillator
            let osc_period = compute_breath_period_ms(activity_rate);
            let fired = firefly_osc.advance(dt, osc_period);
            if fired {
                fire_flash_until = now + Duration::from_millis(FIRE_FLASH_MS);
                println!("EVT:FIRE phase=0 period_ms={}", osc_period);
            }

            // Compute steady-state LED from node telemetry
            let node_state = NodeState {
                peer_count,
                energy_score: last_energy,
                energy_delta,
                tx_ok,
                tx_err,
                last_rssi,
                ms_since_last_rx: ms_since_rx,
                activity_rate,
                uptime_ms,
            };
            let steady = compute_led_steady(&node_state);

            // Firefly brightness: concave-up modulation (0.6 at trough, 1.0 at peak)
            let fire_factor = firefly_osc.brightness_factor();
            let osc_val = ((steady.val as f32) * (0.6 + 0.4 * fire_factor)) as u8;
            let osc_val = osc_val.max(BREATHING_FLOOR);

            // --- Overlays: fire flash > error flash > TX bump ---
            let (final_hue, final_sat, final_val, mode);

            if now < error_flash_until {
                final_hue = 0;
                final_sat = 255;
                final_val = 120;
                mode = "err_flash";
            } else if now < fire_flash_until {
                // Fire flash: peak brightness in current hue (the synchronized moment)
                final_hue = steady.hue;
                final_sat = steady.sat;
                final_val = steady.val; // full brightness, no oscillator reduction
                mode = "fire";
            } else {
                final_hue = steady.hue;
                final_sat = steady.sat;
                let mut val_boost: u16 = 0;
                let mut active_mode = "firefly";

                // TX bump: additive brightness surge (fades over TX_BUMP_MS)
                if now < tx_bump_until {
                    let remaining = (tx_bump_until - now).as_millis() as f32;
                    let progress = remaining / TX_BUMP_MS as f32;
                    val_boost += (20.0 * progress) as u16;
                    active_mode = "tx_bump";
                }

                final_val = (osc_val as u16 + val_boost).min(140) as u8;
                mode = active_mode;
            }

            // Schedule error flash if error rate > 10%
            if tx_ok.saturating_add(tx_err) > 10
                && (tx_err as f32 / tx_ok.saturating_add(tx_err) as f32) > 0.10
                && last_error_flash.elapsed() >= Duration::from_millis(ERROR_FLASH_INTERVAL_MS)
            {
                last_error_flash = Instant::now();
                error_flash_until = now + Duration::from_millis(ERROR_FLASH_MS);
            }

            // Write to LED
            let rgb = hsv2rgb(Hsv { hue: final_hue, sat: final_sat, val: final_val });
            let _ = led.write(brightness(gamma([rgb].iter().cloned()), 255));

            // LED telemetry (throttled)
            if last_led_telemetry.elapsed() >= Duration::from_millis(LED_TELEMETRY_INTERVAL_MS) {
                last_led_telemetry = Instant::now();
                println!(
                    "EVT:LED hue={} sat={} val={} mode={} period_ms={} phase={:.2}",
                    final_hue, final_sat, final_val, mode, osc_period,
                    firefly_osc.phase()
                );
            }
        }

        // --- GPIO LED fallback ---
        #[cfg(feature = "led-gpio")]
        {
            let now = Instant::now();
            if now >= next_heartbeat_gpio {
                next_heartbeat_gpio = now + Duration::from_millis(1000);
                heartbeat_on_until = now + Duration::from_millis(80);
            }
            let on = now <= led_blink_until || now <= rx_flash_until_gpio || now <= heartbeat_on_until;
            if on {
                led_pin.set_high();
            } else {
                led_pin.set_low();
            }
        }

        // =====================================================================
        // RX path — peers are tracked even during boot grace; only LED/sync
        // reactions are suppressed until after the grace period.
        // =====================================================================
        if let Some(frame) = esp_now.receive() {
            if frame.info.src_address != my_mac {
            let src = &frame.info.src_address;
            let had_peers = peer_macs.iter().any(|m| m.iter().any(|&b| b != 0));

            // Peer tracking: add or refresh (always, even during boot grace)
            if let Some(i) = peer_macs.iter().position(|m| m == src) {
                peer_last_seen[i] = Instant::now();
            } else if let Some(i) = peer_macs.iter().position(|m| m.iter().all(|&b| b == 0)) {
                peer_macs[i].copy_from_slice(src);
                peer_last_seen[i] = Instant::now();
                let new_count = peer_macs.iter().filter(|m| m.iter().any(|&b| b != 0)).count();
                println!(
                    "EVT:PEER_ADD mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} count={}",
                    src[0], src[1], src[2], src[3], src[4], src[5], new_count
                );
            } else {
                // All MAX_PEERS slots full — warn so we know if this happens
                println!(
                    "EVT:PEER_OVERFLOW mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} max={}",
                    src[0], src[1], src[2], src[3], src[4], src[5], MAX_PEERS
                );
            }

            let boot_elapsed_ms = boot_instant.elapsed().as_millis();
            if boot_elapsed_ms > BOOT_GRACE_MS {
                // Update RX freshness + activity counter
                #[cfg(all(feature = "led", not(feature = "led-gpio")))]
                {
                    last_rx_time = Instant::now();
                    rx_count_recent += 1;
                }

                // Mirollo-Strogatz firefly coupling: when we hear a peer's beacon,
                // treat it as a "pulse" — advance our oscillator state by epsilon
                // via the concave coupling function. If the pulse pushes us past
                // threshold, we fire immediately (absorption -> instant sync).
                #[cfg(all(feature = "led", not(feature = "led-gpio")))]
                {
                    let absorbed = firefly_osc.receive_pulse();
                    if absorbed {
                        fire_flash_until = Instant::now() + Duration::from_millis(FIRE_FLASH_MS);
                        println!(
                            "EVT:FIRE phase=0 trigger=absorption period_ms={}",
                            compute_breath_period_ms(activity_rate)
                        );
                    } else {
                        println!(
                            "EVT:SYNC phase={:.3} refractory={}",
                            firefly_osc.phase(),
                            firefly_osc.phase() < FIREFLY_REFRACTORY
                        );
                    }
                }

                let have_peers_now = peer_macs.iter().any(|m| m.iter().any(|&b| b != 0));
                if have_peers_now && !had_peers {
                    tx_start = Instant::now();
                }

                // RX flash (GPIO LED)
                #[cfg(feature = "led-gpio")]
                {
                    rx_flash_until_gpio = Instant::now() + Duration::from_millis(120);
                    led_pin.set_high();
                }

                // RSSI extraction
                let rssi_dbm = frame.info.rx_control.rssi as i8 as i16;
                last_rssi = rssi_dbm;

                #[allow(unused_variables)]
                if let Ok(text) = core::str::from_utf8(frame.data()) {
                    // Mesh OTA handling (stream-to-flash)
                    #[cfg(feature = "mesh_ota")]
                    if text.contains("\"ota\"") && text.contains("\"manifest\"") {
                        if let Some((v, n, h, _sig)) =
                            hypha_esp_c6::mesh_ota::verify_manifest_json_embedded_full(frame.data())
                        {
                            println!("OTA_VERIFIED v={} n={}", v, n);
                            if n <= hypha_esp_c6::mesh_ota::MAX_CHUNKS && ota_sender.is_none() {
                                // Compute image size and erase OTA partition
                                let image_len = n * hypha_esp_c6::mesh_ota::CHUNK_SIZE as u32;
                                if hypha_esp_c6::ota_apply::erase_ota_partition(image_len) {
                                    println!("OTA_ERASED {} sectors", (image_len + 4095) / 4096);
                                    ota_sender = Some(frame.info.src_address);
                                    ota_version = Some(v.clone());
                                    ota_n = n;
                                    ota_hash_hex = Some(h);
                                    ota_next_chunk = 0;
                                    ota_hasher = Some(sha2::Sha256::new());
                                    ota_erased = true;
                                    let req = hypha_esp_c6::mesh_ota::build_chunk_request(0);
                                    if let Ok(waiter) = esp_now.send(&frame.info.src_address, req.as_bytes()) {
                                        let _ = waiter.wait();
                                    }
                                } else {
                                    println!("OTA_ERASE_FAILED");
                                }
                            }
                        }
                    }
                    #[cfg(feature = "mesh_ota")]
                    if ota_sender.is_some() && frame.info.src_address == ota_sender.unwrap()
                        && text.contains("\"ota\"") && text.contains("\"chunk\"")
                    {
                        if let Some((idx, chunk_data)) = hypha_esp_c6::mesh_ota::parse_chunk_response(frame.data()) {
                            if idx == ota_next_chunk && ota_next_chunk < ota_n && ota_erased {
                                // Write chunk directly to flash
                                if hypha_esp_c6::ota_apply::write_ota_chunk(idx, &chunk_data) {
                                    // Update running hash
                                    if let Some(ref mut hasher) = ota_hasher {
                                        hasher.update(&chunk_data);
                                    }
                                    ota_next_chunk += 1;
                                    if ota_next_chunk == ota_n {
                                        // All chunks received — verify hash
                                        let hash_ok = if let (Some(hasher), Some(hash_hex)) =
                                            (ota_hasher.take(), &ota_hash_hex)
                                        {
                                            let digest = hasher.finalize();
                                            let expected = hex::decode(hash_hex.as_str());
                                            match expected {
                                                Ok(exp) if exp.len() == 32 => digest.as_slice() == exp.as_slice(),
                                                _ => false,
                                            }
                                        } else {
                                            false
                                        };
                                        if hash_ok {
                                            let total_bytes = ota_n * hypha_esp_c6::mesh_ota::CHUNK_SIZE as u32;
                                            println!("OTA_READY v={} bytes={}", ota_version.as_deref().unwrap_or(""), total_bytes);
                                            hypha_esp_c6::ota_apply::set_boot_ota0_and_reboot();
                                        } else {
                                            println!("OTA_HASH_MISMATCH");
                                        }
                                        // Reset state (only reached if reboot failed or hash mismatch)
                                        ota_sender = None;
                                        ota_version = None;
                                        ota_n = 0;
                                        ota_hash_hex = None;
                                        ota_next_chunk = 0;
                                        ota_hasher = None;
                                        ota_erased = false;
                                    } else {
                                        // Request next chunk
                                        let req = hypha_esp_c6::mesh_ota::build_chunk_request(ota_next_chunk);
                                        if let Ok(waiter) = esp_now.send(&frame.info.src_address, req.as_bytes()) {
                                            let _ = waiter.wait();
                                        }
                                    }
                                } else {
                                    println!("OTA_WRITE_FAILED chunk={}", idx);
                                    // Abort OTA on write failure
                                    ota_sender = None;
                                    ota_n = 0;
                                    ota_erased = false;
                                }
                            }
                        }
                    }
                    #[cfg(feature = "mesh_ota")]
                    if hypha_esp_c6::mesh_ota::has_embedded_manifest() {
                        if let Some(idx) = hypha_esp_c6::mesh_ota::parse_chunk_request(frame.data()) {
                            if let Some(chunk_data) = hypha_esp_c6::mesh_ota::embedded_image_chunk(idx) {
                                let response =
                                    hypha_esp_c6::mesh_ota::build_chunk_response(idx, chunk_data);
                                if let Ok(waiter) = esp_now.send(&frame.info.src_address, response.as_bytes()) {
                                    let _ = waiter.wait();
                                }
                            }
                        }
                    }

                    // RX telemetry event
                    println!(
                        "EVT:RX src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rssi={}",
                        src[0], src[1], src[2], src[3], src[4], src[5], rssi_dbm
                    );
                }
            } // boot_elapsed_ms > BOOT_GRACE_MS
            }
        }

        // Yield CPU: 1ms delay prevents 100% busy-poll. The LED updates at
        // ~100 Hz (10ms interval) and TX fires every 2s, so 1ms idle between
        // iterations wastes negligible latency but saves significant power.
        let t_yield = Instant::now();
        while t_yield.elapsed() < Duration::from_millis(1) {}
    }
}
