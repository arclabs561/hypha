use rand::rng;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Mesh configuration parameters (following gossipsub v1.1 defaults)
#[derive(Debug, Clone)]
pub struct MeshConfig {
    pub d: usize,
    pub d_low: usize,
    pub d_high: usize,
    pub d_lazy: usize,
    pub heartbeat_interval: Duration,
    pub opportunistic_graft_threshold: f32,
    pub graft_threshold: f32,
    pub prune_threshold: f32,
}

impl MeshConfig {
    pub fn adaptive(energy_score: f32) -> Self {
        let mut config = Self::default();
        if energy_score < 0.2 {
            config.d = 2;
            config.d_low = 1;
            config.d_high = 4;
            config.d_lazy = 2;
        } else if energy_score < 0.5 {
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

#[derive(Debug, Clone)]
pub struct MeshPeer {
    pub id: String,
    pub energy_score: f32,
    pub conductivity: f32,
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

    pub fn score(&self) -> f32 {
        let activity_score = (self.message_count as f32 / 100.0).min(1.0);
        let normalized_conductivity = self.conductivity.min(5.0) / 5.0;
        let pressure_score = 1.0 - (self.pressure.min(10.0) / 10.0);

        self.energy_score * 0.3
            + activity_score * 0.2
            + normalized_conductivity * 0.3
            + pressure_score * 0.2
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MeshControl {
    Graft {
        topic: String,
    },
    Prune {
        topic: String,
        backoff: Duration,
    },
    IHave {
        topic: String,
        message_ids: Vec<String>,
    },
    IWant {
        message_ids: Vec<String>,
    },
}

#[derive(Debug)]
pub struct TopicMesh {
    pub topic: String,
    pub config: MeshConfig,
    pub local_pressure: f32,
    pub pulse_phase: f32,
    pub mesh_peers: HashSet<String>,
    pub known_peers: HashMap<String, MeshPeer>,
    pub message_cache: HashSet<String>,
    pub duplicate_count: u64,
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

    pub fn set_pressure(&mut self, pressure: f32) {
        self.local_pressure = pressure;
    }

    pub fn tick_pulse(&mut self, delta: f32) {
        self.pulse_phase = (self.pulse_phase + delta) % 1.0;
    }

    pub fn align_pulse(&mut self, neighbor_phase: f32, weight: f32) {
        let diff = neighbor_phase - self.pulse_phase;
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

    pub fn update_peer_pressure(&mut self, id: &str, pressure: f32) {
        if let Some(peer) = self.known_peers.get_mut(id) {
            peer.pressure = pressure;
        }
    }

    pub fn add_peer(&mut self, id: String, energy_score: f32) {
        self.known_peers
            .entry(id.clone())
            .or_insert_with(|| MeshPeer::new(id, energy_score));
    }

    pub fn update_peer_score(&mut self, id: &str, energy_score: f32) {
        let peer = self
            .known_peers
            .entry(id.to_string())
            .or_insert_with(|| MeshPeer::new(id.to_string(), energy_score));
        peer.energy_score = energy_score;
        peer.last_seen = Instant::now();
    }

    pub fn record_message(&mut self, peer_id: &str, msg_id: &str) {
        if let Some(peer) = self.known_peers.get_mut(peer_id) {
            peer.message_count += 1;
            peer.last_seen = Instant::now();
            let pressure_grad = (self.local_pressure - peer.pressure).abs().max(0.1);
            peer.conductivity = (peer.conductivity + 0.1 * pressure_grad).min(10.0);
        }

        if self.message_cache.contains(msg_id) {
            self.duplicate_count += 1;
        } else {
            self.message_cache.insert(msg_id.to_string());
        }
    }

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

        scores.sort_by(|a, b| a.total_cmp(b));
        let mid = scores.len() / 2;
        if scores.len() % 2 == 0 {
            (scores[mid - 1] + scores[mid]) / 2.0
        } else {
            scores[mid]
        }
    }

    pub fn heartbeat(&mut self) -> Vec<(String, MeshControl)> {
        let mut controls = Vec::new();
        let mut rng = rng();

        for peer in self.known_peers.values_mut() {
            peer.conductivity = (peer.conductivity * 0.95).max(0.5);
        }

        let now = Instant::now();
        self.backoff.retain(|_, expiry| *expiry > now);

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
            if let Some(peer) = self.known_peers.get_mut(&id) {
                peer.in_mesh = false;
            }
            controls.push((
                id.clone(),
                MeshControl::Prune {
                    topic: self.topic.clone(),
                    backoff: Duration::from_secs(60),
                },
            ));
            self.backoff.insert(id, now + Duration::from_secs(60));
        }

        while self.mesh_peers.len() > self.config.d_high {
            let lowest = self
                .mesh_peers
                .iter()
                .filter_map(|id| self.known_peers.get(id).map(|p| (id.clone(), p.score())))
                .min_by(|a, b| a.1.total_cmp(&b.1));

            if let Some((id, _)) = lowest {
                self.mesh_peers.remove(&id);
                if let Some(peer) = self.known_peers.get_mut(&id) {
                    peer.in_mesh = false;
                }
                controls.push((
                    id.clone(),
                    MeshControl::Prune {
                        topic: self.topic.clone(),
                        backoff: Duration::from_secs(60),
                    },
                ));
                self.backoff.insert(id, now + Duration::from_secs(60));
            } else {
                break;
            }
        }

        while self.mesh_peers.len() < self.config.d_low {
            let candidate = self
                .known_peers
                .iter()
                .filter(|(id, peer)| {
                    !self.mesh_peers.contains(*id)
                        && !self.backoff.contains_key(*id)
                        && peer.score() >= self.config.graft_threshold
                })
                .max_by(|a, b| a.1.score().total_cmp(&b.1.score()));

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
                break;
            }
        }

        let median = self.mesh_median_score();
        if median < self.config.opportunistic_graft_threshold
            && self.mesh_peers.len() < self.config.d_high
        {
            let mut candidates: Vec<_> = self
                .known_peers
                .iter()
                .filter(|(id, peer)| {
                    !self.mesh_peers.contains(*id)
                        && !self.backoff.contains_key(*id)
                        && peer.score() > median
                })
                .map(|(id, peer)| (id.clone(), peer.score()))
                .collect();
            candidates.sort_by(|a, b| b.1.total_cmp(&a.1));

            for (id, _) in candidates.into_iter().take(2) {
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

        if self.mesh_peers.len() >= self.config.d_low {
            let weakest = self
                .mesh_peers
                .iter()
                .filter_map(|id| self.known_peers.get(id).map(|p| (id.clone(), p.score())))
                .min_by(|a, b| a.1.total_cmp(&b.1));

            if let Some((weak_id, weak_score)) = weakest {
                let best_candidate = self
                    .known_peers
                    .iter()
                    .filter(|(id, peer)| {
                        !self.mesh_peers.contains(*id)
                            && !self.backoff.contains_key(*id)
                            && peer.score() > weak_score + 0.1
                    })
                    .max_by(|a, b| a.1.score().total_cmp(&b.1.score()));

                if let Some((best_id, _)) = best_candidate {
                    let best_id = best_id.clone();
                    self.mesh_peers.remove(&weak_id);
                    if let Some(peer) = self.known_peers.get_mut(&weak_id) {
                        peer.in_mesh = false;
                    }
                    controls.push((
                        weak_id.clone(),
                        MeshControl::Prune {
                            topic: self.topic.clone(),
                            backoff: Duration::from_secs(30),
                        },
                    ));
                    self.backoff
                        .insert(weak_id.clone(), now + Duration::from_secs(30));

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

    pub fn handle_graft(&mut self, peer_id: &str) -> bool {
        if self.backoff.contains_key(peer_id) {
            return false;
        }
        if let Some(peer) = self.known_peers.get(peer_id) {
            if peer.score() >= self.config.graft_threshold
                && self.mesh_peers.len() < self.config.d_high
            {
                self.mesh_peers.insert(peer_id.to_string());
                if let Some(peer) = self.known_peers.get_mut(peer_id) {
                    peer.in_mesh = true;
                }
                return true;
            }
        }
        false
    }

    pub fn handle_prune(&mut self, peer_id: &str, backoff: Duration) {
        self.mesh_peers.remove(peer_id);
        if let Some(peer) = self.known_peers.get_mut(peer_id) {
            peer.in_mesh = false;
        }
        self.backoff
            .insert(peer_id.to_string(), Instant::now() + backoff);
    }

    pub fn handle_spike(&mut self, source: &str, intensity: u8) {
        if intensity > 200 {
            self.set_pressure(10.0);
            if let Some(peer) = self.known_peers.get_mut(source) {
                peer.conductivity += 2.0;
            }
        }
    }

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
            MeshControl::IWant { .. } => None,
        }
    }

    pub fn get_forward_targets(&self, is_own_message: bool) -> Vec<String> {
        if is_own_message {
            self.known_peers
                .iter()
                .filter(|(_, peer)| peer.score() >= self.config.graft_threshold)
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            self.mesh_peers.iter().cloned().collect()
        }
    }

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
