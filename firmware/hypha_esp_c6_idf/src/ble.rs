//! Passive BLE advertisement scanning with per-window aggregation.
//!
//! A dedicated thread runs a continuous NimBLE scan. The advert callback
//! (which executes on the NimBLE host task, so it must stay cheap) folds
//! each advert into a shared map keyed by address: strongest RSSI wins,
//! name/manufacturer data are captured once when first seen. The main loop
//! drains the map every ~2 s and publishes the batch over MQTT.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use esp32_nimble::{BLEAdvertisedData, BLEAdvertisedDevice, BLEDevice, BLEScan};
use esp_idf_svc::hal::task::block_on;
use log::{info, warn};

use crate::Stats;

/// Aggregation key: big-endian address bytes + raw NimBLE address type
/// (BLE_ADDR_PUBLIC=0, RANDOM=1, PUBLIC_ID=2, RANDOM_ID=3).
pub type AdvertKey = ([u8; 6], u8);

pub struct AdvertEntry {
    pub rssi: i8,
    pub name: Option<String>,
    pub mfr: Option<String>,
}

pub type AdvertMap = Arc<Mutex<HashMap<AdvertKey, AdvertEntry>>>;

/// Bound on distinct addresses tracked between flushes (heap guard in dense
/// RF environments). The publish path separately keeps only the 64 strongest.
const MAP_CAP: usize = 256;

const NAME_MAX: usize = 24;
const MFR_MAX_BYTES: usize = 32;

pub fn spawn_scan_thread(map: AdvertMap, stats: Arc<Stats>) -> anyhow::Result<()> {
    thread::Builder::new()
        .name("ble_scan".into())
        .stack_size(8192)
        .spawn(move || scan_loop(map, stats))?;
    Ok(())
}

fn scan_loop(map: AdvertMap, stats: Arc<Stats>) {
    let device = BLEDevice::take();
    // Active scanning costs airtime (scan requests) and is unnecessary for
    // presence RSSI; opt in at build time with BLE_ACTIVE=1.
    let active = option_env!("BLE_ACTIVE") == Some("1");
    info!("BLE scan starting (active={})", active);

    loop {
        // Yield the shared radio to an OTA download (private design note coex): a
        // continuous BLE scan starves the HTTP transfer, so pause while
        // OTA_ACTIVE. Presence has a brief gap during the ~1-2 min update.
        if crate::OTA_ACTIVE.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }

        let mut scan = BLEScan::new();
        // filter_duplicates(true) is load-bearing for CPU, not just airtime:
        // the C6 is single-core + FPU-less, and with duplicates UNfiltered every
        // advert (~65/s) fires the callback on the high-priority NimBLE host
        // task, starving the app threads so the main loop ran every ~12s (the
        // firefly-8s, 4x-slow-advert, slow-OTA, watchdog symptoms all trace
        // here). Controller-side dedup gives one sighting per device per window
        // -- still a fresh RSSI sample per window, enough for presence -- at a
        // fraction of the callback rate. Coex window(30)<interval(100).
        scan.active_scan(active)
            .filter_duplicates(true)
            .interval(100)
            .window(30);

        // Finite 3s windows (not BLE_HS_FOREVER) so the loop can check
        // OTA_ACTIVE and yield the radio within a few seconds of an OTA start.
        let res = block_on(scan.start(device, 3000, |dev, data| {
            record_advert(&map, &stats, dev, &data);
            None::<()>
        }));

        if let Err(e) = res {
            warn!("BLE scan error: {:?}; restarting", e);
            thread::sleep(Duration::from_secs(1));
        }
    }
}

fn record_advert(
    map: &AdvertMap,
    stats: &Stats,
    dev: &BLEAdvertisedDevice,
    data: &BLEAdvertisedData<&[u8]>,
) {
    stats.adverts_seen.fetch_add(1, Ordering::Relaxed);

    let addr = dev.addr();
    let key: AdvertKey = (addr.as_be_bytes(), addr.addr_type() as u8);
    let rssi = dev.rssi();

    let mut m = map.lock().unwrap();
    let entry = match m.get_mut(&key) {
        Some(e) => e,
        None => {
            if m.len() >= MAP_CAP {
                return;
            }
            m.entry(key).or_insert(AdvertEntry {
                rssi: i8::MIN,
                name: None,
                mfr: None,
            })
        }
    };

    if rssi > entry.rssi {
        entry.rssi = rssi;
    }
    if entry.name.is_none() {
        entry.name = data.name().map(|n| sanitize_name(&n[..]));
    }
    if entry.mfr.is_none() {
        entry.mfr = data.manufacture_data().map(|md| {
            let mut bytes = md.company_identifier.to_le_bytes().to_vec();
            bytes.extend_from_slice(md.payload);
            bytes.truncate(MFR_MAX_BYTES);
            hex(&bytes)
        });
    }
}

/// Names go straight into hand-built JSON: keep printable ASCII minus the two
/// JSON-significant characters, replace the rest, and bound the length.
fn sanitize_name(raw: &[u8]) -> String {
    raw.iter()
        .take(NAME_MAX)
        .map(|&b| match b as char {
            c if c.is_ascii_graphic() && c != '"' && c != '\\' => c,
            ' ' => ' ',
            _ => '_',
        })
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}
