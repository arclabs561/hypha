//! Embeddable core for Hypha: types, metabolism, capabilities, sensors.

pub mod agent;
pub mod metabolism;
pub mod sensor;

pub use agent::{Bid, Capability, EnergyStatus, Task};
pub use metabolism::{BatteryMetabolism, Metabolism, MockMetabolism, PowerMode};
pub use sensor::{BasicSensor, VirtualSensor};
