use serde::{Deserialize, Serialize};

/// High-level capability of a spore (The agentic layer)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Capability {
    Compute(u32),
    Storage(u64),
    Sensing(String),
}

/// Energy status advertisement for gradient-based routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyStatus {
    pub source_id: String,
    pub energy_score: f32, // 0.0 (dead) to 1.0 (mains/full)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub required_capability: Capability,
    pub priority: u8,
    /// Reach intensity (diffuses through mesh)
    pub reach_intensity: f32,
    pub source_id: String,
    /// UCAN Authorization token (not a JWT).
    pub auth_token: Option<String>,
}

impl Task {
    pub fn new(id: String, cap: Capability, priority: u8, source_id: String) -> Self {
        Self {
            id,
            required_capability: cap,
            priority,
            reach_intensity: 1.0,
            source_id,
            auth_token: None,
        }
    }

    pub fn with_auth(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Diffuse reach to a neighbor
    pub fn diffuse(&self, conductivity: f32, neighbor_energy: f32, neighbor_pressure: f32) -> f32 {
        let pressure_factor = 1.0 - (neighbor_pressure.min(10.0) / 10.0);
        // More liberal diffusion to ensure reach
        self.reach_intensity
            * conductivity.min(3.0)
            * (neighbor_energy + 0.2).min(1.0)
            * (pressure_factor + 0.1).min(1.0)
            * 0.9
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bid {
    pub task_id: String,
    pub bidder_id: String,
    pub energy_score: f32,
    pub cost_mah: f32,
}
