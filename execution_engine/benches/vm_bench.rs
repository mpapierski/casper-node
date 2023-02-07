use casper_execution_engine::shared::{
    newtypes::CorrelationId,
    wasm_config::WasmConfig,
    wasm_engine::{
        host_interface::WasmHostInterface, ExecutionMode, FunctionContext, WasmEngine,
        WasmerBackend,
    },
};
use casper_types::{contracts::DEFAULT_ENTRY_POINT_NAME, ProtocolVersion};
use criterion::BenchmarkId;
use once_cell::sync::Lazy;

// Creates minimal session code that does nothing
fn make_do_nothing_bytes() -> Vec<u8> {
    use parity_wasm::builder;

    let module = builder::module()
        .function()
        // A signature with 0 params and no return type
        .signature()
        .build()
        .body()
        .build()
        .build()
        // Export above function
        .export()
        .field(DEFAULT_ENTRY_POINT_NAME)
        .build()
        // Memory section is mandatory
        .memory()
        .build()
        .build();
    parity_wasm::serialize(module).expect("should serialize")
}

use casper_execution_engine::for_each_host_function;
use thiserror::Error;

macro_rules! define_trait_methods {
    (@optional_ret) => { () };
    (@optional_ret $ret:tt) => { $ret };

    ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        $(
            $(#[$cfg])?
            fn $name(&mut self, _context: impl FunctionContext $($(,$arg: $argty)*)?) -> Result<define_trait_methods!(@optional_ret $($ret)?), Self::Error> {
                // Every host function returns an error. This is fine as this benchmark will not hit any of the host functions to measure performance.
                Err(MockHostError(stringify!($name)))
            }
        )*
    };
}

struct MockHost(());

impl MockHost {
    fn new() -> Self {
        MockHost(())
    }
}

#[derive(Error, Debug)]
#[error("mock host interface stopped while executing {0}")]
struct MockHostError(&'static str);

impl WasmHostInterface for MockHost {
    type Error = MockHostError;

    for_each_host_function!(define_trait_methods);
}

/// Executes do nothing session code and measures the time to execute it in isolation.
///
/// Since the "do nothing" costs almost zero to execute on any VM backend we support we can show
/// differencies in the startup times of different backends which includes preprocessing, gas
/// instrumentation, compilation etc.
fn cold_start(c: &mut criterion::Criterion) {
    let execution_modes = [
        ("interpreter", ExecutionMode::Interpreted),
        (
            "singlepass",
            ExecutionMode::Wasmer {
                backend: WasmerBackend::Singlepass,
                /// Wasmer has caching turned off, as we don't need to do this extra work since
                /// wasmi (aka interpreted) doesn't do that either.
                cache_artifacts: false,
            },
        ),
        (
            "wasmtime",
            ExecutionMode::Compiled {
                cache_artifacts: false,
            },
        ),
    ];

    let mut group = c.benchmark_group("cold_start");

    for (backend_name, execution_mode) in execution_modes {
        let wasm_config = WasmConfig {
            execution_mode,
            ..Default::default()
        };
        let engine = WasmEngine::new(wasm_config);

        let baton = (make_do_nothing_bytes(), engine);

        group.bench_with_input(
            BenchmarkId::new(backend_name, "do_nothing"),
            &baton,
            |b, (do_nothing_bytes, engine)| {
                b.iter(|| {
                    let wasm_module = engine.preprocess(None, &do_nothing_bytes).unwrap();
                    let instance = engine
                        .instance_and_memory(wasm_module, MockHost::new())
                        .unwrap();
                    let _result = instance
                        .invoke_export(None, engine, DEFAULT_ENTRY_POINT_NAME, Vec::new())
                        .unwrap();
                });
            },
        );
    }
}

criterion::criterion_group!(benches, cold_start);
criterion::criterion_main!(benches);
