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

/// Command topic (momentary, never retained): {"locate":true|false},
/// {"led":"auto"|...}, {"led_max":0..255}.
pub fn cmd_topic(board_id: &str) -> String {
    format!("hypha/{}/cmd", board_id)
}

/// Config topic (retained = desired state): same {"led","led_max"} fields as
/// cmd, reapplied automatically on every (re)connect because the broker
/// redelivers the retained message per subscribe. This is how an LED mode or
/// night brightness survives a reboot. `locate` is deliberately NOT honored
/// here: find-me is momentary by definition, and a retained locate is exactly
/// the stuck-blinking failure shape.
pub fn config_topic(board_id: &str) -> String {
    format!("hypha/{}/config", board_id)
}

/// Minimal key-scoped JSON field extraction (no JSON dep): the raw token
/// after `"key":`, trimmed, up to the next ',' or '}'. Key must appear as a
/// quoted JSON key, so future keys can't false-match inside other words or
/// values (a bare substring parser matched "led" inside "enabled"/"failed").
fn json_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{}\"", key);
    let i = body.find(&pat)?;
    let rest = body[i + pat.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn mode_from_name(v: &str) -> Option<u8> {
    Some(match v {
        "auto" => crate::led::MODE_AUTO,
        "metabolism" => crate::led::MODE_METABOLISM,
        "link" => crate::led::MODE_LINK,
        "version" => crate::led::MODE_VERSION,
        "off" => crate::led::MODE_OFF,
        "carousel" => crate::led::MODE_CAROUSEL,
        _ => return None,
    })
}

pub fn mode_name(m: u8) -> &'static str {
    match m {
        crate::led::MODE_METABOLISM => "metabolism",
        crate::led::MODE_LINK => "link",
        crate::led::MODE_VERSION => "version",
        crate::led::MODE_OFF => "off",
        crate::led::MODE_CAROUSEL => "carousel",
        _ => "auto",
    }
}

/// Shared firefly-sync topic: every board publishes its pulse here and
/// subscribes to hear peers (the cross-board coupling channel).
pub const SYNC_TOPIC: &str = "hypha/sync/pulse";

pub fn connect(board_id: &str, stats: Arc<Stats>) -> anyhow::Result<EspMqttClient<'static>> {
    let url = format!("mqtt://{}:{}", MQTT_HOST, MQTT_PORT);
    let my_cmd = cmd_topic(board_id);
    let my_config = config_topic(board_id);
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
            let is_cmd = topic == Some(my_cmd.as_str());
            let is_config = topic == Some(my_config.as_str());
            if is_cmd || is_config {
                // sentinel "" is the right failure shape here: a non-UTF8 command
                // matches no key and is counted as ignored, the desired handling
                let body = core::str::from_utf8(data).unwrap_or("");
                let mut applied = false;
                if is_cmd {
                    // momentary only: never honored from the retained config
                    match json_field(body, "locate") {
                        Some("true") => {
                            stats.locate.store(true, Ordering::Relaxed);
                            applied = true;
                        }
                        Some("false") => {
                            stats.locate.store(false, Ordering::Relaxed);
                            applied = true;
                        }
                        _ => {}
                    }
                }
                if let Some(m) = json_field(body, "led")
                    .map(|v| v.trim_matches('"'))
                    .and_then(mode_from_name)
                {
                    stats.led_mode.store(m, Ordering::Relaxed);
                    applied = true;
                }
                if let Some(n) = json_field(body, "led_max").and_then(|v| v.parse::<u32>().ok()) {
                    stats.led_max.store(n.min(255), Ordering::Relaxed);
                    applied = true;
                }
                if applied {
                    // main publishes the ack event when it sees cmd_seq move
                    // (publishing from inside the event callback is unsafe re
                    // the client's own task)
                    stats.cmd_seq.fetch_add(1, Ordering::Relaxed);
                } else {
                    // surfaced in health: a rising count means someone is
                    // sending commands this firmware doesn't understand
                    stats.cmd_ignored.fetch_add(1, Ordering::Relaxed);
                }
                info!(
                    "{}: {} ({})",
                    if is_cmd { "cmd" } else { "config" },
                    body,
                    if applied { "applied" } else { "ignored" }
                );
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
    // Retained config redelivers on every subscribe: this line IS the
    // reboot/reconnect persistence of led mode + brightness.
    client
        .subscribe(&config_topic(board_id), QoS::AtLeastOnce)
        .map_err(|e| anyhow::anyhow!("config subscribe: {:?}", e))?;
    client
        .subscribe(SYNC_TOPIC, QoS::AtMostOnce) // firefly pulses: lossy is fine
        .map_err(|e| anyhow::anyhow!("sync subscribe: {:?}", e))?;
    Ok(())
}

/// Closed-loop command ack: echo what the parser actually decoded and applied
/// (non-retained event). "Did my command take?" and "is locate stuck on?"
/// become bus reads instead of live-probe debugging sessions.
pub fn publish_cmd_ack(
    client: &mut EspMqttClient<'static>,
    board_id: &str,
    stats: &Stats,
) -> anyhow::Result<()> {
    let payload = format!(
        r#"{{"board":"{}","ev":"cmd","locate":{},"mode":"{}","led_max":{},"cmd_ignored":{}}}"#,
        board_id,
        stats.locate.load(Ordering::Relaxed),
        mode_name(stats.led_mode.load(Ordering::Relaxed)),
        stats.led_max.load(Ordering::Relaxed),
        stats.cmd_ignored.load(Ordering::Relaxed),
    );
    client
        .publish(
            &format!("hypha/{}/event", board_id),
            QoS::AtLeastOnce,
            false,
            payload.as_bytes(),
        )
        .map_err(|e| anyhow::anyhow!("cmd ack: {:?}", e))?;
    Ok(())
}

/// Human-readable ESP reset reason for the boot event (why did it restart).
/// Pub: main also reads it to show the green "update applied" blinks on a
/// sw-reset boot (the reset an OTA install ends with).
pub fn reset_reason() -> &'static str {
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
    stats: &Stats,
) -> anyhow::Result<()> {
    let heap = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let placement_state =
        crate::placement::State::from_code(stats.placement_state.load(Ordering::Relaxed)).name();
    let payload = format!(
        r#"{{"board":"{}","ev":"boot","fw":"{}","boot":"{}","reset":"{}","wifi_ms":{},"heap_free":{},"placement_state":"{}","placement_aps":{},"placement_baseline_aps":{},"placement_common":{},"placement_shifted":{},"placement_jaccard_milli":{}}}"#,
        board_id,
        env!("CARGO_PKG_VERSION"),
        boot_id,
        reset_reason(),
        wifi_ms,
        heap,
        placement_state,
        stats.placement_aps.load(Ordering::Relaxed),
        stats.placement_baseline_aps.load(Ordering::Relaxed),
        stats.placement_common.load(Ordering::Relaxed),
        stats.placement_shifted.load(Ordering::Relaxed),
        stats.placement_jaccard_milli.load(Ordering::Relaxed),
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
        if let Some(peer) = &e.peer {
            adverts.push_str(&format!(r#","peer":"{}""#, peer));
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
    boot_id: &str,
    uptime_s: u64,
    stats: &Stats,
) -> anyhow::Result<()> {
    let heap_free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let connects = stats.mqtt_connects.load(Ordering::Relaxed);
    let led_state = stats.led_state.load(Ordering::Relaxed) as usize;
    let power_source = option_env!("POWER_SOURCE").unwrap_or("unknown");
    let placement_state =
        crate::placement::State::from_code(stats.placement_state.load(Ordering::Relaxed)).name();
    let payload = format!(
        r#"{{"board":"{}","fw":"{}","boot":"{}","power_source":"{}","uptime_s":{},"heap_free":{},"wifi_rssi":{},"rssi_err":{},"scan_windows":{},"adverts_seen":{},"mqtt_reconnects":{},"fires":{},"peer_pulses":{},"led":"{:06x}","led_state":"{}","mode":"{}","locate":{},"led_max":{},"cmd_ignored":{},"loop_max_ms":{},"ota_state":"{}","ota_checks":{},"ota_failures":{},"placement_state":"{}","placement_aps":{},"placement_baseline_aps":{},"placement_common":{},"placement_shifted":{},"placement_jaccard_milli":{}}}"#,
        board_id,
        env!("CARGO_PKG_VERSION"),
        boot_id,
        power_source,
        uptime_s,
        heap_free,
        // last-known reading; never a 0 sentinel, which aliases a failed read
        // to "perfect signal". rssi_err counts failed reads alongside it.
        stats.wifi_rssi.load(Ordering::Relaxed),
        stats.rssi_err.load(Ordering::Relaxed),
        stats.scan_windows.load(Ordering::Relaxed),
        stats.adverts_seen.load(Ordering::Relaxed),
        connects.saturating_sub(1),
        // local firefly fire count: telemetry compares this to received pulses
        // on hypha/sync/pulse to separate fire-rate from pulse-loss
        stats.fire.load(Ordering::Relaxed),
        // received firefly pulses through MQTT. This is broker-mediated peer
        // visibility, not direct ESP-NOW neighbor count.
        stats.peer_pulses.load(Ordering::Relaxed),
        // actual rendered LED colour (0xRRGGBB) -- ground-truth hue in telemetry
        stats.led_rgb.load(Ordering::Relaxed),
        // which vocabulary state produced that colour: the "why is it that
        // colour" answer, directly in telemetry
        crate::led::STATE_NAMES.get(led_state).unwrap_or(&"?"),
        mode_name(stats.led_mode.load(Ordering::Relaxed)),
        stats.locate.load(Ordering::Relaxed),
        stats.led_max.load(Ordering::Relaxed),
        stats.cmd_ignored.load(Ordering::Relaxed),
        // worst main-loop period this window (ms): >100 means radio starvation,
        // the single-core scheduling bug; ~50 is healthy. The at-a-glance health.
        stats.loop_max_ms.load(Ordering::Relaxed),
        crate::ota_security::ota_state_name(stats.ota_state.load(Ordering::Relaxed)),
        stats.ota_checks.load(Ordering::Relaxed),
        stats.ota_failures.load(Ordering::Relaxed),
        placement_state,
        stats.placement_aps.load(Ordering::Relaxed),
        stats.placement_baseline_aps.load(Ordering::Relaxed),
        stats.placement_common.load(Ordering::Relaxed),
        stats.placement_shifted.load(Ordering::Relaxed),
        stats.placement_jaccard_milli.load(Ordering::Relaxed),
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

/// None on a failed read (e.g. STA disconnected). Callers keep the last-known
/// value and count the failure; a 0-dBm sentinel here once made the LED Link
/// page render "perfect signal" exactly when WiFi was down.
pub fn sta_rssi() -> Option<i8> {
    let mut ap: esp_idf_svc::sys::wifi_ap_record_t = Default::default();
    let rc = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap) };
    if rc == esp_idf_svc::sys::ESP_OK {
        Some(ap.rssi)
    } else {
        None
    }
}
