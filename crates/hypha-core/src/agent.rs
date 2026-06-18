use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Capability {
    Compute(u32),
    Storage(u64),
    Sensing(String),
}

impl Capability {
    pub fn satisfies(&self, required: &Self) -> bool {
        match (self, required) {
            (Self::Compute(available), Self::Compute(required)) => available >= required,
            (Self::Storage(available), Self::Storage(required)) => available >= required,
            (Self::Sensing(available), Self::Sensing(required)) => available == required,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnergyStatus {
    pub source_id: String,
    pub energy_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub required_capability: Capability,
    pub priority: u8,
    pub reach_intensity: f32,
    pub source_id: String,
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
    pub fn diffuse(&self, conductivity: f32, neighbor_energy: f32, neighbor_pressure: f32) -> f32 {
        let pressure_factor = 1.0 - (neighbor_pressure.min(10.0) / 10.0);
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

#[cfg(test)]
mod tests {
    use super::Capability;

    #[test]
    fn compute_capacity_satisfies_smaller_requirement() {
        assert!(Capability::Compute(101).satisfies(&Capability::Compute(50)));
        assert!(Capability::Compute(50).satisfies(&Capability::Compute(50)));
    }

    #[test]
    fn compute_capacity_rejects_larger_requirement() {
        assert!(!Capability::Compute(49).satisfies(&Capability::Compute(50)));
    }

    #[test]
    fn storage_capacity_satisfies_smaller_requirement() {
        assert!(Capability::Storage(2048).satisfies(&Capability::Storage(1024)));
        assert!(Capability::Storage(1024).satisfies(&Capability::Storage(1024)));
    }

    #[test]
    fn storage_capacity_rejects_larger_requirement() {
        assert!(!Capability::Storage(1023).satisfies(&Capability::Storage(1024)));
    }

    #[test]
    fn sensing_stays_exact_until_vocabulary_is_defined() {
        assert!(Capability::Sensing("thermal".to_string())
            .satisfies(&Capability::Sensing("thermal".to_string())));
        assert!(!Capability::Sensing("thermal".to_string())
            .satisfies(&Capability::Sensing("temperature".to_string())));
    }

    #[test]
    fn different_capability_kinds_do_not_satisfy_each_other() {
        assert!(!Capability::Compute(100).satisfies(&Capability::Storage(100)));
        assert!(!Capability::Storage(100).satisfies(&Capability::Compute(100)));
        assert!(!Capability::Sensing("thermal".to_string()).satisfies(&Capability::Compute(1)));
    }
}
