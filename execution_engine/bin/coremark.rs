use std::{
    fs,
    sync::{Arc, RwLock},
    time::Instant,
};

use bytes::Bytes;
use casper_execution_engine::shared::{
    wasm_config::WasmConfig,
    wasm_engine::{
        host_interface::WasmHostInterface, CraneliftOptLevel, ExecutionMode, FunctionContext,
        WasmEngine, WasmerBackend,
    },
};
use thiserror::Error;
use wasmi::core::F32;

#[derive(Default)]
struct GasCounter {
    gas_calls: u64,
    gas_consumed: u64,
}

#[derive(Clone)]
struct BenchHost {
    // gas_calls: Arc<RwLock<u64>>,
    // gas: Arc<RwLock<GasCounter>>,
}

impl BenchHost {
    fn new() -> Self {
        Self {
            // gas: Arc::new(RwLock::new(Default::default())),
        }
    }
}

#[derive(Error, Debug)]
#[error("bench error")]
struct BenchError {}

fn clock_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Clock may have gone backwards")
        .as_millis() as u64
}

impl WasmHostInterface for BenchHost {
    type Error = BenchError;

    fn gas(&mut self, _ctx: impl FunctionContext, param: u32) -> Result<(), Self::Error> {
        // let mut gas = self.gas.write().unwrap();
        // (*gas).gas_calls += 1;
        // (*gas).gas_consumed += param as u64;
        // *self.gas.write().unwrap().gas_consumed += param as u64;
        Ok(())
    }

    #[cfg(feature = "benches")]
    fn clock_ms(&mut self, _ctx: impl FunctionContext) -> Result<u64, Self::Error> {
        Ok(clock_ms())
    }
}
fn main() {
    #[cfg(not(feature = "benches"))]
    {
        eprintln!("Warning: you may need to enable --features benches to run some benchmark wasm");
    }

    let execution_modes = [
        ("interpreter", ExecutionMode::Interpreted),
        (
            "singlepass",
            ExecutionMode::Wasmer {
                backend: WasmerBackend::Singlepass,
                /// Wasmer has caching turned off, as we don't need to do this extra work since
                /// wasmi (aka interpreted) doesn't do that either.
                cache_artifacts: false,
                instrument: false,
            },
        ),
        (
            "singlepass instr",
            ExecutionMode::Wasmer {
                backend: WasmerBackend::Singlepass,
                /// Wasmer has caching turned off, as we don't need to do this extra work since
                /// wasmi (aka interpreted) doesn't do that either.
                cache_artifacts: false,
                instrument: true,
            },
        ),
        (
            "cranelift",
            ExecutionMode::Wasmer {
                backend: WasmerBackend::Cranelift {
                    optimize: CraneliftOptLevel::Speed, // wasmer's default
                },
                /// Wasmer has caching turned off, as we don't need to do this extra work since
                /// wasmi (aka interpreted) doesn't do that either.
                cache_artifacts: false,
                instrument: true,
            },
        ),
        (
            "wasmtime",
            ExecutionMode::Compiled {
                cache_artifacts: false,
            },
        ),
    ];
    let path = std::env::args().nth(1).expect("no pattern given");

    let wasm_bytes = fs::read(&path)
        .map(Bytes::from)
        .expect("should read wasm file");
    // println!("Execution mode: {:?}", execution_mode);
    println!("Using wasm file: {}", path);

    for (backend_name, execution_mode) in execution_modes {
        // let func_name = std::env::args().nth(2).expect("func name");

        let wasm_config = WasmConfig {
            execution_mode,
            ..Default::default()
        };

        let wasm_engine = WasmEngine::new(wasm_config);

        let host = BenchHost::new();

        let start = Instant::now();
        let wasm_module = wasm_engine.preprocess(None, &wasm_bytes).unwrap();
        let preprocess_step = start.elapsed();
        let wasm_instance = wasm_engine
            .instance_and_memory(wasm_module, host.clone())
            .unwrap();
        let instantiation_step = start.elapsed();
        let invoke_result = wasm_instance
            .invoke_export::<f32, F32>(None, &wasm_engine, "run", ())
            .unwrap();
        let invoke_step = start.elapsed();

        let a = preprocess_step;
        let b = instantiation_step - preprocess_step;
        let c = invoke_step - instantiation_step;
        assert_eq!(a + b + c, invoke_step);

        println!("{} coremark score: {:?}", backend_name, invoke_result);
        println!("{} preprocess: {:?}", backend_name, a);
        println!("{} instantiation: {:?}", backend_name, b);
        println!("{} invoke: {:?}", backend_name, c);
        println!("{} total: {:?}", backend_name, invoke_step);

        // let gas = host.gas.read().unwrap();
        // println!("{} gas calls: {}", backend_name, gas.gas_calls);
        // println!("{} gas consumed: {}", backend_name, gas.gas_consumed);

        // dbg!(&invoke_result);
    }
}
