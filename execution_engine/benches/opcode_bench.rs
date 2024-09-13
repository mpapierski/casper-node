use std::fs;

use casper_execution_engine::{
    engine_state::EngineConfig,
    runtime::{preprocess, utils, PreprocessConfigBuilder},
};
use casper_types::{ProtocolVersion, WasmConfig};
use casper_wasmi::NopExternals;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// Run all the benchmark or just the first in a group.
const RUN_ALL_BENCHMARKS: bool = true;

/// Returns either the first or all the benchmarks.
pub fn first_or_all<'a>(all: &'a [&'a str]) -> &'a [&'a str] {
    if RUN_ALL_BENCHMARKS {
        all
    } else {
        &all[..1]
    }
}

/// Creates a benchmark with its confirmation for the specified `code` snippet.
///
/// The confirmation benchmark is to make sure there is no compiler optimization
/// for the repeated lines of code.
pub fn benchmark_with_confirmation(name: &str, code: &str) -> Vec<Benchmark> {
    let i = DEFAULT_LOOP_ITERATIONS;
    let r = DEFAULT_REPEAT_TIMES;
    let c = CONFIRMATION_REPEAT_TIMES;
    vec![
        benchmark(name, i, r, code),
        benchmark(&format!("{name}/confirmation"), i, c, code),
    ]
}

/// Creates a benchmark with its confirmation for the specified `code` snippet.
///
/// The confirmation benchmark is to make sure there is no compiler optimization
/// for the loop.
pub fn benchmark_with_loop_confirmation(name: &str, code: &str) -> Vec<Benchmark> {
    let i = DEFAULT_LOOP_ITERATIONS;
    let c = CONFIRMATION_LOOP_ITERATIONS;
    let r = DEFAULT_REPEAT_TIMES;
    vec![
        benchmark(name, i, r, code),
        benchmark(&format!("{name}/confirmation"), c, r, code),
    ]
}

/// Creates a benchmark with a code block repeated specified number of times in a loop.
pub fn benchmark(name: &str, i: usize, r: usize, repeat_code: &str) -> Benchmark {
    Benchmark(
        name.into(),
        Block::default()
            .repeat_n(r, repeat_code)
            .loop_n(i)
            .define_variables_and_functions(repeat_code)
            .into_update_func()
            .into_test_module_wat(),
        (i * r) as u64,
    )
}

#[derive(Debug)]
pub struct Benchmark(pub String, pub String, pub u64);

///
/// The new WAT builder.
//

/// Default number of loop iterations.
pub const DEFAULT_LOOP_ITERATIONS: usize = 1_000;
/// Default number of repeat times.
pub const DEFAULT_REPEAT_TIMES: usize = 7_000;
/// Number of loop iterations to confirm the result.
///
/// The main overhead comes from the call itself, so 1000 times more loop iterations
/// take just 4 time more in the wall time.
pub const CONFIRMATION_LOOP_ITERATIONS: usize = 1_000_000;
/// Number of repeat times to confirm the result.
///
/// The idea behind the confirmation is that the same operation but repeated twice
/// should take roughly two times more time to execute, i.e. there are no optimizations.
///
/// Note, the maximum compilation complexity is 15K.
pub const CONFIRMATION_REPEAT_TIMES: usize = 14_000;

////////////////////////////////////////////////////////////////////////
/// WAT Block Builder

/// Represent a block of WAT code with corresponding imports and local variables.
#[derive(Default)]
pub struct Block {
    imports: Vec<String>,
    locals: Vec<String>,
    lines: Vec<String>,
}

impl Block {
    /// Add a new `line` of code.
    pub fn line(&mut self, code: &str) -> &mut Self {
        self.lines.push(code.into());
        self
    }

    /// Add a new `import`.
    pub fn import(&mut self, code: &str) -> &mut Self {
        self.imports.push(code.into());
        self
    }

    /// Add a new `local`.
    pub fn local(&mut self, code: &str) -> &mut Self {
        self.locals.push(code.into());
        self
    }

    /// Loop the current block of code for `n` iterations.
    pub fn loop_n(mut self, n: usize) -> Self {
        self.local("(local $i i32)");

        self.lines = wrap_lines(
            &format!("(local.set $i (i32.const {n})) (loop $loop"),
            self.lines,
            "(br_if $loop (local.tee $i (i32.sub (local.get $i) (i32.const 1)))))",
        );

        self
    }

    /// Repeat the line of code `n` times.
    pub fn repeat_n(mut self, n: usize, code: &str) -> Self {
        for _ in 0..n {
            self.line(code);
        }
        self
    }

    /// Define variables and functions used in the `code` snippet.
    pub fn define_variables_and_functions(mut self, code: &str) -> Self {
        for name in ["x", "y", "z", "zero", "address", "one"] {
            for ty in ["i32", "i64", "f32", "f64", "v128"] {
                if code.contains(&format!("${name}_{ty}")) {
                    self.declare_variable(name, ty);
                }
            }
        }
        if code.contains("$empty") {
            self.import("(func $empty (result i32) (i32.const 0))");
        }
        if code.contains("$empty_return_call") {
            self.import("(func $empty_return_call (result i32) return_call $empty)");
        }
        if code.contains("$result_i32") {
            self.import("(type $result_i32 (func (result i32)))")
                .import("(func $empty_indirect (type $result_i32) (i32.const 0))")
                .import("(table 10 funcref)")
                .import("(elem (i32.const 7) $empty_indirect)");
        }
        self
    }

    /// Declare a `black_box` variable with specified `name` and `type`.
    pub fn declare_variable(&mut self, name: &str, ty: &str) -> &mut Self {
        let init_val = match name {
            "x" => "1000000007",
            "y" => "1337",
            "z" => "2147483647",
            "zero" => "0",
            "address" => "16",
            "one" => "1",
            _ => panic!("Error getting initial value for variable {name}"),
        };
        let var = format!("${name}_{ty}");
        let init_val = if ty != "v128" {
            init_val.into()
        } else {
            format!("i64x2 {init_val} {init_val}")
        };
        self.import(&format!(
            "(global {var} (mut {ty}) ({ty}.const {init_val}))"
        ))
        .local(&format!("(local {var} {ty})"));
        self.lines
            .insert(0, format!("(local.set {var} (global.get {var}))"));
        self.line(&format!("(global.set {var} (local.get {var}))"));
        self
    }

    /// Transform the block into an update function.
    pub fn into_update_func(self) -> Func {
        Func {
            imports: self.imports,
            lines: wrap_lines(
                r#"(func $test (export "canister_update test")"#,
                [self.locals, self.lines].concat(),
                ")",
            ),
        }
    }
}

////////////////////////////////////////////////////////////////////////
/// WAT Function Builder

/// Represent a WAT function with corresponding imports.
#[derive(Default)]
pub struct Func {
    imports: Vec<String>,
    lines: Vec<String>,
}

impl Func {
    /// Transform the function into a test module WAT representation.
    pub fn into_test_module_wat(self) -> String {
        wrap_lines(
            "(module",
            [
                self.imports,
                vec![
                    "(table $table 10 funcref)".into(),
                    // "(elem func 0)".into(),
                    "(elem $table (i32.const 0) func 0)".into(),
                    "(memory $mem 1)".into(),
                ],
                self.lines,
            ]
            .concat(),
            ")",
        )
        .join("\n")
    }
}

////////////////////////////////////////////////////////////////////////
/// Helper functions

/// Return a new block prepended and appended with the specified lines.
fn wrap_lines(prefix: &str, lines: Vec<String>, suffix: &str) -> Vec<String> {
    vec![prefix.into()]
        .into_iter()
        .chain(lines.into_iter().map(|l| format!("    {l}")))
        .chain(vec![suffix.into()])
        .collect()
}

/// Return the destination type for the given operation, i.e. for `i32.wrap_i64` returns `i32`
pub fn dst_type(op: &str) -> &'static str {
    if op.starts_with("i64") {
        return "i64";
    } else if op.starts_with("f32") {
        return "f32";
    } else if op.starts_with("f64") {
        return "f64";
    } else if op.starts_with("v128") {
        return "v128";
    }
    // Fallback to i32 type.
    "i32"
}

/// Return the source type for the given operation, i.e. for `i32.wrap_i64` returns `i64`
pub fn src_type(op: &str) -> &'static str {
    if op.contains("_i32") {
        return "i32";
    } else if op.contains("_i64") {
        return "i64";
    } else if op.contains("_f32") {
        return "f32";
    } else if op.contains("_f64") {
        return "f64";
    }
    // Fallback to the destination type, i.e. for `i64.eqz` returns `i64`.
    dst_type(op)
}

pub fn benchmarks() -> Vec<Benchmark> {
    // List of benchmarks to run.
    let mut benchmarks = vec![];

    ////////////////////////////////////////////////////////////////////
    // Overhead Benchmark

    // The bench is an empty loop: `nop`
    // All we need to capture in this benchmark is the call and loop overhead.
    benchmarks.extend(benchmark_with_loop_confirmation("overhead", "(nop)"));

    ////////////////////////////////////////////////////////////////////
    // Numeric Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#numeric-instructions

    // Constants: `$x_{type} = ({op} u8)`
    // The throughput for the following benchmarks is ~2.8 Gops/s
    for op in first_or_all(&["i32.const", "i64.const"]) {
        let ty = dst_type(op);
        let name = format!("const/{op}");
        let code = &format!("(global.set $x_{ty} ({op} 7))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~2.2 Gops/s
    for op in first_or_all(&["f32.const"]) {
        let ty = dst_type(op);
        let name = format!("const/{op}");
        let code = &format!("(global.set $x_{ty} ({op} 7))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~1.5 Gops/s
    for op in first_or_all(&["f64.const"]) {
        let ty = dst_type(op);
        let name = format!("const/{op}");
        let code = &format!("(global.set $x_{ty} ({op} 7))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Integer Unary Operators (iunop): `$x_{type} = ({op} $x_{type})`
    // The throughput for the following benchmarks is ~2.8 Gops/s
    for op in first_or_all(&[
        "i32.clz",
        "i32.ctz",
        "i32.popcnt",
        "i64.clz",
        "i64.ctz",
        "i64.popcnt",
    ]) {
        let ty = dst_type(op);
        let name = format!("iunop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Floating-Point Unary Operators (funop): `$x_{type} = ({op} $x_{type})`
    // The throughput for the following benchmarks is ~1.9 Gops/s
    for op in first_or_all(&["f32.abs", "f32.neg"]) {
        let ty = dst_type(op);
        let name = format!("funop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~1.3 Gops/s
    for op in first_or_all(&["f64.abs", "f64.neg"]) {
        let ty = dst_type(op);
        let name = format!("funop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.07 Gops/s
    for op in first_or_all(&[
        "f32.ceil",
        "f32.floor",
        "f32.trunc",
        "f32.nearest",
        "f64.ceil",
        "f64.floor",
        "f64.trunc",
        "f64.nearest",
    ]) {
        let ty = dst_type(op);
        let name = format!("funop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.05 Gops/s
    for op in first_or_all(&["f32.sqrt", "f64.sqrt"]) {
        let ty = dst_type(op);
        let name = format!("funop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Integer Binary Operators (ibinop): `$x_{type} = ({op} $x_{type} $y_{type})`
    // The throughput for the following benchmarks is ~2.8 Gops/s
    for op in first_or_all(&[
        "i32.add",
        "i32.sub",
        "i32.mul",
        "i32.and",
        "i32.or",
        "i32.xor",
        "i32.shl",
        "i32.shr_s",
        "i32.shr_u",
        "i32.rotl",
        "i32.rotr",
        "i64.add",
        "i64.sub",
        "i64.mul",
        "i64.and",
        "i64.or",
        "i64.xor",
        "i64.shl",
        "i64.shr_s",
        "i64.shr_u",
        "i64.rotl",
        "i64.rotr",
    ]) {
        let ty = dst_type(op);
        let name = format!("ibinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.1 Gops/s
    for op in first_or_all(&[
        "i32.div_s",
        "i32.div_u",
        "i32.rem_s",
        "i32.rem_u",
        "i64.div_s",
        "i64.div_u",
        "i64.rem_s",
        "i64.rem_u",
    ]) {
        let ty = dst_type(op);
        let name = format!("ibinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Floating-Point Binary Operators (fbinop): `$x_{type} = ({op} $x_{type} $y_{type})`
    // The throughput for the following benchmarks is ~0.07 Gops/s
    for op in first_or_all(&[
        "f32.add", "f32.sub", "f32.mul", "f64.add", "f64.sub", "f64.mul",
    ]) {
        let ty = dst_type(op);
        let name = format!("fbinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.06 Gops/s
    for op in first_or_all(&["f32.div", "f64.div"]) {
        let ty = dst_type(op);
        let name = format!("fbinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.04 Gops/s
    for op in first_or_all(&["f32.min", "f32.max", "f64.min", "f64.max"]) {
        let ty = dst_type(op);
        let name = format!("fbinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~1.5 Gops/s
    for op in first_or_all(&["f32.copysign"]) {
        let ty = dst_type(op);
        let name = format!("fbinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~1.0 Gops/s
    for op in first_or_all(&["f64.copysign"]) {
        let ty = dst_type(op);
        let name = format!("fbinop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Integer Test Operators (itestop): `$x_i32 = ({op} $x_{type})`
    // The throughput for the following benchmarks is ~2.5 Gops/s
    for op in first_or_all(&["i32.eqz", "i64.eqz"]) {
        let ty = dst_type(op);
        let name = format!("itestop/{op}");
        let code = &format!("(global.set $x_i32 ({op} (local.get $x_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Integer Relational Operators (irelop): `$x_i32 = ({op} $x_{type} $y_{type})`
    // The throughput for the following benchmarks is ~2.6 Gops/s
    for op in first_or_all(&[
        "i32.eq", "i32.ne", "i32.lt_s", "i32.lt_u", "i32.gt_s", "i32.gt_u", "i32.le_s", "i32.le_u",
        "i32.ge_s", "i32.ge_u", "i64.eq", "i64.ne", "i64.lt_s", "i64.lt_u", "i64.gt_s", "i64.gt_u",
        "i64.le_s", "i64.le_u", "i64.ge_s", "i64.ge_u",
    ]) {
        let ty = dst_type(op);
        let name = format!("irelop/{op}");
        let code = &format!("(global.set $x_i32 ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Floating-Point Relational Operators (frelop): `$x_i32 = ({op} $x_{type} $y_{type})`
    // The throughput for the following benchmarks is ~1.4 Gops/s
    for op in first_or_all(&["f32.eq", "f32.ne", "f64.eq", "f64.ne"]) {
        let ty = dst_type(op);
        let name = format!("frelop/{op}");
        let code = &format!("(global.set $x_i32 ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~2.1 Gops/s
    for op in first_or_all(&[
        "f32.lt", "f32.gt", "f32.le", "f32.ge", "f64.lt", "f64.gt", "f64.le", "f64.ge",
    ]) {
        let ty = dst_type(op);
        let name = format!("frelop/{op}");
        let code = &format!("(global.set $x_i32 ({op} (local.get $x_{ty}) (local.get $y_{ty})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Numeric Conversions (cvtop): `$x_{type} = ({op} $x_{src_type})`
    // The throughput for the following benchmarks is ~2.9 Gops/s
    for op in first_or_all(&[
        "i32.extend8_s",
        "i32.extend16_s",
        "i64.extend8_s",
        "i64.extend16_s",
        "f32.convert_i32_s",
        "f32.convert_i64_s",
        "f64.convert_i32_s",
        "f64.convert_i64_s",
        "i64.extend32_s",
        "i32.wrap_i64",
        "i64.extend_i32_s",
        "i64.extend_i32_u",
        "f32.demote_f64",
        "f64.promote_f32",
        "f32.reinterpret_i32",
        "f64.reinterpret_i64",
    ]) {
        let ty = dst_type(op);
        let src_type = src_type(op);
        let name = format!("cvtop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{src_type})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~2.2 Gops/s
    for op in first_or_all(&[
        "f32.convert_i32_u",
        "f64.convert_i32_u",
        "i32.reinterpret_f32",
        "i64.reinterpret_f64",
    ]) {
        let ty = dst_type(op);
        let src_type = src_type(op);
        let name = format!("cvtop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{src_type})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.1 Gops/s
    for op in first_or_all(&[
        "i32.trunc_f32_s",
        "i32.trunc_f32_u",
        "i32.trunc_f64_s",
        "i32.trunc_f64_u",
        "i64.trunc_f32_s",
        "i64.trunc_f32_u",
        "i64.trunc_f64_s",
        "i64.trunc_f64_u",
        "i64.trunc_sat_f32_s",
        "i64.trunc_sat_f64_s",
    ]) {
        let ty = dst_type(op);
        let src_type = src_type(op);
        let name = format!("cvtop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{src_type})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.06 Gops/s
    for op in first_or_all(&[
        "i32.trunc_sat_f32_u",
        "i32.trunc_sat_f64_u",
        "i64.trunc_sat_f32_u",
        "i64.trunc_sat_f64_u",
    ]) {
        let ty = dst_type(op);
        let src_type = src_type(op);
        let name = format!("cvtop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{src_type})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }
    // The throughput for the following benchmarks is ~0.2 Gops/s
    for op in first_or_all(&[
        "i32.trunc_sat_f32_s",
        "i32.trunc_sat_f64_s",
        "f32.convert_i64_u",
        "f64.convert_i64_u",
    ]) {
        let ty = dst_type(op);
        let src_type = src_type(op);
        let name = format!("cvtop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $x_{src_type})))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    ////////////////////////////////////////////////////////////////////
    // Reference Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#reference-instructions

    // The throughput for the following benchmarks is ~0.02 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "refop/ref.func",
        "(drop (ref.func 0))",
    ));
    // The throughput for the following benchmarks is ~0.02 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "refop/ref.is_null-ref.func",
        "(global.set $x_i32 (ref.is_null (ref.func 0)))",
    ));

    ////////////////////////////////////////////////////////////////////
    // Variable Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#variable-instructions

    // Get Variable Instructions: `$x_i32 = ({op} x_i32)`
    // The throughput for the following benchmarks is ~2.9 Gops/s
    for op in first_or_all(&["local.get", "global.get"]) {
        let name = format!("varop/{op}");
        let code = &format!("(global.set $x_i32 ({op} $x_i32))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Set Variable Instructions: `({op} x_i32 $x_i32)`
    // The throughput for the following benchmarks is ~5.5 Gops/s, as it
    // just stores a register into the memory.
    // The benchmark is commented out, as otherwise it becomes a baseline and
    // skews all the results.
    // for op in first_or_all(&["local.set"]) {
    //     let name = format!("varop/{op}");
    //     let code = &format!("({op} $x_i32 (global.get $x_i32))");
    //     benchmarks.extend(benchmark_with_confirmation(&name, code));
    // }
    // The throughput for the following benchmarks is ~2.9 Gops/s
    for op in first_or_all(&["global.set"]) {
        let name = format!("varop/{op}");
        let code = &format!("({op} $x_i32 (local.get $x_i32))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Tee Variable Instructions: `$x_i32 = ({op} x_i32 $x_i32)`
    // The throughput for the following benchmarks is ~2.9 Gops/s
    for op in first_or_all(&["local.tee"]) {
        let name = format!("varop/{op}");
        let code = &format!("(global.set $x_i32 ({op} $x_i32 (local.get $x_i32)))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    ////////////////////////////////////////////////////////////////////
    // Table Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#table-instructions

    // The throughput for the following benchmarks is ~0.7 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "tabop/table.get",
        "(drop (table.get $table (local.get $zero_i32)))",
    ));
    // The throughput for the following benchmarks is ~2.8 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "tabop/table.size",
        "(global.set $x_i32 (table.size))",
    ));

    ////////////////////////////////////////////////////////////////////
    // Memory Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#memory-instructions

    // Load: `$x_{type} = ({op} $address_i32))`
    // The throughput for the following benchmarks is ~2.0 Gops/s
    for op in first_or_all(&["i32.load", "i64.load", "f32.load", "f64.load"]) {
        let ty = dst_type(op);
        let name = format!("memop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $address_i32)))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Store: `({op} $address_i32 $x_{type})`
    // The throughput for the following benchmarks is ~2.2 Gops/s
    for op in first_or_all(&["i32.store", "i64.store", "f32.store", "f64.store"]) {
        let ty = dst_type(op);
        let name = format!("memop/{op}");
        let code = &format!("({op} (local.get $address_i32) (local.get $x_{ty}))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Extending Load: `$x_{type} = ({op} $address_i32))`
    // The throughput for the following benchmarks is ~2.1 Gops/s
    for op in first_or_all(&[
        "i32.load8_s",
        "i32.load8_u",
        "i32.load16_s",
        "i32.load16_u",
        "i64.load8_s",
        "i64.load8_u",
        "i64.load16_s",
        "i64.load16_u",
        "i64.load32_s",
        "i64.load32_u",
    ]) {
        let ty = dst_type(op);
        let name = format!("memop/{op}");
        let code = &format!("(global.set $x_{ty} ({op} (local.get $address_i32)))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Wrapping Store: `({op} $address_i32 $x_{type})`
    // The throughput for the following benchmarks is ~2.2 Gops/s
    for op in first_or_all(&[
        "i32.store8",
        "i32.store16",
        "i64.store8",
        "i64.store16",
        "i64.store32",
    ]) {
        let ty = dst_type(op);
        let name = format!("memop/{op}");
        let code = &format!("({op} (local.get $address_i32) (local.get $x_{ty}))");
        benchmarks.extend(benchmark_with_confirmation(&name, code));
    }

    // Memory Instructions: Bulk Memory Operations
    // The throughput for the following benchmarks is ~0.2 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "memop/memory.size",
        "(global.set $x_i32 (memory.size))",
    ));
    // The throughput for the following benchmarks is ~0.006 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "memop/memory.grow",
        "(global.set $x_i32 (memory.grow (local.get $zero_i32)))",
    ));
    // The throughput for the following benchmarks is ~0.03 Gops/s
    // benchmarks.extend(benchmark_with_confirmation(
    //     "memop/memory.fill",
    //     "(memory.fill (local.get $zero_i32) (local.get $zero_i32) (local.get $zero_i32))",
    // ));
    // The throughput for the following benchmarks is ~0.02 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "memop/memory.copy",
        "(memory.copy (local.get $zero_i32) (local.get $zero_i32) (local.get $zero_i32))",
    ));

    ////////////////////////////////////////////////////////////////////
    // Control Instructions
    // See: https://www.w3.org/TR/wasm-core-2/#control-instructions

    // The throughput for the following benchmarks is ~1.4 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "ctrlop/select",
        "(global.set $x_i32 (select (global.get $zero_i32) (global.get $x_i32) (global.get $y_i32)))",
    ));
    // The throughput for the following benchmarks is ~0.2 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "ctrlop/call",
        "(global.set $x_i32 (call $empty))",
    ));
    // The throughput for the following benchmarks is ~0.1 Gops/s
    benchmarks.extend(benchmark_with_confirmation(
        "ctrlop/call_indirect",
        "(global.set $x_i32 (call_indirect (type $result_i32) (i32.const 7)))",
    ));

    benchmarks
}

/// List of benchmarks targeting unsupported extensions.
const SKIP_LIST: &[&str] = &[
    "cvtop/i32.extend8_s", /*  fail: Deserialization error: Sign extension operations are not
                            * supported */
    "cvtop/i32.extend16_s", /*  fail: Deserialization error: Sign extension operations are not
                             * supported */
    "cvtop/i64.extend8_s", /*  fail: Deserialization error: Sign extension operations are not
                            * supported */
    "cvtop/i64.extend16_s", /*  fail: Deserialization error: Sign extension operations are not
                             * supported */
    "cvtop/i64.extend32_s", /*  fail: Deserialization error: Sign extension operations are not
                             * supported */
    "cvtop/i64.trunc_sat_f32_s", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i64.trunc_sat_f64_s", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i32.trunc_sat_f32_u", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i32.trunc_sat_f64_u", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i64.trunc_sat_f32_u", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i64.trunc_sat_f64_u", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i32.trunc_sat_f32_s", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "cvtop/i32.trunc_sat_f64_s", /*  fail: Deserialization error: Bulk memory operations are not
                                  * supported */
    "refop/ref.func",             //  fail: Deserialization error: Unknown opcode 210
    "refop/ref.is_null-ref.func", //  fail: Deserialization error: Unknown opcode 210
    "tabop/table.get",            //  fail: Deserialization error: Unknown opcode 37
    "tabop/table.size",           /*  fail: Deserialization error: Bulk memory operations are
                                   * not supported */
    "memop/memory.copy", //  fail: Deserialization error: Bulk memory operations are not supported
    "ctrlop/call_indirect", /*  fail: Wasm validation error: the number of tables must be at
                          * most one */
];

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut benchmarks = benchmarks();

    benchmarks.retain(|Benchmark(id, _, _)| !SKIP_LIST.contains(&id.as_str()));

    let group = "wasm_instructions";

    for Benchmark(id, wat, expected_ops) in benchmarks {
        let mut group = c.benchmark_group(group);
        let mut bench_args = None;
        let id = id.as_str();
        group
            .throughput(criterion::Throughput::Elements(expected_ops))
            .bench_function(id, |b| {
                b.iter_batched(
                    || {
                        // Lazily setup the benchmark arguments
                        let value = bench_args.get_or_insert({
                            let wasm_bytes = wat::parse_str(&wat).unwrap();
                            let preprocess_config = PreprocessConfigBuilder::default()
                                .with_externalize_memory(true)
                                .with_gas_counter(false)
                                .with_require_memory(false)
                                .with_stack_height_limiter(false)
                                .build();
                            let wat_copy = wat.clone();
                            let module =
                                preprocess(WasmConfig::default(), &wasm_bytes, preprocess_config)
                                    .unwrap_or_else(|error| {
                                        fs::write(
                                            format!("/tmp/wasm_instructions_failure.wat"),
                                            wat_copy,
                                        )
                                        .unwrap();
                                        panic!("Error {error} in {id}");
                                    });
                            let (instance, memory) = utils::instance_and_memory(
                                module.clone(),
                                ProtocolVersion::V1_0_0,
                                &EngineConfig::default(),
                            )
                            .unwrap();

                            (instance, memory)
                        });

                        value.clone()
                    },
                    |(instance, _memory)| match instance.invoke_export(
                        "canister_update test",
                        &[],
                        &mut NopExternals,
                    ) {
                        Ok(_) => {}
                        Err(trap) => {
                            fs::write("/tmp/fail.wat", &wat).unwrap();
                            panic!("Error: {:?}", trap);
                        }
                    },
                    BatchSize::SmallInput,
                );
            });
        group.finish();
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
