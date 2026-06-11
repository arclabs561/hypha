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
mod mqtt;

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
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

/// Shared counters for the retained health topic.
#[derive(Default)]
pub struct Stats {
    pub adverts_seen: AtomicU32,
    pub scan_windows: AtomicU32,
    pub mqtt_connects: AtomicU32,
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

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    connect_wifi(&mut wifi)?;

    let mac = wifi.wifi().get_mac(WifiDeviceId::Sta)?;
    let source_id = format!("esp-c6-{:02x}{:02x}", mac[4], mac[5]);
    let board_id = option_env!("BOARD_ID")
        .map(String::from)
        .unwrap_or_else(|| format!("hypha-{:02x}{:02x}", mac[4], mac[5]));
    // Distinguishes post-reboot publishes so the fusion plane can reset seq.
    let boot_id = format!("{:08x}", unsafe { esp_idf_svc::sys::esp_random() });

    // BLE scan starts after WiFi is up; ESP-IDF coex arbitrates the shared
    // radio from here on (scan windows yield to WiFi).
    let stats = Arc::new(Stats::default());
    let adverts: ble::AdvertMap = Arc::new(Mutex::new(HashMap::new()));
    ble::spawn_scan_thread(adverts.clone(), stats.clone())?;
    let mut mqtt = mqtt::connect(&board_id, stats.clone())?;
    let mut seq: u32 = 0;

    let boot_time = std::time::Instant::now();
    let mut last_health: Option<std::time::Instant> = None;
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

    loop {
        // Print EnergyStatus JSON (host bridge / serial)
        let line = format!(
            r#"{{"source_id":"{}","energy_score":{:.2}{}}}"#,
            source_id, energy_score, power_extra
        );
        println!("{}", line);

        // Toggle mock energy for demo
        energy_score = if high { 0.85 } else { 0.55 };
        high = !high;

        // Flush the BLE advert window (one publish per ~2s loop tick).
        let batch: Vec<_> = {
            let mut m = adverts.lock().unwrap();
            m.drain().collect()
        };
        stats
            .scan_windows
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        seq = seq.wrapping_add(1);
        if let Err(e) = mqtt::publish_adverts(&mut mqtt, &board_id, &boot_id, seq, batch) {
            error!("{:?}", e);
        }

        // Retained health every 60s (and once right after boot).
        if last_health.map_or(true, |t| t.elapsed() >= Duration::from_secs(HEALTH_INTERVAL_SECS)) {
            last_health = Some(std::time::Instant::now());
            if let Err(e) =
                mqtt::publish_health(&mut mqtt, &board_id, boot_time.elapsed().as_secs(), &stats)
            {
                error!("{:?}", e);
            }
        }

        // Check for OTA periodically
        if last_ota_check.elapsed() >= Duration::from_secs(OTA_CHECK_INTERVAL_SECS) {
            last_ota_check = std::time::Instant::now();
            if let Err(e) = try_ota_update() {
                error!("OTA check failed: {:?}", e);
            }
        }

        thread::sleep(Duration::from_secs(2));
    }
}

fn try_ota_update() -> anyhow::Result<()> {
    info!("Checking OTA at {}", OTA_URL);

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
