//! Boot-time WiFi-environment delta: a cheap, immediate "did this board move?"
//! self-signal: the future firmware signal that closes the RF-baseline
//! detector's hours-of-latency gap for the unplug-replug case.
//!
//! At boot the board already scans for APs to associate. Hashing that scan into
//! a fingerprint and comparing it to the last-boot fingerprint (NVS-persisted)
//! lets a board that woke up somewhere new flag itself in its boot event within
//! seconds, instead of waiting ~half a day for the downstream fusion service's
//! sliding baseline to cross threshold. It is ADDITIVE to the downstream
//! RF-baseline conjunction detector, never a replacement: a false positive here
//! just corroborates, and the operator-confirmed quarantine path is unchanged.
//!
//! This is the pure decision core. It has no ESP-IDF dependency, so it is
//! host-tested here; at 0.17.0 it lifts into `hypha-core` and the firmware feeds
//! it a real `esp_wifi_scan` AP list + an NVS-stored prior. The thresholds are
//! the same shape as the downstream conjunction rule (a real move shifts MANY
//! APs together; one deviant AP is noise), kept deliberately conservative so a
//! self-flag never fires on ordinary RF churn.

/// One scanned access point: BSSID packed big-endian into the low 48 bits, and
/// its RSSI in dBm. BSSID (not SSID) so co-located APs sharing an SSID stay
/// distinct, and a hidden/renamed SSID does not perturb the fingerprint.
pub type Ap = (u64, i8);

/// Tunables (defaults reasoned from the path-loss literature + the detector's
/// margins; calibrate against the deployment's first real boot deltas at 0.17.0).
#[derive(Clone, Copy)]
pub struct Cfg {
    /// Need at least this many APs in BOTH scans for a verdict; a sparse scan
    /// (RF-quiet boot, antenna warmup) is inconclusive, never a move.
    pub min_aps: usize,
    /// Fraction of the prior's APs that must vanish/appear to call it a move on
    /// set-change alone (a relocated board sees a different AP population).
    pub max_jaccard_keep: f32,
    /// Per-AP RSSI shift (dB) counted as "shifted" on a common AP.
    pub shift_db: i32,
    /// Fraction of COMMON APs that must shift for an RSSI-conjunction move (the
    /// conjunction min_frac analogue: many links move together, not one).
    pub min_shift_frac: f32,
}

pub const DEFAULT: Cfg = Cfg {
    min_aps: 3,
    max_jaccard_keep: 0.5,
    shift_db: 12,
    min_shift_frac: 0.6,
};

/// Decision breakdown (returned so the boot event can carry the evidence, the
/// same "present the link evidence, operator disambiguates" stance as the detector).
#[derive(Debug, PartialEq)]
pub struct Verdict {
    pub moved: bool,
    pub inconclusive: bool,
    pub jaccard_similarity: f32, // |prev ∩ now| / |prev ∪ now|
    pub common: usize,
    pub shifted: usize,
}

fn bssids(aps: &[Ap]) -> Vec<u64> {
    let mut v: Vec<u64> = aps.iter().map(|&(b, _)| b).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Compare a fresh boot scan against the last-boot fingerprint.
pub fn evaluate(prev: &[Ap], now: &[Ap], cfg: Cfg) -> Verdict {
    let pset = bssids(prev);
    let nset = bssids(now);
    if pset.len() < cfg.min_aps || nset.len() < cfg.min_aps {
        return Verdict {
            moved: false,
            inconclusive: true,
            jaccard_similarity: 1.0,
            common: 0,
            shifted: 0,
        };
    }

    // Set-change leg: Jaccard similarity of the BSSID populations.
    let inter = pset
        .iter()
        .filter(|b| nset.binary_search(b).is_ok())
        .count();
    let union = pset.len() + nset.len() - inter;
    let jaccard = inter as f32 / union as f32;

    // RSSI-conjunction leg: on APs present in both scans, how many shifted a lot.
    let now_rssi = |b: u64| now.iter().find(|&&(x, _)| x == b).map(|&(_, r)| r);
    let mut shifted = 0;
    for &(b, pr) in prev {
        if let Some(nr) = now_rssi(b) {
            if (pr as i32 - nr as i32).abs() >= cfg.shift_db {
                shifted += 1;
            }
        }
    }
    let set_moved = jaccard < cfg.max_jaccard_keep;
    let rssi_moved = inter > 0 && (shifted as f32) >= cfg.min_shift_frac * inter as f32;

    Verdict {
        moved: set_moved || rssi_moved,
        inconclusive: false,
        jaccard_similarity: jaccard,
        common: inter,
        shifted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Five stable APs the board normally sees from its fixed spot.
    fn home() -> Vec<Ap> {
        vec![
            (0x01, -40),
            (0x02, -55),
            (0x03, -60),
            (0x04, -67),
            (0x05, -72),
        ]
    }

    #[test]
    fn identical_scan_is_not_a_move() {
        assert!(!evaluate(&home(), &home(), DEFAULT).moved);
    }

    #[test]
    fn small_jitter_is_not_a_move() {
        let now: Vec<Ap> = home().iter().map(|&(b, r)| (b, r + 2)).collect();
        let v = evaluate(&home(), &now, DEFAULT);
        assert!(!v.moved, "±2 dB jitter must not flag: {v:?}");
    }

    #[test]
    fn wholesale_rssi_shift_is_a_move() {
        // Relocation within the same AP population: every common AP drops ~15 dB.
        let now: Vec<Ap> = home().iter().map(|&(b, r)| (b, r - 15)).collect();
        let v = evaluate(&home(), &now, DEFAULT);
        assert!(
            v.moved && v.shifted >= 3,
            "wholesale shift should flag: {v:?}"
        );
    }

    #[test]
    fn different_ap_population_is_a_move() {
        let elsewhere = vec![(0x11, -45), (0x12, -50), (0x13, -58), (0x14, -64)];
        let v = evaluate(&home(), &elsewhere, DEFAULT);
        assert!(
            v.moved && v.jaccard_similarity == 0.0,
            "new APs should flag: {v:?}"
        );
    }

    #[test]
    fn half_the_aps_changed_is_a_move() {
        // Jaccard = 2 common / (5 ∪ extras) — drops below 0.5 keep threshold.
        let now = vec![
            (0x01, -41),
            (0x02, -54),
            (0x21, -50),
            (0x22, -55),
            (0x23, -60),
        ];
        let v = evaluate(&home(), &now, DEFAULT);
        assert!(v.moved, "AP-population turnover should flag: {v:?}");
    }

    #[test]
    fn one_deviant_ap_is_not_a_move() {
        // A single AP swings hard (it got moved / powered off), the rest hold:
        // conjunction guard keeps this below quorum, like the downstream detector.
        let mut now = home();
        now[0].1 -= 30;
        let v = evaluate(&home(), &now, DEFAULT);
        assert!(!v.moved, "single deviant AP must not flag a move: {v:?}");
    }

    #[test]
    fn sparse_scan_is_inconclusive_not_a_move() {
        let v = evaluate(&[(0x01, -40)], &[(0x99, -50)], DEFAULT);
        assert!(
            !v.moved && v.inconclusive,
            "too few APs must be inconclusive: {v:?}"
        );
    }
}
