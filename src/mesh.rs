//! Gossip Mesh Management for Hypha
//!
//! Implements gossipsub-style mesh management with energy-aware peer scoring.
//! Key concepts:
//!
//! - **D parameters**: Target mesh degree (D=6), bounds (D_low=4, D_high=12)
//! - **Peer scoring**: Energy scores influence mesh membership
//! - **Opportunistic grafting**: Recover from degraded mesh states
//! - **Flood publishing**: Own messages bypass mesh for eclipse resistance
//!
//! This module provides a simulation-friendly mesh layer that can be evaluated
//! without running a full libp2p swarm.

use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Mesh configuration parameters (following gossipsub v1.1 defaults)
#[derive(Debug, Clone)]
pub struct MeshConfig {
    /// Target outbound degree
    pub d: usize,
    /// Lower bound for mesh peers
    pub d_low: usize,
    /// Upper bound for mesh peers
    pub d_high: usize,
    /// Number of peers to gossip IHAVE to
    pub d_lazy: usize,
    /// Heartbeat interval
    pub heartbeat_interval: Duration,
    /// Threshold below which to opportunistically graft
    pub opportunistic_graft_threshold: f32,
    /// Minimum score for mesh membership
    pub graft_threshold: f32,
    /// Score below which peer is pruned
    pub prune_threshold: f32,
}

impl MeshConfig {
    /// Adaptive configuration based on energy score
    pub fn adaptive(energy_score: f32) -> Self {
        let mut config = Self::default();
        if energy_score < 0.2 {
            // Critical: minimize mesh degree to save energy
            config.d = 2;
            config.d_low = 1;
            config.d_high = 4;
            config.d_lazy = 2;
        } else if energy_score < 0.5 {
            // Low Battery: reduced mesh degree
            config.d = 4;
            config.d_low = 2;
            config.d_high = 8;
            config.d_lazy = 4;
        }
        config
    }
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            d: 6,
            d_low: 4,
            d_high: 12,
            d_lazy: 6,
            heartbeat_interval: Duration::from_secs(1),
            opportunistic_graft_threshold: 0.3,
            graft_threshold: 0.1,
            prune_threshold: 0.05,
        }
    }
}

/// A peer in the mesh
#[derive(Debug, Clone)]
pub struct MeshPeer {
    pub id: String,
    pub energy_score: f32,
    /// Bio-inspired conductivity (thickens/thins based on flow)
    pub conductivity: f32,
    /// Local pressure (e.g. message backlog)
    pub pressure: f32,
    pub message_count: u64,
    pub last_seen: Instant,
    pub in_mesh: bool,
}

impl MeshPeer {
    pub fn new(id: String, energy_score: f32) -> Self {
        Self {
            id,
            energy_score,
            conductivity: 1.0,
            pressure: 0.0,
            message_count: 0,
            last_seen: Instant::now(),
            in_mesh: false,
        }
    }

    /// Compute peer score (weighted combination of factors)
    pub fn score(&self) -> f32 {
        // Energy contributes 30%, activity 20%, conductivity 30%, pressure balance 20%
        let activity_score = (self.message_count as f32 / 100.0).min(1.0);
        let normalized_conductivity = self.conductivity.min(5.0) / 5.0;
        // Low pressure peers are preferred (they have capacity)
        let pressure_score = 1.0 - (self.pressure.min(10.0) / 10.0);

        self.energy_score * 0.3
            + activity_score * 0.2
            + normalized_conductivity * 0.3
            + pressure_score * 0.2
    }
}

/// Control messages for mesh management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MeshControl {
    /// Request to join mesh
    Graft { topic: String },
    /// Notification of mesh removal
    Prune { topic: String, backoff: Duration },
    /// Lazy push: announce message availability
    IHave {
        topic: String,
        message_ids: Vec<String>,
    },
    /// Request full message
    IWant { message_ids: Vec<String> },
}

/// Mesh state for a single topic
#[derive(Debug)]
pub struct TopicMesh {
    pub topic: String,
    pub config: MeshConfig,
    /// Local pressure (message backlog)
    pub local_pressure: f32,
    /// Current phase of mycelial pulse (0.0 to 1.0)
    pub pulse_phase: f32,
    /// Peers currently in the mesh
    pub mesh_peers: HashSet<String>,
    /// All known peers (mesh + fanout)
    pub known_peers: HashMap<String, MeshPeer>,
    /// Messages we have (for IHAVE/IWANT)
    pub message_cache: HashSet<String>,
    /// Messages received multiple times
    pub duplicate_count: u64,
    /// Backoff timers for pruned peers
    pub backoff: HashMap<String, Instant>,
}

impl TopicMesh {
    pub fn new(topic: String, config: MeshConfig) -> Self {
        Self {
            topic,
            config,
            local_pressure: 0.0,
            pulse_phase: rand::random::<f32>(),
            mesh_peers: HashSet::new(),
            known_peers: HashMap::new(),
            message_cache: HashSet::new(),
            duplicate_count: 0,
            backoff: HashMap::new(),
        }
    }

    /// Update local pressure based on backlog
    pub fn set_pressure(&mut self, pressure: f32) {
        self.local_pressure = pressure;
    }

    /// Advance pulse phase
    pub fn tick_pulse(&mut self, delta: f32) {
        self.pulse_phase = (self.pulse_phase + delta) % 1.0;
    }

    /// Align pulse with a neighbor
    pub fn align_pulse(&mut self, neighbor_phase: f32, weight: f32) {
        // Simple moving average toward neighbor phase
        let diff = neighbor_phase - self.pulse_phase;
        // Handle wrap-around
        let diff = if diff > 0.5 {
            diff - 1.0
        } else if diff < -0.5 {
            diff + 1.0
        } else {
            diff
        };
        self.pulse_phase = (self.pulse_phase + diff * weight) % 1.0;
        if self.pulse_phase < 0.0 {
            self.pulse_phase += 1.0;
        }
    }

    /// Update peer pressure
    pub fn update_peer_pressure(&mut self, id: &str, pressure: f32) {
        if let Some(peer) = self.known_peers.get_mut(id) {
            peer.pressure = pressure;
        }
    }

    /// Add a peer to the known set
    pub fn add_peer(&mut self, id: String, energy_score: f32) {
        self.known_peers
            .entry(id.clone())
            .or_insert_with(|| MeshPeer::new(id, energy_score));
    }

    /// Update peer's energy score
    pub fn update_peer_score(&mut self, id: &str, energy_score: f32) {
        if let Some(peer) = self.known_peers.get_mut(id) {
            peer.energy_score = energy_score;
            peer.last_seen = Instant::now();
        }
    }

    /// Record message from peer (increases their score)
    pub fn record_message(&mut self, peer_id: &str, msg_id: &str) {
        if let Some(peer) = self.known_peers.get_mut(peer_id) {
            peer.message_count += 1;
            peer.last_seen = Instant::now();

            // Pressure-Aware Path Thickening:
            // Delta D = |P_self - P_peer| * Flow
            let pressure_grad = (self.local_pressure - peer.pressure).abs().max(0.1);
            peer.conductivity += 0.1 * pressure_grad;
        }

        if self.message_cache.contains(msg_id) {
            self.duplicate_count += 1;
        } else {
            self.message_cache.insert(msg_id.to_string());
        }
    }

    /// Get current mesh size
    pub fn mesh_size(&self) -> usize {
        self.mesh_peers.len()
    }

    /// Get median score of mesh peers
    pub fn mesh_median_score(&self) -> f32 {
        let mut scores: Vec<f32> = self
            .mesh_peers
            .iter()
            .filter_map(|id| self.known_peers.get(id))
            .map(|p| p.score())
            .collect();

        if scores.is_empty() {
            return 0.0;
        }

        scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mid = scores.len() / 2;
        if scores.len().is_multiple_of(2) {
            (scores[mid - 1] + scores[mid]) / 2.0
        } else {
            scores[mid]
        }
    }

    /// Heartbeat: maintain mesh invariants
    /// Returns control messages to send
    pub fn heartbeat(&mut self) -> Vec<(String, MeshControl)> {
        let mut controls = Vec::new();
        let mut rng = thread_rng();

        // Path Thinning: decay conductivity
        for peer in self.known_peers.values_mut() {
            peer.conductivity = (peer.conductivity * 0.95).max(0.5);
        }

        // Clean up expired backoffs
        let now = Instant::now();
        self.backoff.retain(|_, expiry| *expiry > now);

        // Prune low-scoring peers and excess peers
        let to_prune: Vec<String> = self
            .mesh_peers
            .iter()
            .filter(|id| {
                self.known_peers
                    .get(*id)
                    .map(|p| p.score() < self.config.prune_threshold)
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        for id in to_prune {
            self.mesh_peers.remove(&id);
            controls.push((
                id.clone(),
                MeshControl::Prune {
                    topic: self.topic.clone(),
                    backoff: Duration::from_secs(60),
                },
            ));
            self.backoff.insert(id, now + Duration::from_secs(60));
        }

        // Prune excess if above D_high
        while self.mesh_peers.len() > self.config.d_high {
            // Remove lowest scoring peer
            let lowest = self
                .mesh_peers
                .iter()
                .filter_map(|id| self.known_peers.get(id).map(|p| (id.clone(), p.score())))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            if let Some((id, _)) = lowest {
                self.mesh_peers.remove(&id);
                controls.push((
                    id.clone(),
                    MeshControl::Prune {
                        topic: self.topic.clone(),
                        backoff: Duration::from_secs(60),
                    },
                ));
            } else {
                break;
            }
        }

        // Graft if below D_low
        while self.mesh_peers.len() < self.config.d_low {
            // Find best non-mesh peer
            let candidate = self
                .known_peers
                .iter()
                .filter(|(id, peer)| {
                    !self.mesh_peers.contains(*id)
                        && !self.backoff.contains_key(*id)
                        && peer.score() >= self.config.graft_threshold
                })
                .max_by(|a, b| a.1.score().partial_cmp(&b.1.score()).unwrap());

            if let Some((id, _)) = candidate {
                let id = id.clone();
                self.mesh_peers.insert(id.clone());
                if let Some(peer) = self.known_peers.get_mut(&id) {
                    peer.in_mesh = true;
                }
                controls.push((
                    id,
                    MeshControl::Graft {
                        topic: self.topic.clone(),
                    },
                ));
            } else {
                break; // No suitable candidates
            }
        }

        // Opportunistic grafting: if median score is low, graft high-scoring peers
        let median = self.mesh_median_score();
        if median < self.config.opportunistic_graft_threshold
            && self.mesh_peers.len() < self.config.d_high
        {
            let candidates: Vec<_> = self
                .known_peers
                .iter()
                .filter(|(id, peer)| {
                    !self.mesh_peers.contains(*id)
                        && !self.backoff.contains_key(*id)
                        && peer.score() > median
                })
                .take(2)
                .map(|(id, _)| id.clone())
                .collect();

            for id in candidates {
                if self.mesh_peers.len() >= self.config.d_high {
                    break;
                }
                self.mesh_peers.insert(id.clone());
                if let Some(peer) = self.known_peers.get_mut(&id) {
                    peer.in_mesh = true;
                }
                controls.push((
                    id,
                    MeshControl::Graft {
                        topic: self.topic.clone(),
                    },
                ));
            }
        }

        // Bio-inspired Rebalancing: try to replace weak links with better ones
        if self.mesh_peers.len() >= self.config.d_low {
            let weakest = self
                .mesh_peers
                .iter()
                .filter_map(|id| self.known_peers.get(id).map(|p| (id.clone(), p.score())))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            if let Some((weak_id, weak_score)) = weakest {
                let best_candidate = self
                    .known_peers
                    .iter()
                    .filter(|(id, peer)| {
                        !self.mesh_peers.contains(*id)
                            && !self.backoff.contains_key(*id)
                            && peer.score() > weak_score + 0.1 // Significant improvement required
                    })
                    .max_by(|a, b| a.1.score().partial_cmp(&b.1.score()).unwrap());

                if let Some((best_id, _)) = best_candidate {
                    let best_id = best_id.clone();
                    // Prune weakest
                    self.mesh_peers.remove(&weak_id);
                    controls.push((
                        weak_id.clone(),
                        MeshControl::Prune {
                            topic: self.topic.clone(),
                            backoff: Duration::from_secs(30),
                        },
                    ));

                    // Graft best
                    self.mesh_peers.insert(best_id.clone());
                    if let Some(peer) = self.known_peers.get_mut(&best_id) {
                        peer.in_mesh = true;
                    }
                    controls.push((
                        best_id,
                        MeshControl::Graft {
                            topic: self.topic.clone(),
                        },
                    ));
                }
            }
        }

        // Lazy push: send IHAVE to random non-mesh peers
        let non_mesh: Vec<_> = self
            .known_peers
            .keys()
            .filter(|id| !self.mesh_peers.contains(*id))
            .cloned()
            .collect();

        let ihave_targets: Vec<_> = non_mesh
            .choose_multiple(&mut rng, self.config.d_lazy.min(non_mesh.len()))
            .cloned()
            .collect();

        if !self.message_cache.is_empty() && !ihave_targets.is_empty() {
            let recent_msgs: Vec<_> = self.message_cache.iter().take(10).cloned().collect();

            for target in ihave_targets {
                controls.push((
                    target,
                    MeshControl::IHave {
                        topic: self.topic.clone(),
                        message_ids: recent_msgs.clone(),
                    },
                ));
            }
        }

        controls
    }

    /// Handle incoming GRAFT request
    pub fn handle_graft(&mut self, peer_id: &str) -> bool {
        if let Some(peer) = self.known_peers.get(peer_id) {
            if peer.score() >= self.config.graft_threshold
                && self.mesh_peers.len() < self.config.d_high
            {
                self.mesh_peers.insert(peer_id.to_string());
                return true;
            }
        }
        false
    }

    /// Handle incoming PRUNE
    pub fn handle_prune(&mut self, peer_id: &str, backoff: Duration) {
        self.mesh_peers.remove(peer_id);
        if let Some(peer) = self.known_peers.get_mut(peer_id) {
            peer.in_mesh = false;
        }
        self.backoff
            .insert(peer_id.to_string(), Instant::now() + backoff);
    }

    /// Handle incoming spike signal
    pub fn handle_spike(&mut self, source: &str, intensity: u8) {
        if intensity > 200 {
            // Extreme spike: Immediate "Excited" state
            self.set_pressure(10.0); // Max pressure
                                     // Thicken paths to source
            if let Some(peer) = self.known_peers.get_mut(source) {
                peer.conductivity += 2.0;
            }
        }
    }

    /// Handle incoming control message
    pub fn handle_control(&mut self, peer_id: &str, control: MeshControl) -> Option<MeshControl> {
        match control {
            MeshControl::Graft { .. } => {
                if self.handle_graft(peer_id) {
                    None
                } else {
                    Some(MeshControl::Prune {
                        topic: self.topic.clone(),
                        backoff: Duration::from_secs(60),
                    })
                }
            }
            MeshControl::Prune { backoff, .. } => {
                self.handle_prune(peer_id, backoff);
                None
            }
            MeshControl::IHave { message_ids, .. } => {
                let missing: Vec<_> = message_ids
                    .into_iter()
                    .filter(|id| !self.message_cache.contains(id))
                    .collect();

                if !missing.is_empty() {
                    Some(MeshControl::IWant {
                        message_ids: missing,
                    })
                } else {
                    None
                }
            }
            MeshControl::IWant { message_ids: _ } => {
                // In a real system, we'd send the full messages here.
                // For simulation, we just return nothing but the caller
                // should trigger a 're-propagation'.
                None
            }
        }
    }

    /// Get peers to forward a message to (mesh peers + flood if own message)
    pub fn get_forward_targets(&self, is_own_message: bool) -> Vec<String> {
        if is_own_message {
            // Flood publishing: send to all peers above publish threshold
            self.known_peers
                .iter()
                .filter(|(_, peer)| peer.score() >= self.config.graft_threshold)
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            // Normal: forward to mesh peers only
            self.mesh_peers.iter().cloned().collect()
        }
    }

    /// Statistics for evaluation
    pub fn stats(&self) -> MeshStats {
        let scores: Vec<f32> = self
            .mesh_peers
            .iter()
            .filter_map(|id| self.known_peers.get(id))
            .map(|p| p.score())
            .collect();

        MeshStats {
            mesh_size: self.mesh_peers.len(),
            known_peers: self.known_peers.len(),
            median_score: self.mesh_median_score(),
            min_score: scores.iter().cloned().fold(f32::INFINITY, f32::min),
            max_score: scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            messages_cached: self.message_cache.len(),
            duplicate_count: self.duplicate_count,
            backoff_count: self.backoff.len(),
        }
    }
}

/// Statistics about mesh state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshStats {
    pub mesh_size: usize,
    pub known_peers: usize,
    pub median_score: f32,
    pub min_score: f32,
    pub max_score: f32,
    pub messages_cached: usize,
    pub duplicate_count: u64,
    pub backoff_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_graft_below_d_low() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());

        // Add 10 peers
        for i in 0..10 {
            mesh.add_peer(format!("peer-{}", i), 0.5 + (i as f32 * 0.05));
        }

        // Initial mesh is empty
        assert_eq!(mesh.mesh_size(), 0);

        // Heartbeat should graft peers up to D_low
        let _ = mesh.heartbeat();

        // Should have grafted D_low (4) peers
        assert!(mesh.mesh_size() >= mesh.config.d_low);
    }

    #[test]
    fn test_mesh_prune_above_d_high() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());

        // Add 15 peers directly to mesh (exceeds D_high=12)
        for i in 0..15 {
            let id = format!("peer-{}", i);
            mesh.add_peer(id.clone(), 0.5);
            mesh.mesh_peers.insert(id);
        }

        assert_eq!(mesh.mesh_size(), 15);

        // Heartbeat should prune down to D_high
        let _ = mesh.heartbeat();

        assert!(mesh.mesh_size() <= mesh.config.d_high);
    }

    #[test]
    fn test_opportunistic_grafting() {
        let config = MeshConfig {
            d: 6,
            d_low: 4,
            d_high: 12,
            opportunistic_graft_threshold: 0.5,
            ..Default::default()
        };
        let mut mesh = TopicMesh::new("test".to_string(), config);

        // Add low-scoring peers to mesh
        for i in 0..6 {
            let id = format!("low-{}", i);
            mesh.add_peer(id.clone(), 0.2); // Below threshold
            mesh.mesh_peers.insert(id);
        }

        // Add high-scoring peers outside mesh
        for i in 0..4 {
            mesh.add_peer(format!("high-{}", i), 0.8);
        }

        // Median score is low
        assert!(mesh.mesh_median_score() < 0.5);

        // Heartbeat should opportunistically graft high-scoring peers
        let _ = mesh.heartbeat();

        let has_high = mesh.mesh_peers.iter().any(|id| id.starts_with("high"));
        assert!(has_high);
    }

    #[test]
    fn test_phase_alignment() {
        let mut mesh_a = TopicMesh::new("test".to_string(), MeshConfig::default());
        let mut mesh_b = TopicMesh::new("test".to_string(), MeshConfig::default());

        mesh_a.pulse_phase = 0.1;
        mesh_b.pulse_phase = 0.9;

        // Initial diff is 0.2 (0.9 - 1.1 or 0.1 - (-0.1))

        mesh_a.align_pulse(mesh_b.pulse_phase, 0.5);
        // Expected phase: 0.1 + (diff * 0.5)
        // Diff 0.9 -> 0.1 is -0.2 (0.1 - 0.9 + 1.0 or whatever)
        // Let's just check they got closer
        let diff_before = 0.2;
        let diff_after = {
            let d = (mesh_a.pulse_phase - mesh_b.pulse_phase).abs();
            if d > 0.5 {
                1.0 - d
            } else {
                d
            }
        };
        assert!(diff_after < diff_before);
    }

    #[test]
    fn test_spike_handling() {
        let mut mesh = TopicMesh::new("test".to_string(), MeshConfig::default());
        mesh.add_peer("danger-node".to_string(), 0.5);

        let initial_cond = mesh.known_peers.get("danger-node").unwrap().conductivity;
        let _initial_pressure = mesh.local_pressure;

        // Handle danger spike
        mesh.handle_spike("danger-node", 255);

        assert_eq!(
            mesh.local_pressure, 10.0,
            "Pressure should max out on danger spike"
        );
        let final_cond = mesh.known_peers.get("danger-node").unwrap().conductivity;
        assert!(
            final_cond >= initial_cond + 2.0,
            "Path should thicken significantly"
        );
    }
}
