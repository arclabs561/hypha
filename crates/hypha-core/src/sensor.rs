pub trait VirtualSensor: Send + Sync {
    fn name(&self) -> &str;
    fn read(&self) -> f32;
    fn update_from_mesh(&mut self, value: f32);
}

pub struct BasicSensor {
    pub name: String,
    pub last_value: f32,
}

impl VirtualSensor for BasicSensor {
    fn name(&self) -> &str {
        &self.name
    }
    fn read(&self) -> f32 {
        self.last_value
    }
    fn update_from_mesh(&mut self, value: f32) {
        self.last_value = value;
    }
}
