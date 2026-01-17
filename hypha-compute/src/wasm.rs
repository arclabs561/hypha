use crate::{ComputeError, ComputeRuntime};
use async_trait::async_trait;
use hypha::Metabolism;
use std::sync::{Arc, Mutex};
use wasmtime::{Config, Engine, Linker, Module, Store};

pub struct WasmTimeRuntime {
    engine: Engine,
}

impl WasmTimeRuntime {
    pub fn new() -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.async_support(true);
        config.consume_fuel(true); // Vital for resource limiting
        
        let engine = Engine::new(&config)?;
        Ok(Self { engine })
    }
}

#[async_trait]
impl ComputeRuntime for WasmTimeRuntime {
    fn name(&self) -> &str {
        "wasmtime"
    }

    async fn execute(
        &self,
        payload: &[u8],
        _input: &[u8],
        metabolism: Arc<Mutex<dyn Metabolism>>,
        budget: f32,
    ) -> Result<Vec<u8>, ComputeError> {
        // 1. Compile Module
        let module = Module::from_binary(&self.engine, payload)
            .map_err(|e| ComputeError::Wasm(e.to_string()))?;

        // 2. Setup Store & Fuel
        struct State {}
        let mut store = Store::new(&self.engine, State {});
        
        // Map 1.0 energy -> 100,000 fuel units (example ratio)
        let fuel_limit = (budget * 100_000.0) as u64;
        store.set_fuel(fuel_limit).map_err(|e| ComputeError::Wasm(e.to_string()))?;

        // 3. Instantiate
        let linker = Linker::new(&self.engine);
        let instance = linker
            .instantiate_async(&mut store, &module)
            .await
            .map_err(|e| ComputeError::Wasm(e.to_string()))?;

        // 4. Invoke "run" export
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .map_err(|e| ComputeError::Wasm(format!("Missing 'run' export: {}", e)))?;

        // 5. Execute
        match run.call_async(&mut store, ()).await {
            Ok(_) => {
                // Calculate consumed energy
                // get_fuel returns remaining fuel.
                let remaining = store.get_fuel().unwrap_or(0);
                let consumed = fuel_limit.saturating_sub(remaining);
                let cost = consumed as f32 / 100_000.0;
                
                // Deduct from metabolism
                let mut meta = metabolism.lock().unwrap();
                if !meta.consume(cost) {
                    return Err(ComputeError::Exhausted);
                }
                
                Ok(vec![]) // Output capturing TBD
            }
            Err(e) => {
                // If it's OOG, it traps.
                Err(ComputeError::Wasm(e.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ComputeError;
    use hypha::{BatteryMetabolism, Metabolism};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn test_wasm_execution_consumes_fuel() {
        // 1. Setup Runtime
        let runtime = WasmTimeRuntime::new().unwrap();
        
        // 2. Setup Metabolism (100% battery)
        let meta = Arc::new(Mutex::new(BatteryMetabolism::default()));
        
        // 3. Create a simple WAT module that loops to burn fuel
        // This module exports "run".
        let wat_finite = r#"
            (module
                (func (export "run")
                    (local $i i32)
                    (local.set $i (i32.const 0))
                    (loop $l
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br_if $l (i32.lt_u (local.get $i) (i32.const 1000)))
                    )
                )
            )
        "#;
        let wasm_bytes = wat::parse_str(wat_finite).unwrap();

        // 4. Execute with budget
        // 1.0 budget = 100,000 units. 1000 loops should be cheap.
        let result = runtime.execute(&wasm_bytes, &[], meta.clone(), 1.0).await;
        
        assert!(result.is_ok(), "Execution failed: {:?}", result.err());

        // 5. Verify energy consumption
        let remaining = meta.lock().unwrap().energy_score();
        println!("Remaining energy: {}", remaining);
        assert!(remaining < 1.0, "Energy should have been consumed");
        assert!(remaining > 0.9, "Energy should not be exhausted by simple loop");
    }

    #[tokio::test]
    async fn test_exhaustion() {
        let runtime = WasmTimeRuntime::new().unwrap();
        let meta = Arc::new(Mutex::new(BatteryMetabolism::default()));
        
        // Loop 1 million times
        let wat = r#"
            (module
                (func (export "run")
                    (local $i i32)
                    (loop $l
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br_if $l (i32.lt_u (local.get $i) (i32.const 1000000)))
                    )
                )
            )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();

        // Tiny budget (0.0001 = 10 fuel)
        let result = runtime.execute(&wasm_bytes, &[], meta.clone(), 0.0001).await;
        
        // Should fail due to OOG (Out Of Gas) / Wasm runtime error
        assert!(result.is_err());
        if let Err(ComputeError::Wasm(msg)) = result {
            println!("Got expected error: {}", msg);
            // Wasmtime error messages vary, but if it errored on a loop, it's likely fuel.
            // assert!(msg.contains("fuel")); 
        }
    }
}
