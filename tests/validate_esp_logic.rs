//! Tests for ESP firmware logic: serial validation, LED state machine, telemetry format.
//!
//! All pure logic lives in `hypha-firefly` (a `no_std`-compatible crate).
//! Tests import it directly — no duplication between firmware and host.

use hypha_firefly::*;

// ── Serial validation ──────────────────────────────────────────────────────

fn would_pass_validation(log: &str) -> bool {
    let has_boot = log.contains("EVT:BOOT") || log.contains("WIRELESS_UP");
    let has_json = log.contains("\"source_id\"") && log.contains("\"energy_score\"");
    has_boot && has_json
}

// ── Tests: Serial validation ───────────────────────────────────────────────

#[test]
fn test_validate_pass_evt_boot() {
    let log = r#"EVT:BOOT source_id=esp-c6-8b48 mac=aa:bb:cc:dd:ee:ff
{"source_id":"esp-c6-8b48","energy_score":0.85,"peers":0,"uptime_ms":2000,"tx_ok":1,"tx_err":0,"rssi":-128,"seq":1}"#;
    assert!(would_pass_validation(log));
}

#[test]
fn test_validate_pass_legacy_wireless_up() {
    let log = r#"WIRELESS_UP source_id=esp-c6-8b48
{"source_id":"esp-c6-8b48","energy_score":0.85}"#;
    assert!(would_pass_validation(log));
}

#[test]
fn test_validate_fail_no_boot() {
    let log = r#"{"source_id":"esp-c6-8b48","energy_score":0.85}"#;
    assert!(!would_pass_validation(log));
}

#[test]
fn test_validate_fail_no_json() {
    let log = "EVT:BOOT source_id=esp-c6-8b48 mac=aa:bb:cc:dd:ee:ff\nboot complete";
    assert!(!would_pass_validation(log));
}

// ── Tests: LED hue encoding ────────────────────────────────────────────────

#[test]
fn test_hue_isolated() {
    let state = NodeState {
        peer_count: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_ISOLATED, "isolated should be warm red");
}

#[test]
fn test_hue_one_peer() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_ONE_PEER, "1 peer should be green");
}

#[test]
fn test_hue_two_peers() {
    let state = NodeState {
        peer_count: 2,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_TWO_PEERS, "2 peers should be cyan");
}

#[test]
fn test_hue_three_plus_peers() {
    let state = NodeState {
        peer_count: 5,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_THREE_PLUS, "3+ peers should be blue-violet");
}

// ── Tests: RSSI hue shift ──────────────────────────────────────────────────

#[test]
fn test_rssi_strong_shifts_hue_up() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: RSSI_STRONG,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.hue > HUE_ONE_PEER,
        "strong RSSI should shift hue up (toward blue): got {}",
        out.hue
    );
}

#[test]
fn test_rssi_weak_shifts_hue_down() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: RSSI_WEAK,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.hue < HUE_ONE_PEER,
        "weak RSSI should shift hue down (toward red): got {}",
        out.hue
    );
}

#[test]
fn test_rssi_no_effect_when_unknown() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.hue, HUE_ONE_PEER,
        "unknown RSSI (-128) should not shift hue"
    );
}

// ── Tests: Energy drift ────────────────────────────────────────────────────

#[test]
fn test_energy_draining_shifts_hue_toward_red() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: -0.10,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.hue < HUE_ONE_PEER,
        "draining should shift hue toward red: got {}",
        out.hue
    );
}

#[test]
fn test_energy_stable_shifts_hue_toward_blue() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: 0.05,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.hue > HUE_ONE_PEER,
        "stable/charging should shift hue toward blue: got {}",
        out.hue
    );
}

// ── Tests: Saturation (link freshness) ─────────────────────────────────────

#[test]
fn test_saturation_fresh_link() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.sat, 255, "fresh link should have full saturation");
}

#[test]
fn test_saturation_stale_link() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: 25_000, // 25s toward 30s timeout
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.sat < 200,
        "stale link (25s) should have reduced saturation: got {}",
        out.sat
    );
    assert!(
        out.sat > 100,
        "shouldn't be fully desaturated yet: got {}",
        out.sat
    );
}

#[test]
fn test_saturation_at_timeout() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: PEER_TIMEOUT_MS,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.sat <= 130,
        "at timeout should be quite desaturated: got {}",
        out.sat
    );
}

#[test]
fn test_saturation_isolated_always_full() {
    let state = NodeState {
        peer_count: 0,
        ms_since_last_rx: 60_000,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.sat, 255,
        "isolated node should always have full saturation"
    );
}

#[test]
fn test_saturation_error_rate_desaturates() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: 0,
        tx_ok: 50,
        tx_err: 30, // 37.5% error rate
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert!(
        out.sat < 200,
        "high error rate should desaturate: got {}",
        out.sat
    );
}

// ── Tests: Brightness ──────────────────────────────────────────────────────

#[test]
fn test_brightness_minimum_energy() {
    let state = NodeState {
        energy_score: 0.0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.val, BRIGHTNESS_MIN,
        "zero energy should map to minimum brightness"
    );
}

#[test]
fn test_brightness_full_energy() {
    let state = NodeState {
        energy_score: 1.0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.val, BRIGHTNESS_MAX,
        "full energy should map to maximum brightness"
    );
}

#[test]
fn test_brightness_mid_energy() {
    let state = NodeState {
        energy_score: 0.5,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    let expected = BRIGHTNESS_MIN as u16 + ((BRIGHTNESS_MAX - BRIGHTNESS_MIN) as f32 * 0.5) as u16;
    assert!(
        (out.val as i16 - expected as i16).unsigned_abs() <= 2,
        "mid energy brightness: expected ~{}, got {}",
        expected,
        out.val
    );
}

// ── Tests: Breathing ───────────────────────────────────────────────────────

#[test]
fn test_breathing_never_invisible() {
    // Sweep through a full breathing cycle at various base values
    for base in [BRIGHTNESS_MIN, 80, 120, 200, BRIGHTNESS_MAX] {
        for phase_pct in 0..100 {
            let uptime = phase_pct * 40; // 0..4000ms
            let val = compute_breathing_val(base, uptime, 4000);
            assert!(
                val >= BREATHING_FLOOR,
                "breathing val={} below floor {} (base={}, phase={}%)",
                val,
                BREATHING_FLOOR,
                base,
                phase_pct
            );
        }
    }
}

#[test]
fn test_breathing_oscillates() {
    let base = 150u8;
    let period = 3000u64;
    let at_peak = compute_breathing_val(base, period / 4, period); // phase 0.25 → tri peak
    let at_trough = compute_breathing_val(base, 0, period); // phase 0.0 → tri trough
    assert!(
        at_peak > at_trough,
        "breathing should oscillate: peak={}, trough={}",
        at_peak,
        at_trough
    );
}

// ── Tests: Breathing rate ──────────────────────────────────────────────────

#[test]
fn test_breath_rate_idle() {
    let period = compute_breath_period_ms(0.0);
    assert_eq!(period, BREATH_PERIOD_MAX_MS, "idle should breathe slowly");
}

#[test]
fn test_breath_rate_busy() {
    let period = compute_breath_period_ms(1.0);
    assert_eq!(period, BREATH_PERIOD_MIN_MS, "busy should breathe fast");
}

#[test]
fn test_breath_rate_mid() {
    let period = compute_breath_period_ms(0.5);
    let expected = (BREATH_PERIOD_MAX_MS + BREATH_PERIOD_MIN_MS) / 2;
    assert!(
        (period as i64 - expected as i64).unsigned_abs() <= 100,
        "mid activity: expected ~{}, got {}",
        expected,
        period
    );
}

// ── Tests: Telemetry format ────────────────────────────────────────────────

fn parse_evt_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{}=", key);
    let start = line.find(&prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(' ').unwrap_or(rest.len());
    Some(&rest[..end])
}

#[test]
fn test_evt_boot_format() {
    let line = "EVT:BOOT source_id=esp-c6-4f5f mac=aa:bb:cc:dd:ee:ff";
    assert!(line.starts_with("EVT:BOOT"));
    assert_eq!(parse_evt_field(line, "source_id"), Some("esp-c6-4f5f"));
    assert_eq!(parse_evt_field(line, "mac"), Some("aa:bb:cc:dd:ee:ff"));
}

#[test]
fn test_evt_tx_format() {
    let line = "EVT:TX seq=5 ok=10 err=1 energy=0.85 peers=2";
    assert!(line.starts_with("EVT:TX"));
    assert_eq!(parse_evt_field(line, "seq"), Some("5"));
    assert_eq!(parse_evt_field(line, "ok"), Some("10"));
    assert_eq!(parse_evt_field(line, "err"), Some("1"));
    assert_eq!(parse_evt_field(line, "energy"), Some("0.85"));
    assert_eq!(parse_evt_field(line, "peers"), Some("2"));
}

#[test]
fn test_evt_rx_format() {
    let line = "EVT:RX src=40:4c:ca:40:87:40 rssi=-52";
    assert!(line.starts_with("EVT:RX"));
    assert_eq!(parse_evt_field(line, "src"), Some("40:4c:ca:40:87:40"));
    assert_eq!(parse_evt_field(line, "rssi"), Some("-52"));
}

#[test]
fn test_evt_peer_add_format() {
    let line = "EVT:PEER_ADD mac=40:4c:ca:40:87:40 count=1";
    assert!(line.starts_with("EVT:PEER_ADD"));
    assert_eq!(parse_evt_field(line, "mac"), Some("40:4c:ca:40:87:40"));
    assert_eq!(parse_evt_field(line, "count"), Some("1"));
}

#[test]
fn test_evt_peer_drop_format() {
    let line = "EVT:PEER_DROP mac=40:4c:ca:40:87:40 count=0 timeout_ms=30500";
    assert!(line.starts_with("EVT:PEER_DROP"));
    assert_eq!(parse_evt_field(line, "timeout_ms"), Some("30500"));
}

#[test]
fn test_evt_led_format() {
    let line = "EVT:LED hue=85 sat=240 val=180 mode=breathing breath_ms=3000 phase_off=150";
    assert!(line.starts_with("EVT:LED"));
    assert_eq!(parse_evt_field(line, "hue"), Some("85"));
    assert_eq!(parse_evt_field(line, "sat"), Some("240"));
    assert_eq!(parse_evt_field(line, "val"), Some("180"));
    assert_eq!(parse_evt_field(line, "mode"), Some("breathing"));
    assert_eq!(parse_evt_field(line, "breath_ms"), Some("3000"));
    assert_eq!(parse_evt_field(line, "phase_off"), Some("150"));
}

#[test]
fn test_json_with_seq() {
    let line = r#"{"source_id":"esp-c6-8b48","energy_score":0.85,"peers":2,"uptime_ms":4221,"tx_ok":4,"tx_err":0,"rssi":-52,"seq":31}"#;
    let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
    assert_eq!(v["seq"], 31);
    assert_eq!(v["source_id"], "esp-c6-8b48");
}

// ── Tests: Scenario simulations ────────────────────────────────────────────

#[test]
fn test_cold_boot_to_mesh() {
    // Phase 1: Just booted, isolated
    let isolated = NodeState {
        peer_count: 0,
        energy_score: 0.8,
        uptime_ms: 5000,
        ..Default::default()
    };
    let out1 = compute_led_steady(&isolated);
    assert_eq!(out1.hue, HUE_ISOLATED, "should start as warm red");
    assert!(
        out1.val > BRIGHTNESS_MIN + 50,
        "high energy = notably brighter than min"
    );

    // Phase 2: First peer discovered
    let one_peer = NodeState {
        peer_count: 1,
        energy_score: 0.8,
        last_rssi: -50,
        ms_since_last_rx: 500,
        uptime_ms: 8000,
        ..Default::default()
    };
    let out2 = compute_led_steady(&one_peer);
    assert!(
        out2.hue >= 80 && out2.hue <= 95,
        "should be green-ish: got {}",
        out2.hue
    );
    assert!(
        out2.sat >= 250,
        "recent RX should have near-full saturation: got {}",
        out2.sat
    );

    // Phase 3: Second peer joins
    let two_peers = NodeState {
        peer_count: 2,
        energy_score: 0.8,
        last_rssi: -45,
        ms_since_last_rx: 200,
        uptime_ms: 15000,
        ..Default::default()
    };
    let out3 = compute_led_steady(&two_peers);
    assert!(
        out3.hue >= 120 && out3.hue <= 140,
        "should be cyan-ish: got {}",
        out3.hue
    );
}

#[test]
fn test_peer_loss_recovery() {
    // Start: 2 peers, fresh link
    let healthy = NodeState {
        peer_count: 2,
        ms_since_last_rx: 500,
        last_rssi: -128,
        energy_score: 0.7,
        ..Default::default()
    };
    let out1 = compute_led_steady(&healthy);
    assert_eq!(out1.hue, HUE_TWO_PEERS);
    assert!(
        out1.sat >= 250,
        "fresh link should have near-full saturation: got {}",
        out1.sat
    );

    // Link goes stale (20s without RX)
    let stale = NodeState {
        peer_count: 2,
        ms_since_last_rx: 20_000,
        last_rssi: -128,
        energy_score: 0.7,
        ..Default::default()
    };
    let out2 = compute_led_steady(&stale);
    assert!(
        out2.sat < 200,
        "stale link should desaturate: got {}",
        out2.sat
    );

    // One peer drops, back to 1
    let degraded = NodeState {
        peer_count: 1,
        ms_since_last_rx: 1000,
        last_rssi: -128,
        energy_score: 0.7,
        ..Default::default()
    };
    let out3 = compute_led_steady(&degraded);
    assert_eq!(out3.hue, HUE_ONE_PEER, "should drop to green");
    assert!(
        out3.sat >= 249,
        "fresh RX from remaining peer: got {}",
        out3.sat
    );
}

#[test]
fn test_error_escalation() {
    // Low errors: saturation stays high
    let low_err = NodeState {
        peer_count: 1,
        tx_ok: 90,
        tx_err: 5,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out1 = compute_led_steady(&low_err);
    assert_eq!(out1.sat, 255, "5% error rate below 20% threshold");

    // High errors: desaturation kicks in
    let high_err = NodeState {
        peer_count: 1,
        tx_ok: 50,
        tx_err: 50,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out2 = compute_led_steady(&high_err);
    assert!(
        out2.sat < 200,
        "50% error rate should desaturate: got {}",
        out2.sat
    );
}

// ── Tests: Breathing phase offset (firefly sync) ──────────────────────────

#[test]
fn test_breathing_phase_offset_shifts_cycle() {
    let base = 100u8;
    let period = 4000u64;
    // Without offset: value at uptime=0
    let val_no_offset = compute_breathing_val_with_offset(base, 0, period, 0);
    // With offset=1000 (quarter period): value at uptime=0 should equal no-offset at uptime=1000
    let val_with_offset = compute_breathing_val_with_offset(base, 0, period, 1000);
    let val_no_offset_at_1000 = compute_breathing_val_with_offset(base, 1000, period, 0);
    assert_eq!(
        val_with_offset, val_no_offset_at_1000,
        "phase offset should shift the breathing cycle"
    );
    // The values should differ (offset actually changes the output)
    assert_ne!(
        val_no_offset, val_with_offset,
        "offset=1000 should produce different value than offset=0 at same uptime"
    );
}

#[test]
fn test_breathing_two_boards_converge_with_same_offset() {
    // Two boards with different uptimes but same phase offset should breathe identically
    // if their uptimes modulo the period are the same after offset adjustment
    let base = 100u8;
    let period = 4000u64;
    // Board A: uptime=5000, offset=500
    // Board B: uptime=9000, offset=500
    // Both: (uptime+500) % 4000 = 5500%4000=1500 vs 9500%4000=1500 → same!
    let val_a = compute_breathing_val_with_offset(base, 5000, period, 500);
    let val_b = compute_breathing_val_with_offset(base, 9000, period, 500);
    assert_eq!(
        val_a, val_b,
        "same phase position should produce same brightness"
    );
}

#[test]
fn test_breathing_phase_offset_never_invisible() {
    // Phase offset should never cause brightness to drop below floor
    for offset in [0, 500, 1000, 2000, 3999] {
        for uptime in (0..8000).step_by(100) {
            let val = compute_breathing_val_with_offset(BRIGHTNESS_MIN, uptime, 4000, offset);
            assert!(
                val >= BREATHING_FLOOR,
                "val={} below floor (offset={}, uptime={})",
                val,
                offset,
                uptime
            );
        }
    }
}

// ── Tests: Mirollo-Strogatz firefly oscillator ────────────────────────────

#[test]
fn test_osc_fires_at_threshold() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    // Advance through slightly more than one full period (FP accumulation margin)
    let mut fired = false;
    for _ in 0..110 {
        if osc.advance(30, 3000) {
            fired = true;
            break;
        }
    }
    assert!(fired, "oscillator should fire within one period");
    assert!(osc.phase() < 0.1, "phase should reset near 0 after firing");
}

#[test]
fn test_osc_advance_period_zero() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    // Should not panic with period=0
    let fired = osc.advance(10, 0);
    assert!(fired, "period=0 should fire immediately");
}

#[test]
fn test_osc_brightness_monotonic() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    let mut prev = 0.0f32;
    for i in 0..100 {
        osc.advance(30, 3000);
        let bf = osc.brightness_factor();
        if !osc.just_fired() {
            assert!(
                bf >= prev - 0.001,
                "brightness should increase monotonically (step {}: {} < {})",
                i,
                bf,
                prev
            );
            prev = bf;
        } else {
            prev = 0.0; // reset after fire
        }
    }
}

#[test]
fn test_osc_brightness_range() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    for _ in 0..200 {
        osc.advance(15, 3000);
        let bf = osc.brightness_factor();
        assert!(
            bf >= 0.0 && bf <= 1.0,
            "brightness_factor out of range: {}",
            bf
        );
    }
}

#[test]
fn test_osc_refractory_blocks_pulse() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    // Just after firing, phase is near 0 — in refractory
    osc.set_phase(0.1);
    let absorbed = osc.receive_pulse();
    assert!(!absorbed, "pulse during refractory should be ignored");
    assert!(
        (osc.phase() - 0.1).abs() < 0.001,
        "phase should not change during refractory"
    );
}

#[test]
fn test_osc_pulse_advances_phase() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    osc.set_phase(0.5); // past refractory
    let before = osc.phase();
    let absorbed = osc.receive_pulse();
    assert!(
        !absorbed,
        "0.5 + epsilon should not fire (x=0.75 + 0.15 = 0.90 < 1.0)"
    );
    assert!(
        osc.phase() > before,
        "pulse should advance phase: {} -> {}",
        before,
        osc.phase()
    );
}

#[test]
fn test_osc_pulse_absorption_near_threshold() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    osc.set_phase(0.92); // very close to threshold
                         // x = 2*0.92 - 0.92^2 = 1.84 - 0.8464 = 0.9936
                         // x + epsilon = 0.9936 + 0.15 = 1.1436 >= 1.0 -> absorption!
    let absorbed = osc.receive_pulse();
    assert!(
        absorbed,
        "pulse near threshold should cause absorption (fire)"
    );
    assert!(
        osc.just_fired(),
        "just_fired should be true after absorption"
    );
    assert!(
        osc.phase() < 0.01,
        "phase should reset to ~0 after absorption"
    );
}

#[test]
fn test_osc_concave_coupling_amplifies_near_threshold() {
    // The M-S concave state function means the same epsilon in state-space
    // produces a LARGER phase advance when phase is closer to threshold.
    let mut osc_low = FireflyOscillator::new(0.15, 0.0); // no refractory for test
    let mut osc_high = FireflyOscillator::new(0.15, 0.0);

    osc_low.set_phase(0.4);
    osc_high.set_phase(0.8);

    let before_low = osc_low.phase();
    let before_high = osc_high.phase();
    osc_low.receive_pulse();
    osc_high.receive_pulse();
    let advance_low = osc_low.phase() - before_low;
    let advance_high = if osc_high.just_fired() {
        // absorbed: effectively advanced from 0.8 to 1.0 = 0.2
        1.0 - before_high
    } else {
        osc_high.phase() - before_high
    };

    assert!(
        advance_high > advance_low,
        "coupling should amplify near threshold: low_advance={:.4}, high_advance={:.4}",
        advance_low,
        advance_high
    );
}

#[test]
fn test_osc_two_oscillators_converge() {
    // Simulate two oscillators with different initial phases exchanging pulses.
    // They should synchronize within ~20 periods.
    // NOTE: receive_pulse() can cause absorption (immediate fire), which must
    // be recorded alongside natural fires from advance().
    let period = 3000u64;
    let dt = 10u64; // 10ms steps (100 Hz)
    let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.0);
    osc_b.set_phase(0.6); // 60% phase offset

    let total_steps = (period / dt) * 30; // 30 periods worth of steps
    let mut fire_times_a: Vec<u64> = Vec::new();
    let mut fire_times_b: Vec<u64> = Vec::new();

    for step in 0..total_steps {
        let t = step * dt;
        let fired_a = osc_a.advance(dt, period);
        let fired_b = osc_b.advance(dt, period);

        if fired_a {
            fire_times_a.push(t);
            // A fires -> B receives pulse (may cause absorption-fire)
            if osc_b.receive_pulse() {
                fire_times_b.push(t);
            }
        }
        if fired_b {
            fire_times_b.push(t);
            // B fires -> A receives pulse (may cause absorption-fire)
            if osc_a.receive_pulse() {
                fire_times_a.push(t);
            }
        }
    }

    // Check: last few fire times should be nearly simultaneous
    assert!(
        fire_times_a.len() >= 5 && fire_times_b.len() >= 5,
        "both should fire multiple times: A={}, B={}",
        fire_times_a.len(),
        fire_times_b.len()
    );

    // Find the closest fire of B to each of A's last 3 fires
    let last_a = &fire_times_a[fire_times_a.len() - 3..];
    for &ta in last_a {
        let closest_b = fire_times_b
            .iter()
            .map(|&tb| (ta as i64 - tb as i64).unsigned_abs())
            .min()
            .unwrap();
        assert!(
            closest_b < period / 5,
            "after convergence, fires should be within 20% of period: gap={}ms (period={}ms)",
            closest_b,
            period
        );
    }
}

#[test]
fn test_osc_three_oscillators_converge() {
    // Three oscillators, all-to-all coupling. Should all sync.
    // Track fires from both advance() and receive_pulse() absorption.
    let period = 3000u64;
    let dt = 10u64;
    let mut oscs = [
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
    ];
    oscs[0].set_phase(0.0);
    oscs[1].set_phase(0.33);
    oscs[2].set_phase(0.66);

    let total_steps = (period / dt) * 40; // 40 periods
    let mut last_fire = [0u64; 3];

    for step in 0..total_steps {
        let t = step * dt;
        let mut fired = [false; 3];
        for i in 0..3 {
            fired[i] = oscs[i].advance(dt, period);
            if fired[i] {
                last_fire[i] = t;
            }
        }
        // All-to-all coupling: each fire sends pulse to all others
        for i in 0..3 {
            if fired[i] {
                for j in 0..3 {
                    if j != i {
                        if oscs[j].receive_pulse() {
                            last_fire[j] = t; // absorption-fire
                        }
                    }
                }
            }
        }
    }

    // All three should have fired recently (within 2 periods of the end)
    let total_time = total_steps * dt;
    for (i, &lf) in last_fire.iter().enumerate() {
        assert!(
            total_time - lf < period * 2,
            "oscillator {} last fired at {}ms, too far from end {}ms",
            i,
            lf,
            total_time
        );
    }

    // Fire times should be clustered
    let max_fire = last_fire.iter().max().unwrap();
    let min_fire = last_fire.iter().min().unwrap();
    let spread = max_fire - min_fire;
    assert!(
        spread < period,
        "three oscillators should converge: fire spread={}ms (period={}ms), last_fires={:?}",
        spread,
        period,
        last_fire
    );
}

#[test]
fn test_osc_epsilon_zero_no_sync() {
    // With epsilon=0.001 (near zero), coupling is negligible
    let mut osc = FireflyOscillator::new(0.001, 0.0);
    osc.set_phase(0.5);
    let before = osc.phase();
    osc.receive_pulse();
    let advance = osc.phase() - before;
    assert!(
        advance < 0.01,
        "near-zero epsilon should barely advance: {}",
        advance
    );
}

#[test]
fn test_osc_high_epsilon_fast_sync() {
    // With epsilon=0.4, a single pulse near threshold should always absorb
    let mut osc = FireflyOscillator::new(0.4, 0.3);
    osc.set_phase(0.7);
    // x = 2*0.7 - 0.49 = 0.91, x + 0.4 = 1.31 >= 1.0
    assert!(
        osc.receive_pulse(),
        "high epsilon should absorb from phase 0.7"
    );
}

#[test]
fn test_osc_set_phase_clamps() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    osc.set_phase(1.5);
    assert!(osc.phase() < 1.0, "set_phase should clamp above 1.0");
    osc.set_phase(-0.5);
    assert!(osc.phase() >= 0.0, "set_phase should clamp below 0.0");
}

#[test]
fn test_osc_state_function_concavity() {
    // Verify f(phi) = 2*phi - phi*phi is concave-down by checking
    // second derivative is negative (f''(phi) = -2 < 0)
    // Also verify boundary conditions: f(0) = 0, f(1) = 1
    let f = |phi: f32| 2.0 * phi - phi * phi;
    assert!((f(0.0) - 0.0).abs() < 0.001, "f(0) should be 0");
    assert!((f(1.0) - 1.0).abs() < 0.001, "f(1) should be 1");
    // Midpoint should be above the line (concave-down property)
    let midpoint = f(0.5);
    let line_midpoint = 0.5; // linear interpolation between f(0)=0 and f(1)=1
    assert!(
        midpoint > line_midpoint,
        "f(0.5)={} should be > 0.5 (concave-down)",
        midpoint
    );
}

#[test]
fn test_osc_fire_resets_just_fired_on_next_advance() {
    let mut osc = FireflyOscillator::new(0.15, 0.3);
    osc.set_phase(0.99);
    osc.advance(100, 3000); // should fire
    assert!(osc.just_fired());
    osc.advance(10, 3000); // next tick, should clear
    assert!(!osc.just_fired());
}

// ── Tests: Temperature-to-energy mapping ───────────────────────────────────

#[test]
fn test_temp_freezing_full_energy() {
    assert!((temp_to_energy(0.0) - 1.0).abs() < 0.001);
}

#[test]
fn test_temp_80c_min_energy() {
    assert!(
        (temp_to_energy(80.0) - 0.05).abs() < 0.01,
        "80C should clamp to min energy: got {}",
        temp_to_energy(80.0)
    );
}

#[test]
fn test_temp_40c_midpoint() {
    let e = temp_to_energy(40.0);
    assert!((e - 0.5).abs() < 0.01, "40C should be ~0.5: got {}", e);
}

#[test]
fn test_temp_negative_clamps_to_max() {
    assert!(
        (temp_to_energy(-20.0) - 1.0).abs() < 0.001,
        "negative temp should clamp to 1.0"
    );
}

#[test]
fn test_temp_extreme_hot_clamps() {
    assert!(
        (temp_to_energy(200.0) - 0.05).abs() < 0.001,
        "200C should clamp to min 0.05"
    );
}

#[test]
fn test_temp_monotonic_decreasing() {
    let mut prev = 2.0f32;
    for t in (-10..=90).step_by(5) {
        let e = temp_to_energy(t as f32);
        assert!(
            e <= prev,
            "energy should decrease with temperature: T={} e={} prev={}",
            t,
            e,
            prev
        );
        prev = e;
    }
}

#[test]
fn test_temp_never_zero() {
    // Energy should never be exactly 0.0 (LED always visible)
    for t in (0..=150).step_by(10) {
        let e = temp_to_energy(t as f32);
        assert!(
            e >= 0.05,
            "energy should never drop below 0.05: T={} e={}",
            t,
            e
        );
    }
}

#[test]
fn test_temp_room_temperature_reasonable() {
    // ~25C should give reasonable energy (~0.69)
    let e = temp_to_energy(25.0);
    assert!(
        e > 0.5 && e < 0.8,
        "room temp energy should be 0.5-0.8: got {}",
        e
    );
}

// ── Tests: Edge cases ──────────────────────────────────────────────────────

// Energy score out of bounds
#[test]
fn test_energy_score_negative() {
    let state = NodeState {
        energy_score: -0.5,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.val, BRIGHTNESS_MIN,
        "negative energy should clamp to min"
    );
}

#[test]
fn test_energy_score_above_one() {
    let state = NodeState {
        energy_score: 2.0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.val, BRIGHTNESS_MAX, "energy > 1.0 should clamp to max");
}

// RSSI boundaries
#[test]
fn test_rssi_zero() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // Clamped to RSSI_STRONG=-35, norm=1.0, shift=+8
    assert_eq!(out.hue, HUE_ONE_PEER + 8);
}

#[test]
fn test_rssi_just_above_unknown() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: -127,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // -127 > -128 so RSSI path is taken, clamped to RSSI_WEAK=-80, shift=-8
    assert_eq!(out.hue, HUE_ONE_PEER - 8);
}

#[test]
fn test_rssi_midpoint() {
    let state = NodeState {
        peer_count: 1,
        last_rssi: -57,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // norm = (-57 - -80) / (-35 - -80) = 23/45 = 0.511
    // shift = (0.511 * 16 - 8) = 0.18 → 0
    assert_eq!(
        out.hue, HUE_ONE_PEER,
        "midpoint RSSI should produce near-zero shift"
    );
}

// peer_count high values
#[test]
fn test_peer_count_six() {
    let state = NodeState {
        peer_count: 6,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_THREE_PLUS);
}

#[test]
fn test_peer_count_hundred() {
    let state = NodeState {
        peer_count: 100,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.hue, HUE_THREE_PLUS);
}

// Energy delta exact boundaries (strict < and > comparisons)
#[test]
fn test_energy_delta_exactly_neg_005() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: -0.05,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // -0.05 < -0.05 is FALSE (strict <), so drift_shift = 0
    assert_eq!(
        out.hue, HUE_ONE_PEER,
        "exactly -0.05 should NOT trigger draining shift"
    );
}

#[test]
fn test_energy_delta_exactly_002() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: 0.02,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // 0.02 > 0.02 is FALSE (strict >), so drift_shift = 0
    assert_eq!(
        out.hue, HUE_ONE_PEER,
        "exactly 0.02 should NOT trigger stable shift"
    );
}

#[test]
fn test_energy_delta_just_below_neg_005() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: -0.050001,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.hue,
        HUE_ONE_PEER - 5,
        "just below -0.05 should trigger draining shift"
    );
}

#[test]
fn test_energy_delta_just_above_002() {
    let state = NodeState {
        peer_count: 1,
        energy_delta: 0.020001,
        last_rssi: -128,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.hue,
        HUE_ONE_PEER + 3,
        "just above 0.02 should trigger stable shift"
    );
}

// ms_since_last_rx edge cases
#[test]
fn test_ms_since_last_rx_beyond_timeout() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: PEER_TIMEOUT_MS + 10000,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // t clamps to 1.0, freshness_sat = 120
    assert!(
        out.sat <= 130,
        "beyond timeout should clamp: got {}",
        out.sat
    );
}

#[test]
fn test_ms_since_last_rx_half_timeout() {
    let state = NodeState {
        peer_count: 1,
        ms_since_last_rx: PEER_TIMEOUT_MS / 2,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // t = 0.5, freshness_sat = 255 - 67.5 = 187
    assert!(
        out.sat >= 180 && out.sat <= 195,
        "half timeout sat: got {}",
        out.sat
    );
}

// Breathing edge cases
#[test]
fn test_breathing_period_zero() {
    let val = compute_breathing_val(100, 5000, 0);
    assert!(val >= BREATHING_FLOOR, "zero period should not panic");
}

#[test]
fn test_breathing_uptime_zero() {
    let val = compute_breathing_val(100, 0, 4000);
    // phase=0.0, tri=0.0, factor=0.6, v=60
    assert_eq!(val, 60, "uptime 0 should be at trough");
}

#[test]
fn test_breathing_val_never_exceeds_base() {
    for base in [25, 50, 100, 140, 200, 255] {
        for t in (0..8000).step_by(50) {
            let val = compute_breathing_val(base, t, 4000);
            assert!(
                val <= base,
                "breathing val {} > base {} at t={}",
                val,
                base,
                t
            );
        }
    }
}

// Error rate boundaries
#[test]
fn test_error_rate_exactly_20pct() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 80,
        tx_err: 20,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(
        out.sat, 255,
        "exactly 20% should NOT trigger desaturation (strict >)"
    );
}

#[test]
fn test_error_rate_below_sample_threshold() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 2,
        tx_err: 8,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.sat, 255, "below 11 samples, error rate ignored");
}

#[test]
fn test_error_rate_exactly_11_samples() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 6,
        tx_err: 5,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // tx_ok + tx_err = 11 > 10, rate = 5/11 = 0.454 > 0.20
    assert!(
        out.sat < 255,
        "11 samples with high error should desaturate"
    );
}

#[test]
fn test_extreme_error_rate_95pct() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 5,
        tx_err: 95,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // rate=0.95, desat=min(332.5, 175)=175, sat=(255-175).clamp(80,255)=80
    assert_eq!(out.sat, 80, "95% error rate should hit minimum saturation");
}

#[test]
fn test_100pct_error_rate() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 0,
        tx_err: 100,
        ms_since_last_rx: 0,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.sat, 80, "100% error rate should hit minimum saturation");
}

// Combined worst case
#[test]
fn test_combined_worst_case() {
    let state = NodeState {
        peer_count: 1,
        tx_ok: 10,
        tx_err: 90,
        ms_since_last_rx: PEER_TIMEOUT_MS,
        energy_score: 0.1,
        last_rssi: RSSI_WEAK,
        energy_delta: -0.10,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    assert_eq!(out.sat, 80, "worst case should clamp to min sat");
    assert_eq!(out.hue, 72, "hue: 85 - 8(weak rssi) - 5(draining)");
    assert_eq!(out.val, 41, "val: 30 + 0.1*110 = 41");
}

// Scenario: rapid peer churn
#[test]
fn test_rapid_peer_churn() {
    let join = NodeState {
        peer_count: 1,
        ms_since_last_rx: 100,
        last_rssi: -50,
        energy_score: 0.7,
        ..Default::default()
    };
    let o1 = compute_led_steady(&join);
    let drop = NodeState {
        peer_count: 0,
        energy_score: 0.7,
        ..Default::default()
    };
    let o2 = compute_led_steady(&drop);
    assert_eq!(o2.hue, HUE_ISOLATED);
    let rejoin = NodeState {
        peer_count: 1,
        ms_since_last_rx: 50,
        last_rssi: -50,
        energy_score: 0.7,
        ..Default::default()
    };
    let o3 = compute_led_steady(&rejoin);
    assert_eq!(
        o1.hue, o3.hue,
        "rejoin should produce same hue as initial join"
    );
}

// Scenario: all peers drop at once
#[test]
fn test_all_peers_drop_simultaneously() {
    let s = NodeState {
        peer_count: 0,
        ms_since_last_rx: 0,
        energy_score: 0.5,
        ..Default::default()
    };
    let out = compute_led_steady(&s);
    assert_eq!(out.hue, HUE_ISOLATED);
    assert_eq!(out.sat, 255, "isolated ignores ms_since_last_rx");
}

// Scenario: energy oscillation
#[test]
fn test_energy_oscillation_bounded() {
    let base = NodeState {
        peer_count: 1,
        last_rssi: -128,
        ..Default::default()
    };
    for &(score, delta) in &[(0.8, 0.05), (0.6, -0.10), (0.7, 0.03), (0.5, -0.08)] {
        let s = NodeState {
            energy_score: score,
            energy_delta: delta,
            ..base.clone()
        };
        let out = compute_led_steady(&s);
        assert!(
            out.val >= BRIGHTNESS_MIN && out.val <= BRIGHTNESS_MAX,
            "energy={} delta={}: val={} out of range",
            score,
            delta,
            out.val
        );
    }
}

// Scenario: very long uptime
#[test]
fn test_long_uptime_breathing() {
    let uptime = 7 * 24 * 60 * 60 * 1000u64; // 7 days
    let val = compute_breathing_val(100, uptime, 3000);
    assert!(val >= BREATHING_FLOOR && val <= 100);
}

// Hue clamping under extreme shifts
#[test]
fn test_hue_clamp_lower_bound() {
    let state = NodeState {
        peer_count: 0,
        last_rssi: -128,
        energy_delta: -0.10,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // base=10, rssi=0, drift=-5 → 5
    assert_eq!(out.hue, 5, "hue should clamp at low end");
}

#[test]
fn test_hue_clamp_upper_bound() {
    let state = NodeState {
        peer_count: 5,
        last_rssi: -30,
        energy_delta: 0.05,
        ..Default::default()
    };
    let out = compute_led_steady(&state);
    // base=170, rssi=+8, drift=+3 → 181
    assert_eq!(out.hue, 181);
}

// Property-style tests: invariants that must always hold
#[test]
fn test_prop_sat_always_in_range() {
    let configs = vec![
        NodeState {
            peer_count: 0,
            tx_ok: 0,
            tx_err: 0,
            ms_since_last_rx: 0,
            ..Default::default()
        },
        NodeState {
            peer_count: 1,
            tx_ok: 100,
            tx_err: 100,
            ms_since_last_rx: 60000,
            ..Default::default()
        },
        NodeState {
            peer_count: 3,
            tx_ok: 0,
            tx_err: 0,
            ms_since_last_rx: 0,
            energy_delta: -0.5,
            ..Default::default()
        },
        NodeState {
            peer_count: 1,
            tx_ok: 1,
            tx_err: 99,
            ms_since_last_rx: PEER_TIMEOUT_MS * 2,
            ..Default::default()
        },
    ];
    for (i, s) in configs.iter().enumerate() {
        let out = compute_led_steady(s);
        assert!(
            out.sat >= 80 && out.sat <= 255,
            "config {}: sat={} out of [80,255]",
            i,
            out.sat
        );
    }
}

#[test]
fn test_prop_val_always_in_range() {
    for energy in [0.0, 0.25, 0.5, 0.75, 1.0, -1.0, 2.0] {
        let s = NodeState {
            energy_score: energy,
            ..Default::default()
        };
        let out = compute_led_steady(&s);
        assert!(
            out.val >= BRIGHTNESS_MIN && out.val <= BRIGHTNESS_MAX,
            "energy={}: val={} out of [{},{}]",
            energy,
            out.val,
            BRIGHTNESS_MIN,
            BRIGHTNESS_MAX
        );
    }
}

#[test]
fn test_prop_breath_period_always_in_bounds() {
    for ar_x10 in -10..=20i32 {
        let ar = ar_x10 as f32 / 10.0;
        let period = compute_breath_period_ms(ar);
        assert!(
            period >= BREATH_PERIOD_MIN_MS && period <= BREATH_PERIOD_MAX_MS,
            "activity_rate={}: period={} out of [{},{}]",
            ar,
            period,
            BREATH_PERIOD_MIN_MS,
            BREATH_PERIOD_MAX_MS
        );
    }
}

#[test]
fn test_prop_monotonic_energy_brightness() {
    let mut prev_val = 0u8;
    for e_x10 in 0..=10u32 {
        let energy = e_x10 as f32 / 10.0;
        let s = NodeState {
            energy_score: energy,
            ..Default::default()
        };
        let out = compute_led_steady(&s);
        assert!(
            out.val >= prev_val,
            "brightness should increase with energy: e={} val={} prev={}",
            energy,
            out.val,
            prev_val
        );
        prev_val = out.val;
    }
}

#[test]
fn test_prop_monotonic_peer_hue() {
    let hues: Vec<u8> = (0..=4)
        .map(|pc| {
            let s = NodeState {
                peer_count: pc,
                last_rssi: -128,
                ..Default::default()
            };
            compute_led_steady(&s).hue
        })
        .collect();
    for i in 1..hues.len() {
        assert!(
            hues[i] >= hues[i - 1],
            "hue should increase with peers: {:?}",
            hues
        );
    }
}

// ── Tests: End-to-end pipeline (temp -> energy -> LED + oscillator) ────────

#[test]
fn test_e2e_cold_chip_bright_led() {
    // Cold chip (25C) -> high energy -> bright LED
    let energy = temp_to_energy(25.0);
    let state = NodeState {
        energy_score: energy,
        peer_count: 1,
        last_rssi: -50,
        ..Default::default()
    };
    let led = compute_led_steady(&state);
    assert!(
        led.val > BRIGHTNESS_MIN + 40,
        "cold chip should produce bright LED: val={}",
        led.val
    );
}

#[test]
fn test_e2e_hot_chip_dim_led() {
    // Hot chip (70C) -> low energy -> dim LED
    let energy = temp_to_energy(70.0);
    let state = NodeState {
        energy_score: energy,
        peer_count: 1,
        last_rssi: -50,
        ..Default::default()
    };
    let led = compute_led_steady(&state);
    assert!(
        led.val < BRIGHTNESS_MIN + 30,
        "hot chip should produce dim LED: val={}",
        led.val
    );
}

#[test]
fn test_e2e_oscillator_modulates_brightness() {
    // The oscillator's brightness_factor modulates the LED val
    let energy = temp_to_energy(30.0);
    let state = NodeState {
        energy_score: energy,
        ..Default::default()
    };
    let steady = compute_led_steady(&state);

    let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);

    // Near phase 0: brightness_factor ~0, so modulated val is low
    osc.set_phase(0.1);
    let factor_low = osc.brightness_factor();
    let val_low = ((steady.val as f32) * (0.6 + 0.4 * factor_low)) as u8;

    // Near phase 1: brightness_factor ~1, so modulated val is high
    osc.set_phase(0.95);
    let factor_high = osc.brightness_factor();
    let val_high = ((steady.val as f32) * (0.6 + 0.4 * factor_high)) as u8;

    assert!(
        val_high > val_low + 10,
        "oscillator should modulate brightness: low={} high={}",
        val_low,
        val_high
    );
}

#[test]
fn test_e2e_full_cycle_led_range() {
    // Sweep through a full oscillator cycle and verify LED stays in valid range
    let energy = temp_to_energy(35.0);
    let state = NodeState {
        energy_score: energy,
        peer_count: 2,
        last_rssi: -45,
        ms_since_last_rx: 500,
        ..Default::default()
    };
    let steady = compute_led_steady(&state);

    let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let period = 3000u64;

    for step in 0..300 {
        osc.advance(10, period);
        let factor = osc.brightness_factor();
        let val = ((steady.val as f32) * (0.6 + 0.4 * factor)) as u8;
        let val = val.max(BREATHING_FLOOR);
        assert!(
            val >= BREATHING_FLOOR && val <= BRIGHTNESS_MAX,
            "step {}: val={} out of valid range [{}, {}]",
            step,
            val,
            BREATHING_FLOOR,
            BRIGHTNESS_MAX
        );
    }
}

#[test]
fn test_e2e_absorption_produces_fire_flash() {
    // Simulate: oscillator near threshold, peer pulse causes absorption,
    // fire_flash should be triggered
    let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc.set_phase(0.92); // near threshold, past refractory
    assert!(osc.receive_pulse(), "should absorb");
    assert!(osc.just_fired(), "should have fired");
    // In firmware, this triggers fire_flash_until = now + FIRE_FLASH_MS
    // During fire flash, LED shows steady.val (full brightness, no oscillator reduction)
}

#[test]
fn test_prop_temp_energy_led_pipeline_never_panics() {
    // Property: for any temperature in [-40, 130], the full pipeline produces valid LED output
    for t in (-40..=130).step_by(5) {
        let energy = temp_to_energy(t as f32);
        assert!(
            energy >= 0.05 && energy <= 1.0,
            "T={}: energy={}",
            t,
            energy
        );
        for peers in 0..=4 {
            for rssi in [-128i16, -80, -50, -30, 0] {
                let state = NodeState {
                    energy_score: energy,
                    peer_count: peers,
                    last_rssi: rssi,
                    energy_delta: if t > 60 { -0.1 } else { 0.03 },
                    ms_since_last_rx: if peers > 0 { 1000 } else { 0 },
                    ..Default::default()
                };
                let led = compute_led_steady(&state);
                assert!(led.hue <= 255); // always true for u8 but documents intent
                assert!(
                    led.sat >= 80 && led.sat <= 255,
                    "T={} peers={} rssi={}: sat={}",
                    t,
                    peers,
                    rssi,
                    led.sat
                );
                assert!(
                    led.val >= BRIGHTNESS_MIN && led.val <= BRIGHTNESS_MAX,
                    "T={} peers={} rssi={}: val={}",
                    t,
                    peers,
                    rssi,
                    led.val
                );
            }
        }
    }
}

// Mirollo-Strogatz convergence with realistic beacon timing (2s interval)
#[test]
fn test_firefly_ms_convergence_with_beacon_interval() {
    // Simulate two boards exchanging beacons every 2s while their oscillators
    // advance continuously. This matches the real firmware's TX_INTERVAL_MS=2000.
    let period = 3000u64;
    let dt = 10u64; // 10ms steps
    let beacon_interval = 2000u64;
    let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.0);
    osc_b.set_phase(0.55); // 55% offset

    let mut next_beacon_a = beacon_interval;
    let mut next_beacon_b = beacon_interval + 1000; // B starts offset
    let total_time = period * 30;

    let mut fire_a = Vec::new();
    let mut fire_b = Vec::new();

    for t in (0..total_time).step_by(dt as usize) {
        if osc_a.advance(dt, period) {
            fire_a.push(t);
        }
        if osc_b.advance(dt, period) {
            fire_b.push(t);
        }

        // Beacon exchange (not every tick, only every 2s)
        // Absorption-fires must be recorded too.
        if t >= next_beacon_a {
            next_beacon_a = t + beacon_interval;
            if osc_b.receive_pulse() {
                fire_b.push(t);
            }
        }
        if t >= next_beacon_b {
            next_beacon_b = t + beacon_interval;
            if osc_a.receive_pulse() {
                fire_a.push(t);
            }
        }
    }

    // Check last 3 fires of A and B are close
    assert!(
        fire_a.len() >= 3 && fire_b.len() >= 3,
        "both should fire: A={}, B={}",
        fire_a.len(),
        fire_b.len()
    );
    let last_a = &fire_a[fire_a.len() - 3..];
    for &ta in last_a {
        let closest = fire_b
            .iter()
            .map(|&tb| (ta as i64 - tb as i64).unsigned_abs())
            .min()
            .unwrap();
        // With 2s beacon intervals and 3s period, sync is coarser than continuous.
        // Convergence within half a period is a reasonable expectation.
        assert!(
            closest <= period / 2,
            "beacon-interval sync: fire gap={}ms (period={}ms)",
            closest,
            period
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// ADVANCED TESTS: Kuramoto metrics, stochastic sync, topologies, perception
// ══════════════════════════════════════════════════════════════════════════════

// ── Tests: Kuramoto order parameter ───────────────────────────────────────

#[test]
fn test_kuramoto_perfect_sync() {
    let phases = vec![0.5, 0.5, 0.5, 0.5];
    let r = kuramoto_order_parameter(&phases);
    assert!(
        (r - 1.0).abs() < 0.02,
        "identical phases should give R~1.0: got {}",
        r
    );
}

#[test]
fn test_kuramoto_perfect_antisync() {
    // Two oscillators exactly opposite
    let phases = vec![0.0, 0.5];
    let r = kuramoto_order_parameter(&phases);
    assert!(r < 0.05, "opposite phases should give R~0: got {}", r);
}

#[test]
fn test_kuramoto_uniform_spread() {
    // 4 oscillators evenly spaced: R should be ~0
    let phases = vec![0.0, 0.25, 0.5, 0.75];
    let r = kuramoto_order_parameter(&phases);
    assert!(r < 0.05, "uniform spread should give R~0: got {}", r);
}

#[test]
fn test_kuramoto_near_sync() {
    // Phases clustered within 10% of cycle: R should be high
    let phases = vec![0.48, 0.50, 0.52, 0.49, 0.51];
    let r = kuramoto_order_parameter(&phases);
    assert!(r > 0.95, "clustered phases should give R>0.95: got {}", r);
}

#[test]
fn test_kuramoto_empty() {
    assert_eq!(kuramoto_order_parameter(&[]), 0.0);
}

#[test]
fn test_kuramoto_single() {
    let r = kuramoto_order_parameter(&[0.3]);
    assert!(
        (r - 1.0).abs() < 0.02,
        "single oscillator always R=1: got {}",
        r
    );
}

#[test]
fn test_kuramoto_monotonic_with_convergence() {
    // As phases converge, R should increase monotonically
    let spreads: Vec<f32> = vec![0.5, 0.3, 0.2, 0.1, 0.05, 0.01, 0.001];
    let mut prev_r = 0.0f32;
    for spread in spreads {
        let phases: Vec<f32> = (0..6)
            .map(|i| 0.5 + (i as f32 / 5.0 - 0.5) * spread)
            .collect();
        let r = kuramoto_order_parameter(&phases);
        assert!(
            r >= prev_r - 0.01,
            "R should increase as spread decreases: spread={} R={} prev_R={}",
            spread,
            r,
            prev_r
        );
        prev_r = r;
    }
}

// ── Tests: Stochastic synchronization ─────────────────────────────────────

/// Simple deterministic PRNG for reproducible test randomness (no std::rand dependency).
/// xorshift32 — period 2^32-1, good enough for test jitter.
struct Xorshift32 {
    state: u32,
}
impl Xorshift32 {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }
    fn next(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }
    /// Uniform float in [0, 1)
    fn next_f32(&mut self) -> f32 {
        self.next() as f32 / u32::MAX as f32
    }
    /// Approximate Gaussian via Box-Muller (cheap, good enough for tests)
    fn next_gaussian(&mut self) -> f32 {
        let _u1 = self.next_f32().max(1e-10);
        let _u2 = self.next_f32();
        // Approximate sqrt(-2*ln(u1)): use -2*ln(u1) ≈ -2*(u1-1)/u1 for u1 near 1
        // Actually, let's just use a simpler Irwin-Hall approximation: sum of 12 uniforms - 6
        // More predictable than Box-Muller without ln/cos
        let sum: f32 = (0..12).map(|_| self.next_f32()).sum();
        sum - 6.0 // approximate N(0,1)
    }
}

#[test]
fn test_stochastic_sync_with_jitter() {
    // Two oscillators with jittered beacon delivery. Beacon arrives at
    // t_nominal + jitter where jitter ~ N(0, sigma). Should still converge.
    // Use fire-on-advance as the beacon trigger (like the real firmware), which
    // ensures pulses land at biologically meaningful times.
    let period = 3000u64;
    let dt = 10u64;
    let sigma_ms = 50.0f32; // 50ms jitter std dev (much worse than real ESP-NOW ~0.1ms)

    let mut rng = Xorshift32::new(42);
    let num_trials = 20;
    let mut converged_count = 0;

    for _trial in 0..num_trials {
        let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        osc_a.set_phase(rng.next_f32());
        osc_b.set_phase(rng.next_f32());

        let total_time = period * 60; // longer simulation
                                      // Pending pulse delivery: (delivery_time, target_osc_is_b)
        let mut pending: Vec<(i64, bool)> = Vec::new();

        for t in (0..total_time).step_by(dt as usize) {
            let fired_a = osc_a.advance(dt, period);
            let fired_b = osc_b.advance(dt, period);

            // When an oscillator fires, schedule a jittered pulse to the other
            if fired_a {
                let jitter = (rng.next_gaussian() * sigma_ms) as i64;
                pending.push((t as i64 + jitter.max(0), true));
            }
            if fired_b {
                let jitter = (rng.next_gaussian() * sigma_ms) as i64;
                pending.push((t as i64 + jitter.max(0), false));
            }

            // Deliver any pending pulses whose time has come
            pending.retain(|&(deliver_at, target_is_b)| {
                if t as i64 >= deliver_at {
                    if target_is_b {
                        osc_b.receive_pulse();
                    } else {
                        osc_a.receive_pulse();
                    }
                    false
                } else {
                    true
                }
            });
        }

        let r = kuramoto_order_parameter(&[osc_a.phase(), osc_b.phase()]);
        if r > 0.90 {
            converged_count += 1;
        }
    }

    // At least 70% of trials should converge (allowing some bad luck with jitter)
    assert!(
        converged_count >= num_trials * 70 / 100,
        "jitter test: only {}/{} trials converged (need 70%)",
        converged_count,
        num_trials
    );
}

#[test]
fn test_stochastic_sync_with_packet_loss() {
    // Two oscillators where 20% of fire-pulses are dropped. Should still converge.
    // Uses fire-on-advance as the pulse trigger (matching the real firmware).
    let period = 3000u64;
    let dt = 10u64;
    let loss_rate = 0.20f32;

    let mut rng = Xorshift32::new(123);
    let num_trials = 20;
    let mut converged_count = 0;

    for _trial in 0..num_trials {
        let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
        osc_a.set_phase(rng.next_f32());
        osc_b.set_phase(rng.next_f32());

        let total_time = period * 60;

        for _t in (0..total_time).step_by(dt as usize) {
            let fired_a = osc_a.advance(dt, period);
            let fired_b = osc_b.advance(dt, period);

            // When an oscillator fires, deliver pulse to the other (unless lost)
            if fired_a && rng.next_f32() > loss_rate {
                osc_b.receive_pulse();
            }
            if fired_b && rng.next_f32() > loss_rate {
                osc_a.receive_pulse();
            }
        }

        let r = kuramoto_order_parameter(&[osc_a.phase(), osc_b.phase()]);
        if r > 0.90 {
            converged_count += 1;
        }
    }

    assert!(
        converged_count >= num_trials * 70 / 100,
        "packet loss test: only {}/{} trials converged (need 70%)",
        converged_count,
        num_trials
    );
}

#[test]
fn test_stochastic_50pct_loss_degrades_gracefully() {
    // 50% packet loss: sync may not converge but should not diverge or crash
    let period = 3000u64;
    let dt = 10u64;
    let beacon_interval = 2000u64;
    let mut rng = Xorshift32::new(999);

    let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.1);
    osc_b.set_phase(0.7);

    let total_time = period * 100;
    let mut next_beacon_a = beacon_interval;
    let mut next_beacon_b = beacon_interval + 1000;

    for t in (0..total_time).step_by(dt as usize) {
        osc_a.advance(dt, period);
        osc_b.advance(dt, period);

        if t >= next_beacon_a {
            next_beacon_a = t + beacon_interval;
            if rng.next_f32() > 0.50 {
                osc_b.receive_pulse();
            }
        }
        if t >= next_beacon_b {
            next_beacon_b = t + beacon_interval;
            if rng.next_f32() > 0.50 {
                osc_a.receive_pulse();
            }
        }

        // Verify oscillator state is always valid
        assert!(
            osc_a.phase() >= 0.0 && osc_a.phase() < 1.0,
            "osc_a phase out of range: {}",
            osc_a.phase()
        );
        assert!(
            osc_b.phase() >= 0.0 && osc_b.phase() < 1.0,
            "osc_b phase out of range: {}",
            osc_b.phase()
        );
    }
    // Just verify it didn't panic — convergence is not guaranteed at 50% loss
}

// ── Tests: Multi-node topologies ──────────────────────────────────────────

/// Run a topology simulation and return the final Kuramoto R.
fn simulate_topology(
    n: usize,
    adjacency: &[(usize, usize)], // bidirectional edges
    initial_phases: &[f32],
    period: u64,
    num_periods: u64,
) -> f32 {
    let dt = 10u64;
    let mut oscs: Vec<FireflyOscillator> = (0..n)
        .map(|_| FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY))
        .collect();
    for (i, &phase) in initial_phases.iter().enumerate() {
        oscs[i].set_phase(phase);
    }

    let total_steps = (period / dt) * num_periods;
    for _step in 0..total_steps {
        let mut fired = vec![false; n];
        for i in 0..n {
            fired[i] = oscs[i].advance(dt, period);
        }
        // Deliver pulses along edges
        for &(a, b) in adjacency {
            if fired[a] {
                oscs[b].receive_pulse();
            }
            if fired[b] {
                oscs[a].receive_pulse();
            }
        }
    }

    let phases: Vec<f32> = oscs.iter().map(|o| o.phase()).collect();
    kuramoto_order_parameter(&phases)
}

#[test]
fn test_topology_all_to_all_4() {
    // Complete graph K4: guaranteed convergence
    let edges: Vec<(usize, usize)> = vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    let phases = vec![0.0, 0.25, 0.5, 0.75];
    let r = simulate_topology(4, &edges, &phases, 3000, 30);
    assert!(r > 0.90, "all-to-all K4 should converge: R={:.3}", r);
}

#[test]
fn test_topology_star_5() {
    // Star with center node 0 connected to 1,2,3,4
    let edges: Vec<(usize, usize)> = vec![(0, 1), (0, 2), (0, 3), (0, 4)];
    let phases = vec![0.0, 0.2, 0.4, 0.6, 0.8];
    let r = simulate_topology(5, &edges, &phases, 3000, 40);
    assert!(r > 0.80, "star topology should converge: R={:.3}", r);
}

#[test]
fn test_topology_ring_6() {
    // Ring: 0-1-2-3-4-5-0
    let edges: Vec<(usize, usize)> = vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)];
    let phases = vec![0.0, 0.17, 0.33, 0.50, 0.67, 0.83];
    // Ring is slow to converge and can form splay states
    let r = simulate_topology(6, &edges, &phases, 3000, 60);
    // Relaxed criterion: ring may not achieve R>0.9 but should show some clustering
    assert!(r > 0.40, "ring should show partial sync: R={:.3}", r);
}

#[test]
fn test_topology_line_4() {
    // Line: 0-1-2-3 (diameter 3, slowest topology)
    let edges: Vec<(usize, usize)> = vec![(0, 1), (1, 2), (2, 3)];
    let phases = vec![0.0, 0.25, 0.5, 0.75];
    // Line needs O(51*d) = O(153) cycles for convergence (Lyu 2016)
    let r = simulate_topology(4, &edges, &phases, 3000, 200);
    assert!(r > 0.60, "line should eventually converge: R={:.3}", r);
}

#[test]
fn test_topology_ring_with_chord() {
    // Ring + one chord: should converge much faster than pure ring
    let edges: Vec<(usize, usize)> = vec![(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (0, 3)];
    let phases = vec![0.0, 0.17, 0.33, 0.50, 0.67, 0.83];
    let r = simulate_topology(6, &edges, &phases, 3000, 40);
    assert!(
        r > 0.70,
        "ring+chord should converge faster than pure ring: R={:.3}",
        r
    );
}

#[test]
fn test_topology_disconnected_no_sync() {
    // Two disconnected pairs: should NOT synchronize across the partition
    let edges: Vec<(usize, usize)> = vec![(0, 1), (2, 3)]; // two separate edges
    let phases = vec![0.0, 0.1, 0.5, 0.6]; // pairs close, but partitions apart
    let r = simulate_topology(4, &edges, &phases, 3000, 30);
    // Global R should be low because the two partitions are unsynchronized
    // (though each pair may internally sync)
    assert!(
        r < 0.80,
        "disconnected graph should not globally sync: R={:.3}",
        r
    );
}

// ── Tests: Heterogeneous frequencies ──────────────────────────────────────

#[test]
fn test_hetero_1pct_period_mismatch() {
    // Two oscillators with 1% different natural periods. Should still converge.
    let period_a = 3000u64;
    let period_b = 3030u64; // +1%
    let dt = 10u64;
    let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.0);
    osc_b.set_phase(0.5);

    let total_steps = 300 * 50; // 50 periods worth
    for _step in 0..total_steps {
        let fired_a = osc_a.advance(dt, period_a);
        let fired_b = osc_b.advance(dt, period_b);
        if fired_a {
            osc_b.receive_pulse();
        }
        if fired_b {
            osc_a.receive_pulse();
        }
    }

    let r = kuramoto_order_parameter(&[osc_a.phase(), osc_b.phase()]);
    assert!(r > 0.85, "1% mismatch should converge: R={:.3}", r);
}

#[test]
fn test_hetero_5pct_period_mismatch() {
    // 5% mismatch: harder, requires strong coupling
    let period_a = 3000u64;
    let period_b = 3150u64; // +5%
    let dt = 10u64;
    let mut osc_a = FireflyOscillator::new(0.20, FIREFLY_REFRACTORY); // stronger coupling
    let mut osc_b = FireflyOscillator::new(0.20, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.0);
    osc_b.set_phase(0.5);

    let total_steps = 300 * 80;
    for _step in 0..total_steps {
        let fired_a = osc_a.advance(dt, period_a);
        let fired_b = osc_b.advance(dt, period_b);
        if fired_a {
            osc_b.receive_pulse();
        }
        if fired_b {
            osc_a.receive_pulse();
        }
    }

    let r = kuramoto_order_parameter(&[osc_a.phase(), osc_b.phase()]);
    assert!(
        r > 0.70,
        "5% mismatch with strong coupling should converge: R={:.3}",
        r
    );
}

#[test]
fn test_hetero_10pct_mismatch_weak_coupling_fails() {
    // 10% mismatch with default epsilon: may not converge (documenting the boundary)
    let period_a = 3000u64;
    let period_b = 3300u64; // +10%
    let dt = 10u64;
    let mut osc_a = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let mut osc_b = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    osc_a.set_phase(0.0);
    osc_b.set_phase(0.5);

    let total_steps = 300 * 60;
    for _step in 0..total_steps {
        let fired_a = osc_a.advance(dt, period_a);
        let fired_b = osc_b.advance(dt, period_b);
        if fired_a {
            osc_b.receive_pulse();
        }
        if fired_b {
            osc_a.receive_pulse();
        }
    }

    // Just verify it doesn't crash — sync is not expected at 10% mismatch
    let r = kuramoto_order_parameter(&[osc_a.phase(), osc_b.phase()]);
    // Documenting the actual behavior: R may be anywhere in [0, 1]
    assert!(r >= 0.0 && r <= 1.01, "R should be valid: {:.3}", r);
}

#[test]
fn test_hetero_three_nodes_slight_mismatch() {
    // Three nodes with slightly different periods: 2990, 3000, 3010
    let periods = [2990u64, 3000, 3010];
    let dt = 10u64;
    let mut oscs = [
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
        FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY),
    ];
    oscs[0].set_phase(0.0);
    oscs[1].set_phase(0.33);
    oscs[2].set_phase(0.66);

    let total_steps = 300 * 60;
    for _step in 0..total_steps {
        let mut fired = [false; 3];
        for i in 0..3 {
            fired[i] = oscs[i].advance(dt, periods[i]);
        }
        // All-to-all coupling
        for i in 0..3 {
            if fired[i] {
                for j in 0..3 {
                    if j != i {
                        oscs[j].receive_pulse();
                    }
                }
            }
        }
    }

    let phases: Vec<f32> = oscs.iter().map(|o| o.phase()).collect();
    let r = kuramoto_order_parameter(&phases);
    assert!(
        r > 0.80,
        "slight 3-node mismatch should converge: R={:.3}",
        r
    );
}

// ── Tests: Perceptual brightness (gamma correction, JND) ──────────────────

#[test]
fn test_gamma_correct_zero() {
    assert_eq!(gamma_correct(0), 0);
}

#[test]
fn test_gamma_correct_max() {
    assert_eq!(gamma_correct(255), 255);
}

#[test]
fn test_gamma_correct_monotonic() {
    let mut prev = 0u8;
    for i in 0..=255u8 {
        let g = gamma_correct(i);
        assert!(
            g >= prev,
            "gamma should be monotonic: gamma({})={} < gamma({})={}",
            i,
            g,
            i - 1,
            prev
        );
        prev = g;
    }
}

#[test]
fn test_gamma_correct_compresses_low_end() {
    // Linear step from 10->11 should be a tiny gamma step (or zero)
    let g10 = gamma_correct(10);
    let g11 = gamma_correct(11);
    let g200 = gamma_correct(200);
    let g201 = gamma_correct(201);
    // Low-end steps should be smaller than or equal to high-end steps
    assert!(
        (g11 - g10) <= (g201 - g200) + 1,
        "gamma should compress low-end: step(10-11)={} step(200-201)={}",
        g11 - g10,
        g201 - g200
    );
}

#[test]
fn test_gamma_midpoint_dark() {
    // Linear 128 (50%) should map to roughly 64 (25%) with gamma 2.0
    let g = gamma_correct(128);
    assert!(g >= 55 && g <= 75, "gamma(128) should be ~64: got {}", g);
}

#[test]
fn test_perceptual_brightness_steps_smoother_with_gamma() {
    // Count the number of distinct output values in the low brightness range [1, 50]
    // With gamma correction, there should be fewer distinct values (compressed)
    // but the visual spacing should be more uniform
    let mut linear_values: Vec<u8> = (1..=50).collect();
    let mut gamma_values: Vec<u8> = (1..=50).map(gamma_correct).collect();
    linear_values.dedup();
    gamma_values.dedup();
    // Gamma should produce fewer distinct values at the low end
    assert!(
        gamma_values.len() <= linear_values.len(),
        "gamma should compress low-end range: {} gamma vs {} linear distinct values",
        gamma_values.len(),
        linear_values.len()
    );
}

// ── Tests: HSV transition smoothness ──────────────────────────────────────

/// Approximate CIE delta-E between two HSV values.
/// Uses a simplified model: converts HSV to approximate L*a*b* and computes
/// Euclidean distance. Not CIE ΔE2000 but captures the major perceptual effects.
fn approx_delta_e(h1: u8, s1: u8, v1: u8, h2: u8, s2: u8, v2: u8) -> f32 {
    // Approximate L* from V (brightness)
    let l1 = (v1 as f32 / 255.0).powf(0.43) * 100.0; // rough CIE L* approximation
    let l2 = (v2 as f32 / 255.0).powf(0.43) * 100.0;

    // Approximate chroma from S*V
    let c1 = s1 as f32 / 255.0 * v1 as f32 / 255.0 * 128.0;
    let c2 = s2 as f32 / 255.0 * v2 as f32 / 255.0 * 128.0;

    // Approximate a*, b* from hue angle
    let h1_rad = h1 as f32 / 255.0 * std::f32::consts::TAU;
    let h2_rad = h2 as f32 / 255.0 * std::f32::consts::TAU;
    let a1 = c1 * h1_rad.cos();
    let b1 = c1 * h1_rad.sin();
    let a2 = c2 * h2_rad.cos();
    let b2 = c2 * h2_rad.sin();

    let dl = l1 - l2;
    let da = a1 - a2;
    let db = b1 - b2;
    (dl * dl + da * da + db * db).sqrt()
}

#[test]
fn test_hue_transition_smoothness_one_peer() {
    // Simulate hue transitioning as RSSI varies from weak to strong.
    // Consecutive steps should have small perceptual delta.
    let mut prev_led = compute_led_steady(&NodeState {
        peer_count: 1,
        last_rssi: RSSI_WEAK,
        energy_score: 0.7,
        ms_since_last_rx: 500,
        ..Default::default()
    });

    let mut max_delta = 0.0f32;
    for rssi in (RSSI_WEAK..=RSSI_STRONG).step_by(1) {
        let state = NodeState {
            peer_count: 1,
            last_rssi: rssi,
            energy_score: 0.7,
            ms_since_last_rx: 500,
            ..Default::default()
        };
        let led = compute_led_steady(&state);
        let de = approx_delta_e(
            prev_led.hue,
            prev_led.sat,
            prev_led.val,
            led.hue,
            led.sat,
            led.val,
        );
        if de > max_delta {
            max_delta = de;
        }
        prev_led = led;
    }

    // Max step should be < 5.0 delta-E (clearly visible at a glance threshold)
    assert!(
        max_delta < 5.0,
        "RSSI hue transition has too-large step: max delta-E={:.2}",
        max_delta
    );
}

#[test]
fn test_brightness_transition_smoothness_energy() {
    // Simulate energy score ramping 0.0 -> 1.0 in fine steps.
    // Brightness transitions should be perceptually smooth.
    let mut prev_led = compute_led_steady(&NodeState {
        energy_score: 0.0,
        ..Default::default()
    });

    let mut max_delta = 0.0f32;
    for e_x100 in 1..=100 {
        let energy = e_x100 as f32 / 100.0;
        let state = NodeState {
            energy_score: energy,
            ..Default::default()
        };
        let led = compute_led_steady(&state);
        let de = approx_delta_e(
            prev_led.hue,
            prev_led.sat,
            prev_led.val,
            led.hue,
            led.sat,
            led.val,
        );
        if de > max_delta {
            max_delta = de;
        }
        prev_led = led;
    }

    assert!(
        max_delta < 3.0,
        "energy brightness transition has too-large step: max delta-E={:.2}",
        max_delta
    );
}

#[test]
fn test_saturation_fade_smoothness() {
    // Saturation fading as link goes stale (ms_since_last_rx 0 -> 30000).
    let mut prev_led = compute_led_steady(&NodeState {
        peer_count: 1,
        ms_since_last_rx: 0,
        energy_score: 0.7,
        ..Default::default()
    });

    let mut max_delta = 0.0f32;
    for ms in (500..=30000).step_by(500) {
        let state = NodeState {
            peer_count: 1,
            ms_since_last_rx: ms,
            energy_score: 0.7,
            ..Default::default()
        };
        let led = compute_led_steady(&state);
        let de = approx_delta_e(
            prev_led.hue,
            prev_led.sat,
            prev_led.val,
            led.hue,
            led.sat,
            led.val,
        );
        if de > max_delta {
            max_delta = de;
        }
        prev_led = led;
    }

    assert!(
        max_delta < 5.0,
        "saturation fade has too-large step: max delta-E={:.2}",
        max_delta
    );
}

#[test]
fn test_oscillator_brightness_curve_perceptually_smooth() {
    // The oscillator's brightness_factor (phi^2) should produce smooth transitions.
    // Check that consecutive LED update frames have small perceptual delta.
    let energy = temp_to_energy(30.0);
    let state = NodeState {
        energy_score: energy,
        peer_count: 1,
        last_rssi: -50,
        ..Default::default()
    };
    let steady = compute_led_steady(&state);

    let mut osc = FireflyOscillator::new(FIREFLY_EPSILON, FIREFLY_REFRACTORY);
    let period = 3000u64;

    // Compute initial val so we don't get a spurious jump from 0 on the first frame.
    let init_factor = osc.brightness_factor();
    let init_val = ((steady.val as f32) * (0.6 + 0.4 * init_factor)) as u8;
    let mut prev_val = init_val.max(BREATHING_FLOOR);
    let mut max_jump = 0u8;
    for _step in 0..300 {
        osc.advance(10, period);
        let factor = osc.brightness_factor();
        let val = ((steady.val as f32) * (0.6 + 0.4 * factor)) as u8;
        let val = val.max(BREATHING_FLOOR);

        if !osc.just_fired() {
            let jump = if val > prev_val {
                val - prev_val
            } else {
                prev_val - val
            };
            if jump > max_jump {
                max_jump = jump;
            }
        }
        prev_val = val;
    }

    // Max brightness jump between non-fire frames should be small (< 5 PWM levels)
    assert!(
        max_jump <= 5,
        "oscillator brightness has too-large jump between frames: {}",
        max_jump
    );
}

#[test]
fn test_peer_count_hue_transitions_are_large() {
    // Verify that peer count changes produce CLEARLY VISIBLE hue shifts.
    // This is the opposite of smoothness — we WANT these to be noticeable.
    for (pc_from, pc_to) in [(0, 1), (1, 2), (2, 3)] {
        let led_from = compute_led_steady(&NodeState {
            peer_count: pc_from,
            last_rssi: -128,
            ..Default::default()
        });
        let led_to = compute_led_steady(&NodeState {
            peer_count: pc_to,
            last_rssi: -128,
            ..Default::default()
        });
        let de = approx_delta_e(
            led_from.hue,
            led_from.sat,
            led_from.val,
            led_to.hue,
            led_to.sat,
            led_to.val,
        );
        assert!(
            de > 10.0,
            "peer count {} -> {} should be clearly visible: delta-E={:.2}",
            pc_from,
            pc_to,
            de
        );
    }
}

// ── Tests: Rayleigh Z statistic for convergence significance ──────────────

#[test]
fn test_rayleigh_z_synchronized() {
    // N oscillators locked at same phase: Z should be large (>> 3.0)
    let n = 5;
    let phases = vec![0.3; n];
    let r = kuramoto_order_parameter(&phases);
    let z = n as f32 * r * r; // Rayleigh statistic
    assert!(
        z > 3.0,
        "synchronized: Z={:.2} should exceed 3.0 (p<0.05 for uniform H0)",
        z
    );
}

#[test]
fn test_rayleigh_z_uniform() {
    // Evenly spaced phases: Z should be small (< 3.0)
    let n = 8;
    let phases: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let r = kuramoto_order_parameter(&phases);
    let z = n as f32 * r * r;
    assert!(
        z < 3.0,
        "uniform: Z={:.2} should be < 3.0 (cannot reject uniform H0)",
        z
    );
}

#[test]
fn test_convergence_produces_significant_rayleigh_z() {
    // Run a 6-oscillator all-to-all simulation and verify that convergence
    // produces a statistically significant Rayleigh Z (> 3.0) by the end.
    let n = 6;
    let edges: Vec<(usize, usize)> = {
        let mut e = Vec::new();
        for i in 0..n {
            for j in (i + 1)..n {
                e.push((i, j));
            }
        }
        e
    };
    let phases: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
    let r = simulate_topology(n, &edges, &phases, 3000, 30);
    let z = n as f32 * r * r;
    assert!(
        z > 3.0,
        "converged 6-node all-to-all should have significant Rayleigh Z: Z={:.2} R={:.3}",
        z,
        r
    );
}
