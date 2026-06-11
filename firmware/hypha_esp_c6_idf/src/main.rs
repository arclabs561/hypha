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
//! For secure OTA use HTTPS and certificate verification: set OTA_URL to an https:// URL.
//! This crate enables CONFIG_MBEDTLS_CERTIFICATE_BUNDLE via sdkconfig.defaults so the
//! device verifies the server certificate when using https.
//!
//! Flash once via USB with the dual-slot OTA table (required for EspOta):
//!   cargo espflash flash --release --partition-table partitions_ota.csv --monitor
//! Run OTA server: python -m http.server in the dir holding firmware.bin.
//! Device checks for updates every 5 min and installs over WiFi.

mod ble;
mod firefly;
mod led;
mod mqtt;

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
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};

use log::{error, info};

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

/// Shared counters for the retained health topic (+ LED state inputs).
#[derive(Default)]
pub struct Stats {
    pub adverts_seen: AtomicU32,
    pub scan_windows: AtomicU32,
    pub mqtt_connects: AtomicU32,
    pub mqtt_connected: AtomicBool,
    /// Operator-toggled locate blink (hypha/<board>/cmd {"locate":true|false}).
    pub locate: AtomicBool,
    /// Advert-batch publishes; the LED reads increments as the firefly heartbeat.
    pub publishes: AtomicU32,
    /// Last WiFi STA RSSI (dBm), refreshed each loop for the LED Link page.
    pub wifi_rssi: AtomicI32,
    /// LED carousel mode (led::MODE_*), set from the cmd topic.
    pub led_mode: AtomicU8,
    /// Firefly fires (heartbeat); the LED flashes on each increment.
    pub fire: AtomicU32,
    /// Peer firefly pulses heard on hypha/sync/pulse; main couples on each.
    pub peer_pulses: AtomicU32,
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

    // LED first: the locate blink should run from power-on, before WiFi is up.
    let boot_time = std::time::Instant::now();
    let stats = Arc::new(Stats::default());
    led::spawn(peripherals.rmt.channel0, peripherals.pins.gpio8, stats.clone());

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    connect_wifi(&mut wifi)?;
    let wifi_ms = boot_time.elapsed().as_millis();

    let mac = wifi.wifi().get_mac(WifiDeviceId::Sta)?;
    let source_id = format!("esp-c6-{:02x}{:02x}", mac[4], mac[5]);
    let board_id = option_env!("BOARD_ID")
        .map(String::from)
        .unwrap_or_else(|| format!("hypha-{:02x}{:02x}", mac[4], mac[5]));
    // Distinguishes post-reboot publishes so the fusion plane can reset seq.
    let boot_id = format!("{:08x}", unsafe { esp_idf_svc::sys::esp_random() });

    // BLE scan starts after WiFi is up; ESP-IDF coex arbitrates the shared
    // radio from here on (scan windows yield to WiFi).
    let adverts: ble::AdvertMap = Arc::new(Mutex::new(HashMap::new()));
    ble::spawn_scan_thread(adverts.clone(), stats.clone())?;
    let mut mqtt = mqtt::connect(&board_id, stats.clone())?;
    let mut seq: u32 = 0;
    // Subscribe to the cmd topic once per connection generation (the esp-idf
    // client drops subscriptions on reconnect; clean session).
    let mut subscribed_gen: u32 = 0;

    let mut last_health: Option<std::time::Instant> = None;
    let mut boot_announced = false;
    let mut energy_score: f32 = 0.85;
    let mut high = true;
    let mut last_ota_check = std::time::Instant::now();

    // Check for OTA once soon after boot (so you don't wait 5 min for first update).
    if let Err(e) = try_ota_update() {
        info!("OTA check at boot: {:?} (will retry every {} min)", e, OTA_CHECK_INTERVAL_SECS / 60);
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
                error!("{:?}", e);
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
            if mqtt::publish_boot(&mut mqtt, &board_id, &boot_id, wifi_ms).is_ok() {
                boot_announced = true;
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

            stats.wifi_rssi.store(mqtt::sta_rssi() as i32, Ordering::Relaxed);

            let batch: Vec<_> = {
                let mut m = adverts.lock().unwrap();
                m.drain().collect()
            };
            stats.scan_windows.fetch_add(1, Ordering::Relaxed);
            seq = seq.wrapping_add(1);
            if let Err(e) = mqtt::publish_adverts(&mut mqtt, &board_id, &boot_id, seq, batch) {
                error!("{:?}", e);
            } else {
                stats.publishes.fetch_add(1, Ordering::Relaxed);
            }

            if last_health.map_or(true, |t| t.elapsed() >= Duration::from_secs(HEALTH_INTERVAL_SECS))
            {
                last_health = Some(std::time::Instant::now());
                if let Err(e) =
                    mqtt::publish_health(&mut mqtt, &board_id, boot_time.elapsed().as_secs(), &stats)
                {
                    error!("{:?}", e);
                }
            }

            if last_ota_check.elapsed() >= Duration::from_secs(OTA_CHECK_INTERVAL_SECS) {
                last_ota_check = std::time::Instant::now();
                if let Err(e) = try_ota_update() {
                    error!("OTA check failed: {:?}", e);
                }
            }
        }

        thread::sleep(Duration::from_millis(TICK_MS));
    }
}

const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

fn try_ota_update() -> anyhow::Result<()> {
    info!("Checking OTA at {}", OTA_URL);

    // Version gate: without it, any staged image re-installs every poll cycle
    // forever (the pre-0.2.0 loop bug). The stage step writes firmware.bin.version
    // next to the image; missing version file = no update (loop-proof default).
    {
        let mut client = HttpClient::wrap(EspHttpConnection::new(&HttpConfig::default())?);
        let ver_url = format!("{}.version", OTA_URL);
        let request = client.request(Method::Get, &ver_url, &[])?;
        let mut response = request.submit()?;
        if response.status() != 200 {
            info!("OTA: no version file (status {}), skipping", response.status());
            return Ok(());
        }
        let mut vbuf = [0u8; 64];
        let n = io::try_read_full(&mut response, &mut vbuf).map_err(|e| e.0)?;
        // non-UTF8 version file = skip, never update: "" would compare unequal
        // and re-trigger the very download loop this gate exists to prevent
        let staged = match core::str::from_utf8(&vbuf[..n]) {
            Ok(s) => s.trim(),
            Err(e) => {
                info!("OTA: version file not UTF-8 ({}), skipping", e);
                return Ok(());
            }
        };
        if staged == FW_VERSION {
            info!("OTA: up to date ({})", FW_VERSION);
            return Ok(());
        }
        info!("OTA: staged {} != running {}, updating", staged, FW_VERSION);
    }

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
            return Err(anyhow::anyhow!(
                "HTTPS OTA not available: certificate bundle not enabled"
            ));
        }
    } else {
        HttpConfig::default()
    };

    let mut client = HttpClient::wrap(EspHttpConnection::new(&http_config)?);
    let request = client.request(Method::Get, OTA_URL, &[])?;
    let mut response = request.submit()?;

    let status = response.status();
    if status != 200 {
        info!("OTA: no update (status {})", status);
        return Ok(());
    }

    info!("OTA: downloading firmware...");

    let mut ota = EspOta::new().map_err(|e| anyhow::anyhow!("OTA init: {:?}", e))?;
    let mut update = ota
        .initiate_update()
        .map_err(|e| anyhow::anyhow!("OTA begin: {:?}", e))?;

    let mut buf = [0u8; CHUNK_SIZE];
    let mut total: usize = 0;

    loop {
        let n = io::try_read_full(&mut response, &mut buf).map_err(|e| e.0)?;
        if n == 0 {
            break;
        }
        update.write_all(&buf[..n])?;
        total += n;
        if total % (64 * 1024) < CHUNK_SIZE {
            info!("OTA: {} bytes written", total);
        }
    }

    update.complete().map_err(|e| anyhow::anyhow!("OTA complete: {:?}", e))?;

    info!("OTA: success, rebooting...");
    thread::sleep(Duration::from_secs(1));
    reset::restart();
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
