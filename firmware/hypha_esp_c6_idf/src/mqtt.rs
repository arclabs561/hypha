//! MQTT uplink: per-window advert batches + retained health.
//!
//! Config is baked at build time (same contract as WIFI_SSID/WIFI_PASS):
//! MQTT_HOST required, MQTT_PORT/MQTT_USER/MQTT_PASS optional.

use std::cmp::Reverse;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::mqtt::client::{EspMqttClient, EventPayload, MqttClientConfiguration, QoS};
use log::{info, warn};

use crate::ble::{AdvertEntry, AdvertKey};
use crate::Stats;

const MQTT_HOST: &str = env!("MQTT_HOST");
const MQTT_PORT: &str = match option_env!("MQTT_PORT") {
    Some(p) => p,
    None => "1883",
};
const MQTT_USER: Option<&str> = option_env!("MQTT_USER");
const MQTT_PASS: Option<&str> = option_env!("MQTT_PASS");

/// Strongest-RSSI entries kept per published batch.
const BATCH_CAP: usize = 64;

pub fn connect(board_id: &str, stats: Arc<Stats>) -> anyhow::Result<EspMqttClient<'static>> {
    let url = format!("mqtt://{}:{}", MQTT_HOST, MQTT_PORT);
    let conf = MqttClientConfiguration {
        client_id: Some(board_id),
        username: MQTT_USER,
        password: MQTT_PASS,
        // Broker-loss handling is delegated to the ESP-IDF client: it
        // re-dials every `reconnect_timeout` until the broker is back.
        reconnect_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };

    // Connection is async; creation succeeds even if the broker is down and
    // the client keeps retrying. The callback only tracks connection state
    // (publishes happen from the main loop).
    let client = EspMqttClient::new_cb(&url, &conf, move |event| match event.payload() {
        EventPayload::Connected(_) => {
            stats.mqtt_connects.fetch_add(1, Ordering::Relaxed);
            info!("MQTT connected");
        }
        EventPayload::Disconnected => warn!("MQTT disconnected; will reconnect"),
        EventPayload::Error(e) => warn!("MQTT error: {:?}", e),
        _ => {}
    })?;

    info!("MQTT client created for {}", url);
    Ok(client)
}

pub fn publish_adverts(
    client: &mut EspMqttClient<'static>,
    board_id: &str,
    boot_id: &str,
    seq: u32,
    mut entries: Vec<(AdvertKey, AdvertEntry)>,
) -> anyhow::Result<()> {
    if entries.len() > BATCH_CAP {
        entries.sort_by_key(|(_, e)| Reverse(e.rssi));
        entries.truncate(BATCH_CAP);
    }

    let mut adverts = String::new();
    for (i, ((addr, addr_type), e)) in entries.iter().enumerate() {
        if i > 0 {
            adverts.push(',');
        }
        // Even raw types (BLE_ADDR_PUBLIC/PUBLIC_ID) are public; odd are random.
        let t = if addr_type & 1 == 0 { "pub" } else { "rnd" };
        adverts.push_str(&format!(
            r#"{{"a":"{}","t":"{}","r":{}"#,
            addr_str(addr),
            t,
            e.rssi
        ));
        if let Some(n) = &e.name {
            adverts.push_str(&format!(r#","n":"{}""#, n));
        }
        if let Some(m) = &e.mfr {
            adverts.push_str(&format!(r#","mfr":"{}""#, m));
        }
        adverts.push('}');
    }

    let payload = format!(
        r#"{{"board":"{}","boot":"{}","seq":{},"window_ms":2000,"adverts":[{}]}}"#,
        board_id, boot_id, seq, adverts
    );
    client
        .publish(
            &format!("hypha/{}/ble", board_id),
            QoS::AtMostOnce,
            false,
            payload.as_bytes(),
        )
        .map_err(|e| anyhow::anyhow!("ble publish: {:?}", e))?;
    Ok(())
}

pub fn publish_health(
    client: &mut EspMqttClient<'static>,
    board_id: &str,
    uptime_s: u64,
    stats: &Stats,
) -> anyhow::Result<()> {
    let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let wifi_rssi = sta_rssi();
    let connects = stats.mqtt_connects.load(Ordering::Relaxed);
    let payload = format!(
        r#"{{"board":"{}","fw":"{}","uptime_s":{},"heap_free":{},"wifi_rssi":{},"scan_windows":{},"adverts_seen":{},"mqtt_reconnects":{}}}"#,
        board_id,
        env!("CARGO_PKG_VERSION"),
        uptime_s,
        heap_free,
        wifi_rssi,
        stats.scan_windows.load(Ordering::Relaxed),
        stats.adverts_seen.load(Ordering::Relaxed),
        connects.saturating_sub(1),
    );
    client
        .publish(
            &format!("hypha/{}/health", board_id),
            QoS::AtMostOnce,
            true, // retained: the doctor reads last-known health + freshness
            payload.as_bytes(),
        )
        .map_err(|e| anyhow::anyhow!("health publish: {:?}", e))?;
    Ok(())
}

fn addr_str(addr: &[u8; 6]) -> String {
    addr.iter()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(":")
}

fn sta_rssi() -> i8 {
    let mut ap: esp_idf_svc::sys::wifi_ap_record_t = Default::default();
    let rc = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap) };
    if rc == esp_idf_svc::sys::ESP_OK {
        ap.rssi
    } else {
        0
    }
}
