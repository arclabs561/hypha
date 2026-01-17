use async_trait::async_trait;
use hypha::Metabolism;
use std::sync::{Arc, Mutex};

/// Error type for compute failures
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    #[error("WASM runtime error: {0}")]
    Wasm(String),
    #[error("Resource exhausted")]
    Exhausted,
    #[error("Task validation failed: {0}")]
    Validation(String),
}

/// Abstract Interface for a Compute Runtime
#[async_trait]
pub trait ComputeRuntime: Send + Sync {
    /// Name of the runtime (e.g., "wasmtime-v1")
    fn name(&self) -> &str;

    /// Execute a task payload
    /// 
    /// * `payload`: The binary code (WASM) to execute
    /// * `input`: Input data for the task
    /// * `metabolism`: Access to resource accounting
    /// * `budget`: Max resource cost allowed
    async fn execute(
        &self,
        payload: &[u8],
        input: &[u8],
        metabolism: Arc<Mutex<dyn Metabolism>>,
        budget: f32,
    ) -> Result<Vec<u8>, ComputeError>;
}

pub mod wasm;
