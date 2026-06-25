//! ESP32-C6 BLE vantage node: passive BLE scan -> MQTT, WiFi STA,
//! EnergyStatus JSON over serial, HTTP(S) OTA updates.
//!
//! Build with:
//!   WIFI_SSID=MyNetwork WIFI_PASS=secret MQTT_HOST=192.168.1.50 \
//!   OTA_URL=http://192.168.1.100:8080/firmware.bin \
//!   cargo build --release
//!
//! Optional build env: MQTT_PORT (1883), MQTT_USER, MQTT_PASS, BOARD_ID
//! (default "hypha-" + last two STA MAC bytes), BLE_ACTIVE=1 (active scan),
//! POWER_SOURCE.
//!
//! OTA requires a signed manifest next to the image. Set OTA_PUBKEY_HEX or
//! OTA_PUBKEY_PATH at build time; the device fetches `<OTA_URL>.manifest.json`,
//! verifies its Ed25519 signature, then streams and hashes the image before
//! committing it. HTTPS is still recommended for transport privacy and server
//! authentication.
//!
//! Flash once via USB with the dual-slot OTA table (required for EspOta):
//!   cargo espflash flash --release --partition-table partitions_ota.csv --monitor
//! Run OTA server: python -m http.server in the dir holding firmware.bin.
//! Device checks for updates every 5 min and installs over WiFi.

mod ble;
mod firefly;
mod led;
mod mqtt;
pub(crate) mod ota_security;
mod placement;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use embedded_svc::{
    http::client::Client as HttpClient,
    http::Method,
    io::Write,
    utils::io,
    wifi::{AuthMethod, ClientConfiguration, Configuration},
};

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi, WifiDeviceId};
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    nvs::{EspDefaultNvsPartition, EspNvs},
};

use log::{error, info};
use sha2::{Digest, Sha256};

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASS");
// match, not unwrap_or: Option::unwrap_or is not const-stable (same idiom as MQTT_PORT)
const OTA_URL: &str = match option_env!("OTA_URL") {
    Some(u) => u,
    None => "http://192.168.4.1:8080/firmware.bin",
};

const OTA_CHECK_INTERVAL_SECS: u64 = 300; // 5 min
const CHUNK_SIZE: usize = 4096;
const HEALTH_INTERVAL_SECS: u64 = 60;
const OTA_MANIFEST_MAX_BYTES: usize = 1024;

/// Shared counters for the retained health topic (+ LED state inputs).
#[derive(Default)]
pub struct Stats {
    pub adverts_seen: AtomicU32,
    pub scan_windows: AtomicU32,
    pub mqtt_connects: AtomicU32,
    pub mqtt_connected: AtomicBool,
    /// Operator-toggled locate blink (hypha/<board>/cmd {"locate":true|false}).
    pub locate: AtomicBool,
    /// Advert-batch publish successes; kept for compatibility with earlier health payloads.
    pub publishes: AtomicU32,
    /// MQTT publish outcomes by channel. These make asymmetric fleet states
    /// visible after recovery: a board can still BLE-advertise while its own
    /// health/BLE MQTT publishes are failing.
    pub pulse_tx_ok: AtomicU32,
    pub pulse_tx_fail: AtomicU32,
    pub ble_tx_ok: AtomicU32,
    pub ble_tx_fail: AtomicU32,
    pub health_tx_ok: AtomicU32,
    pub health_tx_fail: AtomicU32,
    /// Last WiFi STA RSSI (dBm), refreshed each loop for the LED Link page.
    pub wifi_rssi: AtomicI32,
    /// LED carousel mode (led::MODE_*), set from the cmd topic.
    pub led_mode: AtomicU8,
    /// Firefly fires (heartbeat); the LED flashes on each increment (visible
    /// only on the pinned metabolism/carousel diagnostic pages).
    pub fire: AtomicU32,
    /// Peer firefly pulses heard on hypha/sync/pulse; main couples on each.
    pub peer_pulses: AtomicU32,
    /// Runtime LED brightness ceiling 0..255 (cmd/config {"led_max":N}); scales
    /// every signal except locate. 0 = silent board (night use). Initialized
    /// from the LED_MAX_VAL build env in main.
    pub led_max: AtomicU32,
    /// Which vocabulary state the LED is rendering (led::STATE_NAMES index);
    /// health reports it so "why is it that colour" is a telemetry read.
    pub led_state: AtomicU8,
    /// Bumped per applied cmd/config; main publishes an ack event on change
    /// (the mqtt callback can't publish from inside its own client's task).
    pub cmd_seq: AtomicU32,
    /// Cmds/configs that matched no known key or value; rising = someone is
    /// sending commands this firmware doesn't understand.
    pub cmd_ignored: AtomicU32,
    /// Failed STA RSSI reads (wifi_rssi then holds the last-known value).
    pub rssi_err: AtomicU32,
    /// Last rendered LED colour (packed 0xRRGGBB); health reports it so the
    /// actual hue is visible in telemetry, not just reconstructed.
    pub led_rgb: AtomicU32,
    /// Worst main-loop period (ms) seen this health window; health reports it
    /// and main resets it per window. This is the single number that makes the
    /// single-core starvation bug (loop stalled ~9s under WiFi/BLE load instead
    /// of ~50ms) visible at a glance instead of inferred from cadence drift.
    pub loop_max_ms: AtomicU32,
    /// Last OTA decision state. Health reports this directly so "why didn't it
    /// update?" is a retained telemetry read, not a serial-log guess.
    pub ota_state: AtomicU8,
    /// OTA polls attempted since boot.
    pub ota_checks: AtomicU32,
    /// OTA checks or downloads that ended in an error after a signed manifest
    /// was expected or accepted.
    pub ota_failures: AtomicU32,
    /// Boot-time WiFi fingerprint verdict. It says whether this board's AP
    /// environment changed since the previous boot; infra maps evidence to room.
    pub placement_state: AtomicU8,
    pub placement_aps: AtomicU32,
    pub placement_baseline_aps: AtomicU32,
    pub placement_common: AtomicU32,
    pub placement_shifted: AtomicU32,
    pub placement_jaccard_milli: AtomicU32,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();

    // Mark current slot valid (for rollback if next OTA fails)
    {
        let mut ota = EspOta::new().expect("OTA init");
        if let Err(e) = ota.mark_running_slot_valid() {
            info!("mark_running_slot_valid: {:?} (ok if first boot)", e);
        }
    }

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // LED first: the boot bloom should run from power-on, before WiFi is up.
    let boot_time = std::time::Instant::now();
    let stats = Arc::new(Stats::default());
    stats.led_max.store(led::default_max(), Ordering::Relaxed);
    // sw-reset = the reset an OTA install ends with: show the green
    // "update applied" blinks once after the bloom.
    let updated = mqtt::reset_reason() == "sw-reset";
    match option_env!("LED_BACKEND") {
        Some("ws2812") => led::spawn(
            peripherals.rmt.channel0,
            peripherals.pins.gpio8,
            stats.clone(),
            updated,
        ),
        _ => led::spawn_xiao_user_led(peripherals.pins.gpio15, stats.clone(), updated),
    }

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs.clone()))?,
        sys_loop,
    )?;

    connect_wifi(&mut wifi)?;
    let wifi_ms = boot_time.elapsed().as_millis();
    observe_placement(&mut wifi, nvs.clone(), &stats);

    let mac = wifi.wifi().get_mac(WifiDeviceId::Sta)?;
    let source_id = format!("esp-c6-{:02x}{:02x}", mac[4], mac[5]);
    let board_id = option_env!("BOARD_ID")
        .map(String::from)
        .unwrap_or_else(|| format!("hypha-{:02x}{:02x}", mac[4], mac[5]));
    // Distinguishes post-reboot publishes so a downstream consumer can reset seq.
    let boot_id = format!("{:08x}", unsafe { esp_idf_svc::sys::esp_random() });

    // BLE scan starts after WiFi is up; ESP-IDF coex arbitrates the shared
    // radio from here on (scan windows yield to WiFi).
    let adverts: ble::AdvertMap = Arc::new(Mutex::new(HashMap::new()));
    ble::spawn_scan_thread(adverts.clone(), stats.clone(), board_id.clone())?;
    let mut mqtt = mqtt::connect(&board_id, stats.clone())?;
    let mut seq: u32 = 0;
    // Subscribe to the cmd topic once per connection generation (the esp-idf
    // client drops subscriptions on reconnect; clean session).
    let mut subscribed_gen: u32 = 0;
    // Ack each applied cmd/config once (retried next tick on publish failure).
    let mut acked_cmd_seq: u32 = 0;

    let mut last_health: Option<std::time::Instant> = None;
    let mut boot_announced = false;
    let mut energy_score: f32 = 0.85;
    let mut high = true;
    let mut last_ota_check = std::time::Instant::now();

    // Check for OTA once soon after boot (so you don't wait 5 min for first update).
    if let Err(e) = try_ota_update(&stats) {
        info!(
            "OTA check at boot: {:?} (will retry every {} min)",
            e,
            OTA_CHECK_INTERVAL_SECS / 60
        );
    }

    let power_extra = option_env!("POWER_SOURCE")
        .map(|s| format!(",\"power_source\":\"{}\"", s))
        .unwrap_or_default();

    // Cadences are driven by MEASURED elapsed time, never an assumed tick
    // length: under WiFi/BLE load the loop iterates irregularly (observed ~4x
    // slow), so a fixed-dt firefly ran at ~8s instead of 2s. Measured dt keeps
    // the oscillator and the advert window real-time-correct despite jitter.
    const TICK_MS: u64 = 50; // shorter sleep -> lower firefly-pulse jitter
    let mut osc = firefly::Firefly::new(2.0); // 2s heartbeat
    let mut last_peer = stats.peer_pulses.load(Ordering::Relaxed);
    let mut last_tick = std::time::Instant::now();
    let mut last_advert = std::time::Instant::now();

    loop {
        let dt = last_tick.elapsed().as_secs_f32();
        last_tick = std::time::Instant::now();
        // Record the worst loop period this health window (starvation telemetry).
        stats
            .loop_max_ms
            .fetch_max((dt * 1000.0) as u32, Ordering::Relaxed);

        // --- firefly: couple on peer pulses, advance by REAL dt, emit on fire ---
        let peer = stats.peer_pulses.load(Ordering::Relaxed);
        let mut fired = false;
        while last_peer != peer {
            last_peer = last_peer.wrapping_add(1);
            if osc.couple() {
                fired = true;
            }
        }
        if osc.advance(dt) {
            fired = true;
        }
        if fired {
            stats.fire.fetch_add(1, Ordering::Relaxed); // drives the LED heartbeat
            if let Err(e) = mqtt::publish_pulse(&mut mqtt, &board_id) {
                stats.pulse_tx_fail.fetch_add(1, Ordering::Relaxed);
                error!("{:?}", e);
            } else {
                stats.pulse_tx_ok.fetch_add(1, Ordering::Relaxed);
            }
        }

        // (Re)subscribe (cmd + sync) on each new connection generation.
        let gen = stats.mqtt_connects.load(Ordering::Relaxed);
        if gen != subscribed_gen && stats.mqtt_connected.load(Ordering::Relaxed) {
            match mqtt::subscribe_cmd(&mut mqtt, &board_id) {
                Ok(()) => subscribed_gen = gen,
                Err(e) => error!("{:?}", e),
            }
        }

        // Announce the boot once the bus is up (retained; lets watch see boots).
        if !boot_announced && stats.mqtt_connected.load(Ordering::Relaxed) {
            if mqtt::publish_boot(&mut mqtt, &board_id, &boot_id, wifi_ms, &stats).is_ok() {
                boot_announced = true;
            }
        }

        // Closed-loop ack for applied cmds/configs (see Stats::cmd_seq).
        let cseq = stats.cmd_seq.load(Ordering::Relaxed);
        if cseq != acked_cmd_seq && stats.mqtt_connected.load(Ordering::Relaxed) {
            if mqtt::publish_cmd_ack(&mut mqtt, &board_id, &stats).is_ok() {
                acked_cmd_seq = cseq;
            }
        }

        if last_advert.elapsed() >= Duration::from_secs(2) {
            last_advert = std::time::Instant::now();
            // EnergyStatus serial line (host bridge) + mock energy toggle
            println!(
                r#"{{"source_id":"{}","energy_score":{:.2}{}}}"#,
                source_id, energy_score, power_extra
            );
            energy_score = if high { 0.85 } else { 0.55 };
            high = !high;

            match mqtt::sta_rssi() {
                Some(r) => stats.wifi_rssi.store(r as i32, Ordering::Relaxed),
                None => {
                    stats.rssi_err.fetch_add(1, Ordering::Relaxed);
                }
            }

            let batch: Vec<_> = {
                let mut m = adverts.lock().unwrap();
                m.drain().collect()
            };
            stats.scan_windows.fetch_add(1, Ordering::Relaxed);
            seq = seq.wrapping_add(1);
            if let Err(e) = mqtt::publish_adverts(&mut mqtt, &board_id, &boot_id, seq, batch) {
                stats.ble_tx_fail.fetch_add(1, Ordering::Relaxed);
                error!("{:?}", e);
            } else {
                stats.publishes.fetch_add(1, Ordering::Relaxed);
                stats.ble_tx_ok.fetch_add(1, Ordering::Relaxed);
            }

            if last_health.map_or(true, |t| {
                t.elapsed() >= Duration::from_secs(HEALTH_INTERVAL_SECS)
            }) {
                last_health = Some(std::time::Instant::now());
                if let Err(e) = mqtt::publish_health(
                    &mut mqtt,
                    &board_id,
                    &boot_id,
                    boot_time.elapsed().as_secs(),
                    &stats,
                ) {
                    stats.health_tx_fail.fetch_add(1, Ordering::Relaxed);
                    error!("{:?}", e);
                } else {
                    stats.health_tx_ok.fetch_add(1, Ordering::Relaxed);
                    // Reset the per-window loop-stall high-water mark after a
                    // successful report. Failed reports keep the evidence until
                    // the next retained health publish.
                    stats.loop_max_ms.store(0, Ordering::Relaxed);
                }
            }

            if last_ota_check.elapsed() >= Duration::from_secs(OTA_CHECK_INTERVAL_SECS) {
                last_ota_check = std::time::Instant::now();
                if let Err(e) = try_ota_update(&stats) {
                    error!("OTA check failed: {:?}", e);
                }
            }
        }

        thread::sleep(Duration::from_millis(TICK_MS));
    }
}

const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Signals the BLE scan thread to yield the shared 2.4GHz radio during an OTA
/// download. Without this the continuous BLE scan starves the HTTP transfer to
/// ~64KB/48s (a 1.5MB image would take ~18 min and never finish) -- the C6
/// single-front-end coex constraint biting the OTA path.
pub static OTA_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Resets OTA_ACTIVE (resumes BLE) when the download returns by any path.
struct RadioYield;
impl Drop for RadioYield {
    fn drop(&mut self) {
        OTA_ACTIVE.store(false, Ordering::Relaxed);
    }
}

fn try_ota_update(stats: &Stats) -> anyhow::Result<()> {
    stats.ota_checks.fetch_add(1, Ordering::Relaxed);
    info!("Checking OTA at {}", OTA_URL);

    let Some(pubkey_hex) = option_env!("OTA_PUBKEY_HEX") else {
        stats
            .ota_state
            .store(ota_security::OTA_DISABLED, Ordering::Relaxed);
        info!("OTA: no OTA_PUBKEY_HEX embedded, skipping unsigned update path");
        return Ok(());
    };
    let pubkey = match ota_security::decode_pubkey_hex(pubkey_hex) {
        Some(pubkey) => pubkey,
        None => {
            stats
                .ota_state
                .store(ota_security::OTA_BAD_KEY, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            return Err(anyhow::anyhow!(
                "OTA_PUBKEY_HEX must be a 32-byte Ed25519 pubkey"
            ));
        }
    };

    let manifest_url = ota_security::manifest_url_for(OTA_URL);
    let manifest_bytes = match fetch_url_limited(&manifest_url, OTA_MANIFEST_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(e) => {
            stats
                .ota_state
                .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }
    };
    if manifest_bytes.is_empty() {
        stats
            .ota_state
            .store(ota_security::OTA_NO_MANIFEST, Ordering::Relaxed);
        info!("OTA: no signed manifest at {}, skipping", manifest_url);
        return Ok(());
    }
    let manifest = match ota_security::verify_signed_manifest(&manifest_bytes, &pubkey) {
        Some(manifest) => manifest,
        None => {
            stats
                .ota_state
                .store(ota_security::OTA_BAD_MANIFEST, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            return Err(anyhow::anyhow!("OTA signed manifest verification failed"));
        }
    };

    // Only update to a STRICTLY NEWER signed version. Exact-match skips, and an
    // older staged image cannot trigger a downgrade loop.
    if ota_security::is_strictly_newer(&manifest.version, FW_VERSION) {
        info!(
            "OTA: signed staged {} > running {}, updating",
            manifest.version, FW_VERSION
        );
    } else {
        stats
            .ota_state
            .store(ota_security::OTA_NOT_NEWER, Ordering::Relaxed);
        info!(
            "OTA: signed staged {} not newer than running {}, skipping",
            manifest.version, FW_VERSION
        );
        return Ok(());
    }

    // Yield the radio to the download (BLE scan pauses until this returns).
    stats
        .ota_state
        .store(ota_security::OTA_DOWNLOADING, Ordering::Relaxed);
    OTA_ACTIVE.store(true, Ordering::Relaxed);
    let _radio_yield = RadioYield;

    let http_config = if OTA_URL.starts_with("https://") {
        #[cfg(esp_idf_mbedtls_certificate_bundle)]
        {
            HttpConfig {
                crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
                use_global_ca_store: true,
                ..Default::default()
            }
        }
        #[cfg(not(esp_idf_mbedtls_certificate_bundle))]
        {
            error!("HTTPS OTA requires CONFIG_MBEDTLS_CERTIFICATE_BUNDLE (sdkconfig.defaults)");
            stats
                .ota_state
                .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            return Err(anyhow::anyhow!(
                "HTTPS OTA not available: certificate bundle not enabled"
            ));
        }
    } else {
        HttpConfig::default()
    };

    let connection = EspHttpConnection::new(&http_config).map_err(|e| {
        stats
            .ota_state
            .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        anyhow::anyhow!("OTA image HTTP connection: {:?}", e)
    })?;
    let mut client = HttpClient::wrap(connection);
    let request = client.request(Method::Get, OTA_URL, &[]).map_err(|e| {
        stats
            .ota_state
            .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        anyhow::anyhow!("OTA image HTTP request: {:?}", e)
    })?;
    let mut response = request.submit().map_err(|e| {
        stats
            .ota_state
            .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        anyhow::anyhow!("OTA image HTTP submit: {:?}", e)
    })?;

    let status = response.status();
    if status != 200 {
        stats
            .ota_state
            .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        return Err(anyhow::anyhow!(
            "OTA image GET {} returned {}",
            OTA_URL,
            status
        ));
    }

    info!("OTA: downloading firmware...");

    let mut ota = EspOta::new().map_err(|e| {
        stats
            .ota_state
            .store(ota_security::OTA_APPLY_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        anyhow::anyhow!("OTA init: {:?}", e)
    })?;
    let mut update = ota.initiate_update().map_err(|e| {
        stats
            .ota_state
            .store(ota_security::OTA_APPLY_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        anyhow::anyhow!("OTA begin: {:?}", e)
    })?;

    let mut buf = [0u8; CHUNK_SIZE];
    let mut total: usize = 0;
    let mut hasher = Sha256::new();

    loop {
        let n = io::try_read_full(&mut response, &mut buf).map_err(|e| {
            stats
                .ota_state
                .store(ota_security::OTA_FETCH_ERROR, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            e.0
        })?;
        if n == 0 {
            break;
        }
        update.write_all(&buf[..n]).map_err(|e| {
            stats
                .ota_state
                .store(ota_security::OTA_APPLY_ERROR, Ordering::Relaxed);
            stats.ota_failures.fetch_add(1, Ordering::Relaxed);
            e
        })?;
        hasher.update(&buf[..n]);
        total += n;
        if total % (64 * 1024) < CHUNK_SIZE {
            info!("OTA: {} bytes written", total);
        }
        // Yield the single core so IDLE runs and feeds the task watchdog: this
        // hand-rolled download loop otherwise monopolizes the CPU once BLE is
        // paused (OTA_ACTIVE), starving IDLE0 -> TWDT reboots mid-download.
        thread::sleep(Duration::from_millis(1));
    }

    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != manifest.hash_hex {
        stats
            .ota_state
            .store(ota_security::OTA_HASH_MISMATCH, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        return Err(anyhow::anyhow!(
            "OTA image hash mismatch: got {}, signed {}",
            actual_hash,
            manifest.hash_hex
        ));
    }
    let actual_chunks = hypha_ota::protocol::n_chunks_for_len(total);
    if actual_chunks != manifest.n_chunks {
        stats
            .ota_state
            .store(ota_security::OTA_CHUNK_MISMATCH, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        return Err(anyhow::anyhow!(
            "OTA chunk count mismatch: got {}, signed {}",
            actual_chunks,
            manifest.n_chunks
        ));
    }

    if let Err(e) = update.complete() {
        stats
            .ota_state
            .store(ota_security::OTA_APPLY_ERROR, Ordering::Relaxed);
        stats.ota_failures.fetch_add(1, Ordering::Relaxed);
        return Err(anyhow::anyhow!("OTA complete: {:?}", e));
    }

    stats
        .ota_state
        .store(ota_security::OTA_REBOOTING, Ordering::Relaxed);
    info!("OTA: success, rebooting...");
    thread::sleep(Duration::from_secs(1));
    reset::restart();
}

fn fetch_url_limited(url: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
    let http_config = if url.starts_with("https://") {
        #[cfg(esp_idf_mbedtls_certificate_bundle)]
        {
            HttpConfig {
                crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
                use_global_ca_store: true,
                ..Default::default()
            }
        }
        #[cfg(not(esp_idf_mbedtls_certificate_bundle))]
        {
            error!("HTTPS OTA requires CONFIG_MBEDTLS_CERTIFICATE_BUNDLE (sdkconfig.defaults)");
            return Err(anyhow::anyhow!(
                "HTTPS OTA not available: certificate bundle not enabled"
            ));
        }
    } else {
        HttpConfig::default()
    };

    let mut client = HttpClient::wrap(EspHttpConnection::new(&http_config)?);
    let request = client.request(Method::Get, url, &[])?;
    let mut response = request.submit()?;
    if response.status() == 404 {
        return Ok(Vec::new());
    }
    if response.status() != 200 {
        return Err(anyhow::anyhow!(
            "GET {} returned {}",
            url,
            response.status()
        ));
    }
    let mut buf = vec![0u8; max_bytes + 1];
    let n = io::try_read_full(&mut response, &mut buf).map_err(|e| e.0)?;
    if n > max_bytes {
        return Err(anyhow::anyhow!("GET {} exceeded {} bytes", url, max_bytes));
    }
    buf.truncate(n);
    Ok(buf)
}

fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> anyhow::Result<()> {
    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: PASSWORD.try_into().unwrap(),
        channel: None,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    info!("Wifi started");
    wifi.connect()?;
    info!("Wifi connected");
    wifi.wait_netif_up()?;
    info!("Wifi netif up");
    Ok(())
}

fn observe_placement(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    nvs_partition: EspDefaultNvsPartition,
    stats: &Stats,
) {
    let aps = match wifi.scan() {
        Ok(aps) => aps,
        Err(e) => {
            stats
                .placement_state
                .store(placement::State::ScanError.code(), Ordering::Relaxed);
            error!("placement scan failed: {:?}", e);
            return;
        }
    };
    let now = placement::select_stored(
        aps.into_iter()
            .map(|ap| placement::Ap {
                bssid: placement::pack_bssid(ap.bssid),
                rssi: ap.signal_strength,
            })
            .collect(),
    );
    stats
        .placement_aps
        .store(now.len() as u32, Ordering::Relaxed);

    let mut nvs = match EspNvs::new(nvs_partition, "hypha", true) {
        Ok(nvs) => nvs,
        Err(e) => {
            stats
                .placement_state
                .store(placement::State::StoreError.code(), Ordering::Relaxed);
            error!("placement nvs open failed: {:?}", e);
            return;
        }
    };

    let mut buf = [0u8; placement::STORAGE_MAX_BYTES];
    let previous = match nvs.get_raw("wifi_fp", &mut buf) {
        Ok(Some(bytes)) => placement::decode(bytes),
        Ok(None) => None,
        Err(e) => {
            stats
                .placement_state
                .store(placement::State::StoreError.code(), Ordering::Relaxed);
            error!("placement nvs read failed: {:?}", e);
            None
        }
    };

    if let Some(prev) = previous {
        stats
            .placement_baseline_aps
            .store(prev.len() as u32, Ordering::Relaxed);
        let verdict = placement::evaluate(&prev, &now, placement::DEFAULT);
        stats
            .placement_common
            .store(verdict.common as u32, Ordering::Relaxed);
        stats
            .placement_shifted
            .store(verdict.shifted as u32, Ordering::Relaxed);
        stats.placement_jaccard_milli.store(
            (verdict.jaccard_similarity.clamp(0.0, 1.0) * 1000.0) as u32,
            Ordering::Relaxed,
        );
        stats
            .placement_state
            .store(placement::verdict_state(&verdict).code(), Ordering::Relaxed);
    } else {
        stats
            .placement_state
            .store(placement::State::NoBaseline.code(), Ordering::Relaxed);
        stats.placement_jaccard_milli.store(1000, Ordering::Relaxed);
    }

    if let Err(e) = nvs.set_raw("wifi_fp", &placement::encode(&now)) {
        stats
            .placement_state
            .store(placement::State::StoreError.code(), Ordering::Relaxed);
        error!("placement nvs write failed: {:?}", e);
    }
}
