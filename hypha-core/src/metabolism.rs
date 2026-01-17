use serde::{Deserialize, Serialize};

/// Trait for resource management/metabolism
pub trait Metabolism: Send + Sync + std::fmt::Debug {
    fn energy_score(&self) -> f32;
    fn consume(&mut self, cost: f32) -> bool;
    fn remaining(&self) -> f32;
    fn set_mode(&mut self, mode: PowerMode);
    fn is_mains_powered(&self) -> bool;
    fn as_any(&mut self) -> &mut dyn std::any::Any;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PowerMode {
    Normal,
    LowBattery,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryMetabolism {
    pub voltage: f32,
    pub mah_remaining: f32,
    pub temp_celsius: f32,
    pub is_mains: bool,
}

impl Default for BatteryMetabolism {
    fn default() -> Self {
        Self {
            voltage: 4.2,
            mah_remaining: 2500.0,
            temp_celsius: 25.0,
            is_mains: false,
        }
    }
}

impl Metabolism for BatteryMetabolism {
    fn energy_score(&self) -> f32 {
        if self.is_mains {
            return 1.0;
        }
        let v_score = (self.voltage - 3.3) / (4.2 - 3.3);
        let c_score = self.mah_remaining / 2500.0;
        (v_score * 0.4 + c_score * 0.6).clamp(0.0, 1.0)
    }

    fn consume(&mut self, cost: f32) -> bool {
        if self.mah_remaining <= 0.0 {
            return false;
        }
        self.mah_remaining = (self.mah_remaining - cost).max(0.0);
        let capacity_ratio = self.mah_remaining / 2500.0;
        self.voltage = 3.3 + (capacity_ratio * 0.9);
        true
    }

    fn remaining(&self) -> f32 {
        self.mah_remaining
    }

    fn set_mode(&mut self, mode: PowerMode) {
        match mode {
            PowerMode::Normal => {
                self.voltage = 4.0;
                self.mah_remaining = 2000.0;
            }
            PowerMode::LowBattery => {
                self.voltage = 3.6;
                self.mah_remaining = 500.0;
            }
            PowerMode::Critical => {
                self.voltage = 3.3;
                self.mah_remaining = 50.0;
            }
        }
    }

    fn is_mains_powered(&self) -> bool {
        self.is_mains
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct MockMetabolism {
    pub energy: f32,
    pub is_mains: bool,
}

impl MockMetabolism {
    pub fn new(energy: f32, is_mains: bool) -> Self {
        Self { energy, is_mains }
    }
}

impl Metabolism for MockMetabolism {
    fn energy_score(&self) -> f32 {
        self.energy
    }
    fn consume(&mut self, cost: f32) -> bool {
        if self.energy <= 0.0 {
            return false;
        }
        self.energy = (self.energy - cost).max(0.0);
        true
    }
    fn remaining(&self) -> f32 {
        self.energy * 2500.0 // Mock mapping 1.0 -> 2500 mAh
    }
    fn set_mode(&mut self, _mode: PowerMode) {}
    fn is_mains_powered(&self) -> bool {
        self.is_mains
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
