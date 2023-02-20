use std::mem;

use bytes::Bytes;
use casper_execution_engine::shared::{
    newtypes::CorrelationId,
    wasm_config::WasmConfig,
    wasm_engine::{
        host_interface::WasmHostInterface, ExecutionMode, FunctionContext, InstrumentMode,
        WasmEngine, Wasmer, WasmerBackend,
    },
};
use casper_types::{contracts::DEFAULT_ENTRY_POINT_NAME, ProtocolVersion};
use criterion::BenchmarkId;
use once_cell::sync::Lazy;

// Creates minimal session code that does nothing
fn make_do_nothing_bytes() -> Bytes {
    use parity_wasm::builder;

    let module = builder::module()
        .function()
        // A signature with 0 params and no return type
        .signature()
        .build()
        .body()
        .with_instructions(Instructions::new(vec![Instruction::Nop, Instruction::End]))
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
    let raw_vec = parity_wasm::serialize(module).expect("should serialize");
    Bytes::from(raw_vec)
}

fn fixed_cost_br_table(total_labels: usize, br_table_element_size: u32) -> Bytes {
    assert!((br_table_element_size as usize) <= total_labels);

    let mut module = Module::with_config(ModuleConfig::new());

    let _memory_id = module.memories.add_local(false, 11, None);

    let mut br_table_func = FunctionBuilder::new(&mut module.types, &[ValType::I32], &[]);

    let param_jump_label = module.locals.add(ValType::I32);

    fn recursive_block_generator(
        current_block: &mut InstrSeqBuilder,
        mut recursive_step_fn: impl FnMut(&mut InstrSeqBuilder) -> bool,
    ) {
        if !recursive_step_fn(current_block) {
            current_block.block(None, |nested_block| {
                recursive_block_generator(nested_block, recursive_step_fn);
            });
        }
    }

    br_table_func.func_body().block(None, |outer_block| {
        // Outer block becames the "default" jump label for `br_table`.
        let outer_block_id = outer_block.id();

        // Count of recursive iterations left
        let mut counter = total_labels;

        // Labels are extended with newly generated labels at each recursive step
        let mut labels = Vec::new();

        // Generates nested blocks
        recursive_block_generator(outer_block, |step| {
            // Save current nested block in labels.
            labels.push(step.id());

            if counter == 0 {
                // At the tail of this recursive generator we'll create a `br_table` with variable
                // amount of labels depending on this function parameter.
                let labels = mem::take(&mut labels);
                let sliced_labels = labels.as_slice()[..br_table_element_size as usize].to_vec();

                // Code at the tail block
                step.local_get(param_jump_label)
                    .br_table(sliced_labels.into(), outer_block_id);

                // True means this is a tail call, and we won't go deeper
                true
            } else {
                counter -= 1;

                // step.i32_const(counter as i32).drop();

                // Go deeper
                false
            }
        })
    });

    let br_table_func = br_table_func.finish(vec![param_jump_label], &mut module.funcs);

    let mut call_func = FunctionBuilder::new(&mut module.types, &[], &[]);
    call_func
        .func_body()
        // Call `br_table_func` with 0 as the jump label,
        // Specific value does not change the cost, so as long as it will generate valid wasm it's
        // ok.
        .i32_const(total_labels as i32 - 1)
        .call(br_table_func);

    let call_func = call_func.finish(Vec::new(), &mut module.funcs);

    module.exports.add(DEFAULT_ENTRY_POINT_NAME, call_func);

    Bytes::from(module.emit_wasm())
}

use casper_execution_engine::for_each_host_function;
use parity_wasm::elements::{Instruction, Instructions};
use thiserror::Error;
use walrus::{ir::BinaryOp, FunctionBuilder, InstrSeqBuilder, Module, ModuleConfig, ValType};

macro_rules! define_trait_methods {
    (@optional_ret) => { () };
    (@optional_ret $ret:tt) => { $ret };

    ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        $(
            $(#[$cfg])?
            fn $name(&mut self, _context: impl FunctionContext $($(,$arg: $argty)*)?) -> Result<define_trait_methods!(@optional_ret $($ret)?), Self::Error> {
                // Every host function returns ok. For instance: a gas function will be fine in a mock host interface without any work, but others might produce incorrect results.
                Ok(Default::default())
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
                instrument: InstrumentMode::MeteringMiddleware,
            },
        ),
        (
            "wasmtime",
            ExecutionMode::Compiled {
                cache_artifacts: false,
            },
        ),
    ];

    let mut group = c.benchmark_group("preprocess");

    for (backend_name, execution_mode) in execution_modes {
        let wasm_config = WasmConfig {
            execution_mode,
            ..Default::default()
        };

        let engine = WasmEngine::new(wasm_config);

        // let module_bytes = cpu_burner_br_if(10);
        let module_bytes = make_do_nothing_bytes();
        let baton = (module_bytes, engine);

        group.bench_with_input(
            BenchmarkId::new("do_nothing", backend_name),
            &baton,
            |b, (do_nothing_bytes, engine)| {
                b.iter(|| {
                    let _wasm_module = engine.preprocess(None, do_nothing_bytes).unwrap();
                    // let _instance = engine
                    //     .instance_and_memory(wasm_module, MockHost::new())
                    //     .unwrap();
                    // let _result = instance
                    //     .invoke_export(None, engine, DEFAULT_ENTRY_POINT_NAME, Vec::new())
                    //     .unwrap();
                });
            },
        );
    }

    // let mut group = c.benchmark_group("preprocess");

    for (backend_name, execution_mode) in execution_modes {
        let wasm_config = WasmConfig {
            execution_mode,
            ..Default::default()
        };

        for step in (2..13).map(|exp| 2u32.pow(exp)) {
            let module_bytes = fixed_cost_br_table(step as usize, step);
            let engine = WasmEngine::new(wasm_config);

            let baton = (module_bytes, engine);

            group.bench_with_input(
                BenchmarkId::new(&format!("br_table/{}", step), backend_name),
                &baton,
                |b, (do_nothing_bytes, engine)| {
                    b.iter(|| {
                        let _wasm_module = engine.preprocess(None, do_nothing_bytes).unwrap();
                        // let _instance = engine
                        //     .instance_and_memory(wasm_module, MockHost::new())
                        //     .unwrap();
                        // let _result = instance
                        //     .invoke_export(None, engine, DEFAULT_ENTRY_POINT_NAME, Vec::new())
                        //     .unwrap();
                    });
                },
            );
        }
    }
}

criterion::criterion_group!(benches, cold_start);
criterion::criterion_main!(benches);
