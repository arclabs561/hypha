//! Scan-window desynchronization (DESYNC; Degesys et al., SenSys 2007).
//!
//! The firmware's firefly oscillator (`firefly.rs`) runs Mirollo-Strogatz with
//! ATTRACTIVE coupling, so the boards' heartbeats SYNCHRONISE. For the LED that
//! is a pretty liveness flash, but for the actual job — passive BLE scanning —
//! synchrony is the wrong objective: if all vantages scan at the same instant
//! they share blind spots, and an intermittent advertiser seen by no one in that
//! window is missed by the whole fleet. The dual algorithm, DESYNC, spreads N
//! nodes to EVEN phase spacing (1/N apart) with the same one-pulse-per-cycle
//! observation, so the fleet's scan windows interleave and temporal coverage of
//! bursty advertisers improves. This is the "flip the firefly" finding made
//! precise: DESYNC is not inverted M-S, it is the midpoint-jump rule below.
//!
//! Pure decision core, host-tested. At 0.17.0 this rides into hypha-core beside
//! the firefly and the firmware chooses coupling by role (LED keeps sync if
//! desired; the scan scheduler uses desync). Kept out of the OTA rollout.

/// One DESYNC round on phases in [0,1) (one point on the unit cycle per node).
/// Each node jumps a fraction `alpha` toward the midpoint of its two circular
/// phase-neighbours (the nodes that fire just before and just after it) — the
/// Degesys update. Returned phases are re-wrapped into [0,1). Order-independent:
/// it sorts internally, so callers may pass phases in any order.
pub fn desync_round(phases: &[f32], alpha: f32) -> Vec<f32> {
    let n = phases.len();
    if n < 2 {
        return phases.to_vec();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| phases[i].partial_cmp(&phases[j]).unwrap());
    let sorted: Vec<f32> = order.iter().map(|&i| wrap01(phases[i])).collect();

    let mut next = vec![0.0_f32; n];
    for k in 0..n {
        // Circular neighbours: the predecessor wraps below 0, the successor above 1,
        // so the midpoint is computed on a continuous line then re-wrapped.
        let prev = if k == 0 {
            sorted[n - 1] - 1.0
        } else {
            sorted[k - 1]
        };
        let succ = if k == n - 1 {
            sorted[0] + 1.0
        } else {
            sorted[k + 1]
        };
        let mid = 0.5 * (prev + succ);
        let moved = (1.0 - alpha) * sorted[k] + alpha * mid;
        next[order[k]] = wrap01(moved);
    }
    next
}

fn wrap01(x: f32) -> f32 {
    let r = x % 1.0;
    if r < 0.0 {
        r + 1.0
    } else {
        r
    }
}

/// Largest minus smallest circular gap between adjacent phases. 0 means perfectly
/// even spacing (the DESYNC fixed point); large means clustered.
pub fn gap_spread(phases: &[f32]) -> f32 {
    let n = phases.len();
    if n < 2 {
        return 0.0;
    }
    let mut p: Vec<f32> = phases.iter().map(|&x| wrap01(x)).collect();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut min_g = f32::MAX;
    let mut max_g = 0.0_f32;
    for i in 0..n {
        let g = if i == n - 1 {
            p[0] + 1.0 - p[i]
        } else {
            p[i + 1] - p[i]
        };
        min_g = min_g.min(g);
        max_g = max_g.max(g);
    }
    max_g - min_g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clustered_nodes_spread_to_even_spacing() {
        // Four boards all scanning in nearly the same window (the failure DESYNC
        // fixes): heavily clustered near phase 0.
        let mut phases = vec![0.00, 0.04, 0.08, 0.12];
        let before = gap_spread(&phases);
        for _ in 0..60 {
            phases = desync_round(&phases, 0.5);
        }
        let after = gap_spread(&phases);
        assert!(
            after < 0.02 && after < before,
            "expected near-even (1/4) spacing; spread {before:.3} -> {after:.3}"
        );
    }

    #[test]
    fn even_spacing_is_a_fixed_point() {
        // Already 1/3 apart: a round should not perturb it (within float noise).
        let phases = vec![0.0, 1.0 / 3.0, 2.0 / 3.0];
        let next = desync_round(&phases, 0.5);
        assert!(
            gap_spread(&next) < 1e-3,
            "even spacing must be stable: {next:?}"
        );
    }

    #[test]
    fn two_nodes_go_antiphase() {
        // The N=2 case: coverage is maximised at half a period apart.
        let mut phases = vec![0.10, 0.18];
        for _ in 0..60 {
            phases = desync_round(&phases, 0.5);
        }
        let gap = (phases[0] - phases[1]).abs();
        let circ = gap.min(1.0 - gap);
        assert!(
            (circ - 0.5).abs() < 0.02,
            "two nodes should reach anti-phase: {circ:.3}"
        );
    }
}
