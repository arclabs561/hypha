//! Boot-time WiFi-environment fingerprinting.
//!
//! This does not assign a room label. It compares this boot's visible AP set
//! and RSSI profile with the previous boot and reports whether the board likely
//! moved. Infra owns room names and operator confirmation.

pub type PackedBssid = u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ap {
    pub bssid: PackedBssid,
    pub rssi: i8,
}

#[derive(Clone, Copy)]
pub struct Cfg {
    pub min_aps: usize,
    pub max_jaccard_keep: f32,
    pub shift_db: i32,
    pub min_shift_frac: f32,
}

pub const DEFAULT: Cfg = Cfg {
    min_aps: 3,
    max_jaccard_keep: 0.5,
    shift_db: 12,
    min_shift_frac: 0.6,
};

pub const MAX_STORED_APS: usize = 16;
pub const STORAGE_MAX_BYTES: usize = 1 + MAX_STORED_APS * 9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    pub moved: bool,
    pub inconclusive: bool,
    pub jaccard_similarity: f32,
    pub common: usize,
    pub shifted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NoBaseline,
    Stable,
    Moved,
    Inconclusive,
    ScanError,
    StoreError,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::NoBaseline => "no_baseline",
            State::Stable => "stable",
            State::Moved => "moved",
            State::Inconclusive => "inconclusive",
            State::ScanError => "scan_error",
            State::StoreError => "store_error",
        }
    }

    pub fn code(self) -> u8 {
        match self {
            State::NoBaseline => 0,
            State::Stable => 1,
            State::Moved => 2,
            State::Inconclusive => 3,
            State::ScanError => 4,
            State::StoreError => 5,
        }
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            1 => State::Stable,
            2 => State::Moved,
            3 => State::Inconclusive,
            4 => State::ScanError,
            5 => State::StoreError,
            _ => State::NoBaseline,
        }
    }
}

fn bssids(aps: &[Ap]) -> Vec<PackedBssid> {
    let mut v: Vec<PackedBssid> = aps.iter().map(|ap| ap.bssid).collect();
    v.sort_unstable();
    v.dedup();
    v
}

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

    let inter = pset
        .iter()
        .filter(|b| nset.binary_search(b).is_ok())
        .count();
    let union = pset.len() + nset.len() - inter;
    let jaccard = inter as f32 / union as f32;

    let now_rssi = |b: PackedBssid| now.iter().find(|ap| ap.bssid == b).map(|ap| ap.rssi);
    let mut shifted = 0;
    for ap in prev {
        if let Some(nr) = now_rssi(ap.bssid) {
            if (ap.rssi as i32 - nr as i32).abs() >= cfg.shift_db {
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

pub fn verdict_state(v: &Verdict) -> State {
    if v.inconclusive {
        State::Inconclusive
    } else if v.moved {
        State::Moved
    } else {
        State::Stable
    }
}

pub fn pack_bssid(bytes: [u8; 6]) -> PackedBssid {
    bytes.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

pub fn select_stored(mut aps: Vec<Ap>) -> Vec<Ap> {
    aps.sort_by_key(|ap| -i16::from(ap.rssi));
    aps.truncate(MAX_STORED_APS);
    aps.sort_by_key(|ap| ap.bssid);
    aps
}

pub fn encode(aps: &[Ap]) -> Vec<u8> {
    let aps = select_stored(aps.to_vec());
    let mut out = Vec::with_capacity(1 + aps.len() * 9);
    out.push(aps.len() as u8);
    for ap in aps {
        out.extend_from_slice(&ap.bssid.to_be_bytes());
        out.push(ap.rssi as u8);
    }
    out
}

pub fn decode(bytes: &[u8]) -> Option<Vec<Ap>> {
    let (&count, rest) = bytes.split_first()?;
    let count = count as usize;
    if count > MAX_STORED_APS || rest.len() != count * 9 {
        return None;
    }
    let mut aps = Vec::with_capacity(count);
    for chunk in rest.chunks_exact(9) {
        let mut bssid = [0u8; 8];
        bssid.copy_from_slice(&chunk[..8]);
        aps.push(Ap {
            bssid: u64::from_be_bytes(bssid),
            rssi: chunk[8] as i8,
        });
    }
    Some(aps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Vec<Ap> {
        vec![
            Ap {
                bssid: 0x01,
                rssi: -40,
            },
            Ap {
                bssid: 0x02,
                rssi: -55,
            },
            Ap {
                bssid: 0x03,
                rssi: -60,
            },
            Ap {
                bssid: 0x04,
                rssi: -67,
            },
            Ap {
                bssid: 0x05,
                rssi: -72,
            },
        ]
    }

    #[test]
    fn identical_scan_is_not_a_move() {
        assert_eq!(
            verdict_state(&evaluate(&home(), &home(), DEFAULT)),
            State::Stable
        );
    }

    #[test]
    fn small_jitter_is_not_a_move() {
        let now: Vec<Ap> = home()
            .iter()
            .map(|ap| Ap {
                bssid: ap.bssid,
                rssi: ap.rssi + 2,
            })
            .collect();
        assert_eq!(
            verdict_state(&evaluate(&home(), &now, DEFAULT)),
            State::Stable
        );
    }

    #[test]
    fn wholesale_rssi_shift_is_a_move() {
        let now: Vec<Ap> = home()
            .iter()
            .map(|ap| Ap {
                bssid: ap.bssid,
                rssi: ap.rssi - 15,
            })
            .collect();
        let v = evaluate(&home(), &now, DEFAULT);
        assert!(
            v.moved && v.shifted >= 3,
            "wholesale shift should flag: {v:?}"
        );
    }

    #[test]
    fn different_ap_population_is_a_move() {
        let elsewhere = vec![
            Ap {
                bssid: 0x11,
                rssi: -45,
            },
            Ap {
                bssid: 0x12,
                rssi: -50,
            },
            Ap {
                bssid: 0x13,
                rssi: -58,
            },
            Ap {
                bssid: 0x14,
                rssi: -64,
            },
        ];
        let v = evaluate(&home(), &elsewhere, DEFAULT);
        assert!(
            v.moved && v.jaccard_similarity == 0.0,
            "new APs should flag: {v:?}"
        );
    }

    #[test]
    fn one_deviant_ap_is_not_a_move() {
        let mut now = home();
        now[0].rssi -= 30;
        assert_eq!(
            verdict_state(&evaluate(&home(), &now, DEFAULT)),
            State::Stable
        );
    }

    #[test]
    fn sparse_scan_is_inconclusive_not_a_move() {
        let v = evaluate(
            &[Ap {
                bssid: 0x01,
                rssi: -40,
            }],
            &[Ap {
                bssid: 0x99,
                rssi: -50,
            }],
            DEFAULT,
        );
        assert!(
            !v.moved && v.inconclusive,
            "too few APs must be inconclusive: {v:?}"
        );
    }

    #[test]
    fn storage_round_trip_bounds_and_sorts() {
        let aps = (0..20)
            .map(|i| Ap {
                bssid: i,
                rssi: -80 + i as i8,
            })
            .collect::<Vec<_>>();
        let encoded = encode(&aps);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.len(), MAX_STORED_APS);
        assert_eq!(decoded[0].bssid, 4);
        assert_eq!(decoded.last().unwrap().bssid, 19);
    }

    #[test]
    fn malformed_storage_is_rejected() {
        assert!(decode(&[]).is_none());
        assert!(decode(&[2, 1, 2, 3]).is_none());
        assert!(decode(&[255]).is_none());
    }
}
