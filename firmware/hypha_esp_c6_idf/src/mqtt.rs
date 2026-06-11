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

/// Command topic: operator publishes {"locate":true|false} to blink one board
/// for physical identification. Dumb substring parse on purpose (no JSON dep).
pub fn cmd_topic(board_id: &str) -> String {
    format!("hypha/{}/cmd", board_id)
}

/// Shared firefly-sync topic: every board publishes its pulse here and
/// subscribes to hear peers (the cross-board coupling channel).
pub const SYNC_TOPIC: &str = "hypha/sync/pulse";

pub fn connect(board_id: &str, stats: Arc<Stats>) -> anyhow::Result<EspMqttClient<'static>> {
    let url = format!("mqtt://{}:{}", MQTT_HOST, MQTT_PORT);
    let my_cmd = cmd_topic(board_id);
    let my_id = board_id.to_string();
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
            stats.mqtt_connected.store(true, Ordering::Relaxed);
            info!("MQTT connected");
        }
        EventPayload::Disconnected => {
            stats.mqtt_connected.store(false, Ordering::Relaxed);
            warn!("MQTT disconnected; will reconnect");
        }
        EventPayload::Received { topic, data, .. } => {
            if topic == Some(SYNC_TOPIC) {
                // a firefly pulse; ignore our own echo, couple on a peer's
                if core::str::from_utf8(data).unwrap_or("") != my_id {
                    stats.peer_pulses.fetch_add(1, Ordering::Relaxed);
                }
            }
            if topic == Some(my_cmd.as_str()) {
                // sentinel "" is the right failure shape here: a non-UTF8 command
                // matches no keyword and is ignored, which is the desired handling
                let body = core::str::from_utf8(data).unwrap_or("");
                if body.contains("locate") {
                    let on = body.contains("true");
                    stats.locate.store(on, Ordering::Relaxed);
                    info!("cmd: locate={}", on);
                }
                if body.contains("led") {
                    // {"led":"auto"|"metabolism"|"link"|"version"|"off"} -- dumb
                    // substring match, longest-distinct keywords, no JSON dep
                    let m = if body.contains("metabolism") {
                        crate::led::MODE_METABOLISM
                    } else if body.contains("link") {
                        crate::led::MODE_LINK
                    } else if body.contains("version") {
                        crate::led::MODE_VERSION
                    } else if body.contains("\"off\"") {
                        crate::led::MODE_OFF
                    } else {
                        crate::led::MODE_AUTO
                    };
                    stats.led_mode.store(m, Ordering::Relaxed);
                    info!("cmd: led mode={}", m);
                }
            }
        }
        EventPayload::Error(e) => warn!("MQTT error: {:?}", e),
        _ => {}
    })?;

    info!("MQTT client created for {}", url);
    Ok(client)
}

/// (Re)subscribe to the command topic. The esp-idf client does not carry
/// subscriptions across reconnects (clean session), so the main loop calls this
/// once per observed connection generation.
pub fn subscribe_cmd(client: &mut EspMqttClient<'static>, board_id: &str) -> anyhow::Result<()> {
    client
        .subscribe(&cmd_topic(board_id), QoS::AtLeastOnce)
        .map_err(|e| anyhow::anyhow!("cmd subscribe: {:?}", e))?;
    client
        .subscribe(SYNC_TOPIC, QoS::AtMostOnce) // firefly pulses: lossy is fine
        .map_err(|e| anyhow::anyhow!("sync subscribe: {:?}", e))?;
    Ok(())
}

/// Human-readable ESP reset reason for the boot event (why did it restart).
fn reset_reason() -> &'static str {
    use esp_idf_svc::sys::*;
    match unsafe { esp_reset_reason() } {
        x if x == esp_reset_reason_t_ESP_RST_POWERON => "poweron",
        x if x == esp_reset_reason_t_ESP_RST_SW => "sw-reset",
        x if x == esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deepsleep",
        x if x == esp_reset_reason_t_ESP_RST_BROWNOUT => "brownout",
        x if x == esp_reset_reason_t_ESP_RST_PANIC => "panic",
        x if x == esp_reset_reason_t_ESP_RST_INT_WDT => "int-watchdog",
        x if x == esp_reset_reason_t_ESP_RST_TASK_WDT => "task-watchdog",
        x if x == esp_reset_reason_t_ESP_RST_WDT => "watchdog",
        _ => "other",
    }
}

/// Retained boot announcement: lets telemetry watch boot sequences remotely
/// (reset reason distinguishes a clean OTA reboot from a panic/watchdog loop).
pub fn publish_boot(
    client: &mut EspMqttClient<'static>,
    board_id: &str,
    boot_id: &str,
    wifi_ms: u128,
) -> anyhow::Result<()> {
    let heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let payload = format!(
        r#"{{"board":"{}","ev":"boot","fw":"{}","boot":"{}","reset":"{}","wifi_ms":{},"heap_free":{}}}"#,
        board_id,
        env!("CARGO_PKG_VERSION"),
        boot_id,
        reset_reason(),
        wifi_ms,
        heap,
    );
    client
        .publish(
            &format!("hypha/{}/event", board_id),
            QoS::AtLeastOnce,
            true, // retained: a late watcher still sees the last boot
            payload.as_bytes(),
        )
        .map_err(|e| anyhow::anyhow!("boot event: {:?}", e))?;
    Ok(())
}

/// Publish this board's firefly pulse (fire-and-forget; peers couple on it).
pub fn publish_pulse(client: &mut EspMqttClient<'static>, board_id: &str) -> anyhow::Result<()> {
    client
        .publish(SYNC_TOPIC, QoS::AtMostOnce, false, board_id.as_bytes())
        .map_err(|e| anyhow::anyhow!("pulse publish: {:?}", e))?;
    Ok(())
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
        r#"{{"board":"{}","fw":"{}","uptime_s":{},"heap_free":{},"wifi_rssi":{},"scan_windows":{},"adverts_seen":{},"mqtt_reconnects":{},"fires":{},"led":"{:06x}","loop_max_ms":{}}}"#,
        board_id,
        env!("CARGO_PKG_VERSION"),
        uptime_s,
        heap_free,
        wifi_rssi,
        stats.scan_windows.load(Ordering::Relaxed),
        stats.adverts_seen.load(Ordering::Relaxed),
        connects.saturating_sub(1),
        // local firefly fire count: telemetry compares this to received pulses
        // on hypha/sync/pulse to separate fire-rate from pulse-loss
        stats.fire.load(Ordering::Relaxed),
        // actual rendered LED colour (0xRRGGBB) -- ground-truth hue in telemetry
        stats.led_rgb.load(Ordering::Relaxed),
        // worst main-loop period this window (ms): >100 means radio starvation,
        // the single-core scheduling bug; ~50 is healthy. The at-a-glance health.
        stats.loop_max_ms.load(Ordering::Relaxed),
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

pub fn sta_rssi() -> i8 {
    let mut ap: esp_idf_svc::sys::wifi_ap_record_t = Default::default();
    let rc = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap) };
    if rc == esp_idf_svc::sys::ESP_OK {
        ap.rssi
    } else {
        0
    }
}
