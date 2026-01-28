pub mod agent;
pub mod mesh;
pub mod metabolism;
pub mod sensor;

// Re-export common types
pub use agent::{Bid, Capability, EnergyStatus, Task};
pub use mesh::{MeshConfig, MeshControl, MeshPeer, MeshStats, TopicMesh};
pub use metabolism::{BatteryMetabolism, Metabolism, MockMetabolism, PowerMode};
pub use sensor::{BasicSensor, VirtualSensor};
