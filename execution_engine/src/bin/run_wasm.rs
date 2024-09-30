use std::{
    fs, path::{Path, PathBuf}, time::{Duration, Instant}
};

use casper_types::{WasmConfig, DEFAULT_WASM_MAX_MEMORY};

use casper_execution_engine::runtime::{self, PreprocessConfigBuilder, RuledOpcodeCosts};
use casper_wasmi::{
    memory_units::Pages, Externals, FuncInstance, HostError, ImportsBuilder, MemoryInstance, ModuleImportResolver, ModuleInstance, RuntimeValue, Signature
};

struct ParityRules(RuledOpcodeCosts);

fn convert_parity_block_type(parity_block_type: wasm_instrument::parity_wasm::elements::BlockType) -> casper_wasm::elements::BlockType {
    match parity_block_type {
        wasm_instrument::parity_wasm::elements::BlockType::Value(wasm_instrument::parity_wasm::elements::ValueType::F32) => casper_wasm::elements::BlockType::Value(casper_wasm::elements::ValueType::F32),
        wasm_instrument::parity_wasm::elements::BlockType::Value(wasm_instrument::parity_wasm::elements::ValueType::F64) => casper_wasm::elements::BlockType::Value(casper_wasm::elements::ValueType::F64),
        wasm_instrument::parity_wasm::elements::BlockType::Value(wasm_instrument::parity_wasm::elements::ValueType::I32) => casper_wasm::elements::BlockType::Value(casper_wasm::elements::ValueType::I32),
        wasm_instrument::parity_wasm::elements::BlockType::Value(wasm_instrument::parity_wasm::elements::ValueType::I64) => casper_wasm::elements::BlockType::Value(casper_wasm::elements::ValueType::I64),
        wasm_instrument::parity_wasm::elements::BlockType::NoResult => casper_wasm::elements::BlockType::NoResult,
    }
}

impl wasm_instrument::gas_metering::Rules for ParityRules {
    fn instruction_cost(&self, parity_instruction: &wasm_instrument::parity_wasm::elements::Instruction) -> Option<u32> {
        let casper_instruction = match parity_instruction {
            wasm_instrument::parity_wasm::elements::Instruction::Unreachable => casper_wasm::elements::Instruction::Unreachable,
            wasm_instrument::parity_wasm::elements::Instruction::Nop => casper_wasm::elements::Instruction::Nop,
            wasm_instrument::parity_wasm::elements::Instruction::Block(block_type) => casper_wasm::elements::Instruction::Block(convert_parity_block_type(*block_type)),
            wasm_instrument::parity_wasm::elements::Instruction::Loop(block_type) => casper_wasm::elements::Instruction::Loop(convert_parity_block_type(*block_type)),
            wasm_instrument::parity_wasm::elements::Instruction::If(block_type) => casper_wasm::elements::Instruction::If(convert_parity_block_type(*block_type)),
            wasm_instrument::parity_wasm::elements::Instruction::Else => casper_wasm::elements::Instruction::Else,
            wasm_instrument::parity_wasm::elements::Instruction::End => casper_wasm::elements::Instruction::End,
            wasm_instrument::parity_wasm::elements::Instruction::Br(br) => casper_wasm::elements::Instruction::Br(*br),
            wasm_instrument::parity_wasm::elements::Instruction::BrIf(br_if) => casper_wasm::elements::Instruction::BrIf(*br_if),
            wasm_instrument::parity_wasm::elements::Instruction::BrTable(br_table_data) => {

                let casper_br_table_data = casper_wasm::elements::BrTableData {
                    table: br_table_data.table.clone(),
                    default: br_table_data.default.clone(),
                };

                casper_wasm::elements::Instruction::BrTable(Box::new(casper_br_table_data))

            }
            wasm_instrument::parity_wasm::elements::Instruction::Return => casper_wasm::elements::Instruction::Return,
            wasm_instrument::parity_wasm::elements::Instruction::Call(call) => casper_wasm::elements::Instruction::Call(*call),
            wasm_instrument::parity_wasm::elements::Instruction::CallIndirect(a, b) => casper_wasm::elements::Instruction::CallIndirect(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::Drop => casper_wasm::elements::Instruction::Drop,
            wasm_instrument::parity_wasm::elements::Instruction::Select => casper_wasm::elements::Instruction::Select,
            wasm_instrument::parity_wasm::elements::Instruction::GetLocal(a) => casper_wasm::elements::Instruction::GetLocal(*a),
            wasm_instrument::parity_wasm::elements::Instruction::SetLocal(a) => casper_wasm::elements::Instruction::SetLocal(*a),
            wasm_instrument::parity_wasm::elements::Instruction::TeeLocal(a) => casper_wasm::elements::Instruction::TeeLocal(*a),
            wasm_instrument::parity_wasm::elements::Instruction::GetGlobal(a) => casper_wasm::elements::Instruction::GetGlobal(*a),
            wasm_instrument::parity_wasm::elements::Instruction::SetGlobal(a) => casper_wasm::elements::Instruction::SetGlobal(*a),
            wasm_instrument::parity_wasm::elements::Instruction::I32Load(a, b) => casper_wasm::elements::Instruction::I32Load(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load(a, b) => casper_wasm::elements::Instruction::I64Load(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::F32Load(a, b) => casper_wasm::elements::Instruction::F32Load(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::F64Load(a, b) => casper_wasm::elements::Instruction::F64Load(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Load8S(a, b) => casper_wasm::elements::Instruction::I32Load8S(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Load8U(a, b) => casper_wasm::elements::Instruction::I32Load8U(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Load16S(a, b) => casper_wasm::elements::Instruction::I32Load16S(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Load16U(a, b) => casper_wasm::elements::Instruction::I32Load16U(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load8S(a, b) => casper_wasm::elements::Instruction::I64Load8S(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load8U(a, b) => casper_wasm::elements::Instruction::I64Load8U(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load16S(a, b) => casper_wasm::elements::Instruction::I64Load16S(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load16U(a, b) => casper_wasm::elements::Instruction::I64Load16U(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load32S(a, b) => casper_wasm::elements::Instruction::I64Load32S(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Load32U(a, b) => casper_wasm::elements::Instruction::I64Load32U(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Store(a, b) => casper_wasm::elements::Instruction::I32Store(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Store(a, b) => casper_wasm::elements::Instruction::I64Store(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::F32Store(a, b) => casper_wasm::elements::Instruction::F32Store(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::F64Store(a, b) => casper_wasm::elements::Instruction::F64Store(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Store8(a, b) => casper_wasm::elements::Instruction::I32Store8(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I32Store16(a, b) => casper_wasm::elements::Instruction::I32Store16(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Store8(a, b) => casper_wasm::elements::Instruction::I64Store8(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Store16(a, b) => casper_wasm::elements::Instruction::I64Store16(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::I64Store32(a, b) => casper_wasm::elements::Instruction::I64Store32(*a, *b),
            wasm_instrument::parity_wasm::elements::Instruction::CurrentMemory(a) => casper_wasm::elements::Instruction::CurrentMemory(*a),
            wasm_instrument::parity_wasm::elements::Instruction::GrowMemory(a) => casper_wasm::elements::Instruction::GrowMemory(*a),
            wasm_instrument::parity_wasm::elements::Instruction::I32Const(a) => casper_wasm::elements::Instruction::I32Const(*a),
            wasm_instrument::parity_wasm::elements::Instruction::I64Const(a) => casper_wasm::elements::Instruction::I64Const(*a),
            wasm_instrument::parity_wasm::elements::Instruction::F32Const(a) => casper_wasm::elements::Instruction::F32Const(*a),
            wasm_instrument::parity_wasm::elements::Instruction::F64Const(a) => casper_wasm::elements::Instruction::F64Const(*a),
            wasm_instrument::parity_wasm::elements::Instruction::I32Eqz => casper_wasm::elements::Instruction::I32Eqz,
            wasm_instrument::parity_wasm::elements::Instruction::I32Eq => casper_wasm::elements::Instruction::I32Eq,
            wasm_instrument::parity_wasm::elements::Instruction::I32Ne => casper_wasm::elements::Instruction::I32Ne,
            wasm_instrument::parity_wasm::elements::Instruction::I32LtS => casper_wasm::elements::Instruction::I32LtS,
            wasm_instrument::parity_wasm::elements::Instruction::I32LtU => casper_wasm::elements::Instruction::I32LtU,
            wasm_instrument::parity_wasm::elements::Instruction::I32GtS => casper_wasm::elements::Instruction::I32GtS,
            wasm_instrument::parity_wasm::elements::Instruction::I32GtU => casper_wasm::elements::Instruction::I32GtU,
            wasm_instrument::parity_wasm::elements::Instruction::I32LeS => casper_wasm::elements::Instruction::I32LeS,
            wasm_instrument::parity_wasm::elements::Instruction::I32LeU => casper_wasm::elements::Instruction::I32LeU,
            wasm_instrument::parity_wasm::elements::Instruction::I32GeS => casper_wasm::elements::Instruction::I32GeS,
            wasm_instrument::parity_wasm::elements::Instruction::I32GeU => casper_wasm::elements::Instruction::I32GeU,
            wasm_instrument::parity_wasm::elements::Instruction::I64Eqz => casper_wasm::elements::Instruction::I64Eqz,
            wasm_instrument::parity_wasm::elements::Instruction::I64Eq => casper_wasm::elements::Instruction::I64Eq,
            wasm_instrument::parity_wasm::elements::Instruction::I64Ne => casper_wasm::elements::Instruction::I64Ne,
            wasm_instrument::parity_wasm::elements::Instruction::I64LtS => casper_wasm::elements::Instruction::I64LtS,
            wasm_instrument::parity_wasm::elements::Instruction::I64LtU => casper_wasm::elements::Instruction::I64LtU,
            wasm_instrument::parity_wasm::elements::Instruction::I64GtS => casper_wasm::elements::Instruction::I64GtS,
            wasm_instrument::parity_wasm::elements::Instruction::I64GtU => casper_wasm::elements::Instruction::I64GtU,
            wasm_instrument::parity_wasm::elements::Instruction::I64LeS => casper_wasm::elements::Instruction::I64LeS,
            wasm_instrument::parity_wasm::elements::Instruction::I64LeU => casper_wasm::elements::Instruction::I64LeU,
            wasm_instrument::parity_wasm::elements::Instruction::I64GeS => casper_wasm::elements::Instruction::I64GeS,
            wasm_instrument::parity_wasm::elements::Instruction::I64GeU => casper_wasm::elements::Instruction::I64GeU,
            wasm_instrument::parity_wasm::elements::Instruction::F32Eq => casper_wasm::elements::Instruction::F32Eq,
            wasm_instrument::parity_wasm::elements::Instruction::F32Ne => casper_wasm::elements::Instruction::F32Ne,
            wasm_instrument::parity_wasm::elements::Instruction::F32Lt => casper_wasm::elements::Instruction::F32Lt,
            wasm_instrument::parity_wasm::elements::Instruction::F32Gt => casper_wasm::elements::Instruction::F32Gt,
            wasm_instrument::parity_wasm::elements::Instruction::F32Le => casper_wasm::elements::Instruction::F32Le,
            wasm_instrument::parity_wasm::elements::Instruction::F32Ge => casper_wasm::elements::Instruction::F32Ge,
            wasm_instrument::parity_wasm::elements::Instruction::F64Eq => casper_wasm::elements::Instruction::F64Eq,
            wasm_instrument::parity_wasm::elements::Instruction::F64Ne => casper_wasm::elements::Instruction::F64Ne,
            wasm_instrument::parity_wasm::elements::Instruction::F64Lt => casper_wasm::elements::Instruction::F64Lt,
            wasm_instrument::parity_wasm::elements::Instruction::F64Gt => casper_wasm::elements::Instruction::F64Gt,
            wasm_instrument::parity_wasm::elements::Instruction::F64Le => casper_wasm::elements::Instruction::F64Le,
            wasm_instrument::parity_wasm::elements::Instruction::F64Ge => casper_wasm::elements::Instruction::F64Ge,
            wasm_instrument::parity_wasm::elements::Instruction::I32Clz => casper_wasm::elements::Instruction::I32Clz,
            wasm_instrument::parity_wasm::elements::Instruction::I32Ctz => casper_wasm::elements::Instruction::I32Ctz,
            wasm_instrument::parity_wasm::elements::Instruction::I32Popcnt => casper_wasm::elements::Instruction::I32Popcnt,
            wasm_instrument::parity_wasm::elements::Instruction::I32Add => casper_wasm::elements::Instruction::I32Add,
            wasm_instrument::parity_wasm::elements::Instruction::I32Sub => casper_wasm::elements::Instruction::I32Sub,
            wasm_instrument::parity_wasm::elements::Instruction::I32Mul => casper_wasm::elements::Instruction::I32Mul,
            wasm_instrument::parity_wasm::elements::Instruction::I32DivS => casper_wasm::elements::Instruction::I32DivS,
            wasm_instrument::parity_wasm::elements::Instruction::I32DivU => casper_wasm::elements::Instruction::I32DivU,
            wasm_instrument::parity_wasm::elements::Instruction::I32RemS => casper_wasm::elements::Instruction::I32RemS,
            wasm_instrument::parity_wasm::elements::Instruction::I32RemU => casper_wasm::elements::Instruction::I32RemU,
            wasm_instrument::parity_wasm::elements::Instruction::I32And => casper_wasm::elements::Instruction::I32And,
            wasm_instrument::parity_wasm::elements::Instruction::I32Or => casper_wasm::elements::Instruction::I32Or,
            wasm_instrument::parity_wasm::elements::Instruction::I32Xor => casper_wasm::elements::Instruction::I32Xor,
            wasm_instrument::parity_wasm::elements::Instruction::I32Shl => casper_wasm::elements::Instruction::I32Shl,
            wasm_instrument::parity_wasm::elements::Instruction::I32ShrS => casper_wasm::elements::Instruction::I32ShrS,
            wasm_instrument::parity_wasm::elements::Instruction::I32ShrU => casper_wasm::elements::Instruction::I32ShrU,
            wasm_instrument::parity_wasm::elements::Instruction::I32Rotl => casper_wasm::elements::Instruction::I32Rotl,
            wasm_instrument::parity_wasm::elements::Instruction::I32Rotr => casper_wasm::elements::Instruction::I32Rotr,
            wasm_instrument::parity_wasm::elements::Instruction::I64Clz => casper_wasm::elements::Instruction::I64Clz,
            wasm_instrument::parity_wasm::elements::Instruction::I64Ctz => casper_wasm::elements::Instruction::I64Ctz,
            wasm_instrument::parity_wasm::elements::Instruction::I64Popcnt => casper_wasm::elements::Instruction::I64Popcnt,
            wasm_instrument::parity_wasm::elements::Instruction::I64Add => casper_wasm::elements::Instruction::I64Add,
            wasm_instrument::parity_wasm::elements::Instruction::I64Sub => casper_wasm::elements::Instruction::I64Sub,
            wasm_instrument::parity_wasm::elements::Instruction::I64Mul => casper_wasm::elements::Instruction::I64Mul,
            wasm_instrument::parity_wasm::elements::Instruction::I64DivS => casper_wasm::elements::Instruction::I64DivS,
            wasm_instrument::parity_wasm::elements::Instruction::I64DivU => casper_wasm::elements::Instruction::I64DivU,
            wasm_instrument::parity_wasm::elements::Instruction::I64RemS => casper_wasm::elements::Instruction::I64RemS,
            wasm_instrument::parity_wasm::elements::Instruction::I64RemU => casper_wasm::elements::Instruction::I64RemU,
            wasm_instrument::parity_wasm::elements::Instruction::I64And => casper_wasm::elements::Instruction::I64And,
            wasm_instrument::parity_wasm::elements::Instruction::I64Or => casper_wasm::elements::Instruction::I64Or,
            wasm_instrument::parity_wasm::elements::Instruction::I64Xor => casper_wasm::elements::Instruction::I64Xor,
            wasm_instrument::parity_wasm::elements::Instruction::I64Shl => casper_wasm::elements::Instruction::I64Shl,
            wasm_instrument::parity_wasm::elements::Instruction::I64ShrS => casper_wasm::elements::Instruction::I64ShrS,
            wasm_instrument::parity_wasm::elements::Instruction::I64ShrU => casper_wasm::elements::Instruction::I64ShrU,
            wasm_instrument::parity_wasm::elements::Instruction::I64Rotl => casper_wasm::elements::Instruction::I64Rotl,
            wasm_instrument::parity_wasm::elements::Instruction::I64Rotr => casper_wasm::elements::Instruction::I64Rotr,
            wasm_instrument::parity_wasm::elements::Instruction::F32Abs => casper_wasm::elements::Instruction::F32Abs,
            wasm_instrument::parity_wasm::elements::Instruction::F32Neg => casper_wasm::elements::Instruction::F32Neg,
            wasm_instrument::parity_wasm::elements::Instruction::F32Ceil => casper_wasm::elements::Instruction::F32Ceil,
            wasm_instrument::parity_wasm::elements::Instruction::F32Floor => casper_wasm::elements::Instruction::F32Floor,
            wasm_instrument::parity_wasm::elements::Instruction::F32Trunc => casper_wasm::elements::Instruction::F32Trunc,
            wasm_instrument::parity_wasm::elements::Instruction::F32Nearest => casper_wasm::elements::Instruction::F32Nearest,
            wasm_instrument::parity_wasm::elements::Instruction::F32Sqrt => casper_wasm::elements::Instruction::F32Sqrt,
            wasm_instrument::parity_wasm::elements::Instruction::F32Add => casper_wasm::elements::Instruction::F32Add,
            wasm_instrument::parity_wasm::elements::Instruction::F32Sub => casper_wasm::elements::Instruction::F32Sub,
            wasm_instrument::parity_wasm::elements::Instruction::F32Mul => casper_wasm::elements::Instruction::F32Mul,
            wasm_instrument::parity_wasm::elements::Instruction::F32Div => casper_wasm::elements::Instruction::F32Div,
            wasm_instrument::parity_wasm::elements::Instruction::F32Min => casper_wasm::elements::Instruction::F32Min,
            wasm_instrument::parity_wasm::elements::Instruction::F32Max => casper_wasm::elements::Instruction::F32Max,
            wasm_instrument::parity_wasm::elements::Instruction::F32Copysign => casper_wasm::elements::Instruction::F32Copysign,
            wasm_instrument::parity_wasm::elements::Instruction::F64Abs => casper_wasm::elements::Instruction::F64Abs,
            wasm_instrument::parity_wasm::elements::Instruction::F64Neg => casper_wasm::elements::Instruction::F64Neg,
            wasm_instrument::parity_wasm::elements::Instruction::F64Ceil => casper_wasm::elements::Instruction::F64Ceil,
            wasm_instrument::parity_wasm::elements::Instruction::F64Floor => casper_wasm::elements::Instruction::F64Floor,
            wasm_instrument::parity_wasm::elements::Instruction::F64Trunc => casper_wasm::elements::Instruction::F64Trunc,
            wasm_instrument::parity_wasm::elements::Instruction::F64Nearest => casper_wasm::elements::Instruction::F64Nearest,
            wasm_instrument::parity_wasm::elements::Instruction::F64Sqrt => casper_wasm::elements::Instruction::F64Sqrt,
            wasm_instrument::parity_wasm::elements::Instruction::F64Add => casper_wasm::elements::Instruction::F64Add,
            wasm_instrument::parity_wasm::elements::Instruction::F64Sub => casper_wasm::elements::Instruction::F64Sub,
            wasm_instrument::parity_wasm::elements::Instruction::F64Mul => casper_wasm::elements::Instruction::F64Mul,
            wasm_instrument::parity_wasm::elements::Instruction::F64Div => casper_wasm::elements::Instruction::F64Div,
            wasm_instrument::parity_wasm::elements::Instruction::F64Min => casper_wasm::elements::Instruction::F64Min,
            wasm_instrument::parity_wasm::elements::Instruction::F64Max => casper_wasm::elements::Instruction::F64Max,
            wasm_instrument::parity_wasm::elements::Instruction::F64Copysign => casper_wasm::elements::Instruction::F64Copysign,
            wasm_instrument::parity_wasm::elements::Instruction::I32WrapI64 => casper_wasm::elements::Instruction::I32WrapI64,
            wasm_instrument::parity_wasm::elements::Instruction::I32TruncSF32 => casper_wasm::elements::Instruction::I32TruncSF32,
            wasm_instrument::parity_wasm::elements::Instruction::I32TruncUF32 => casper_wasm::elements::Instruction::I32TruncUF32,
            wasm_instrument::parity_wasm::elements::Instruction::I32TruncSF64 => casper_wasm::elements::Instruction::I32TruncSF64,
            wasm_instrument::parity_wasm::elements::Instruction::I32TruncUF64 => casper_wasm::elements::Instruction::I32TruncUF64,
            wasm_instrument::parity_wasm::elements::Instruction::I64ExtendSI32 => casper_wasm::elements::Instruction::I64ExtendSI32,
            wasm_instrument::parity_wasm::elements::Instruction::I64ExtendUI32 => casper_wasm::elements::Instruction::I64ExtendUI32,
            wasm_instrument::parity_wasm::elements::Instruction::I64TruncSF32 => casper_wasm::elements::Instruction::I64TruncSF32,
            wasm_instrument::parity_wasm::elements::Instruction::I64TruncUF32 => casper_wasm::elements::Instruction::I64TruncUF32,
            wasm_instrument::parity_wasm::elements::Instruction::I64TruncSF64 => casper_wasm::elements::Instruction::I64TruncSF64,
            wasm_instrument::parity_wasm::elements::Instruction::I64TruncUF64 => casper_wasm::elements::Instruction::I64TruncUF64,
            wasm_instrument::parity_wasm::elements::Instruction::F32ConvertSI32 => casper_wasm::elements::Instruction::F32ConvertSI32,
            wasm_instrument::parity_wasm::elements::Instruction::F32ConvertUI32 => casper_wasm::elements::Instruction::F32ConvertUI32,
            wasm_instrument::parity_wasm::elements::Instruction::F32ConvertSI64 => casper_wasm::elements::Instruction::F32ConvertSI64,
            wasm_instrument::parity_wasm::elements::Instruction::F32ConvertUI64 => casper_wasm::elements::Instruction::F32ConvertUI64,
            wasm_instrument::parity_wasm::elements::Instruction::F32DemoteF64 => casper_wasm::elements::Instruction::F32DemoteF64,
            wasm_instrument::parity_wasm::elements::Instruction::F64ConvertSI32 => casper_wasm::elements::Instruction::F64ConvertSI32,
            wasm_instrument::parity_wasm::elements::Instruction::F64ConvertUI32 => casper_wasm::elements::Instruction::F64ConvertUI32,
            wasm_instrument::parity_wasm::elements::Instruction::F64ConvertSI64 => casper_wasm::elements::Instruction::F64ConvertSI64,
            wasm_instrument::parity_wasm::elements::Instruction::F64ConvertUI64 => casper_wasm::elements::Instruction::F64ConvertUI64,
            wasm_instrument::parity_wasm::elements::Instruction::F64PromoteF32 => casper_wasm::elements::Instruction::F64PromoteF32,
            wasm_instrument::parity_wasm::elements::Instruction::I32ReinterpretF32 => casper_wasm::elements::Instruction::I32ReinterpretF32,
            wasm_instrument::parity_wasm::elements::Instruction::I64ReinterpretF64 => casper_wasm::elements::Instruction::I64ReinterpretF64,
            wasm_instrument::parity_wasm::elements::Instruction::F32ReinterpretI32 => casper_wasm::elements::Instruction::F32ReinterpretI32,
            wasm_instrument::parity_wasm::elements::Instruction::F64ReinterpretI64 => casper_wasm::elements::Instruction::F64ReinterpretI64,
        };
        use casper_wasm_utils::rules::Rules as _;
        self.0.instruction_cost(&casper_instruction)
    }

    fn memory_grow_cost(&self) -> gas_metering::MemoryGrowCost {
        use casper_wasm_utils::rules::Rules as _;
        match self.0.memory_grow_cost() {
            Some(casper_wasm_utils::rules::MemoryGrowCost::Linear(memory_units)) => gas_metering::MemoryGrowCost::Linear(
                memory_units
            ),
            None => gas_metering::MemoryGrowCost::Free,
        }
    }

    fn call_per_local_cost(&self) -> u32 {

        // self.0.call_per_local_cost()
        0
    }
}

fn prepare_instance(
    module_bytes: &[u8],
    chainspec: &ChainspecConfig,
    use_wasm_instrument: bool,
    export_name: &str,
    passed_args: &[String],
) -> (casper_wasmi::ModuleRef, Vec<RuntimeValue>) {
    let preprocess_config = PreprocessConfigBuilder::default()
        .with_validate_imports(true)
        .with_stack_height_limiter(false)
        .with_gas_counter(!use_wasm_instrument)
        .build();
    let mut wasm_module =
        runtime::preprocess(chainspec.wasm_config, &module_bytes, preprocess_config).unwrap();


        // let opcode_costs = WasmConfig::new(DEFAULT_WASM_MAX_MEMORY, chainspec.wasm_config.opcode_costs());
    if use_wasm_instrument {
        wasm_module = {
        let serialized_casper_module =
            casper_wasm::serialize(wasm_module.clone()).expect("serialized");
        let parity_wasm_module = wasm_instrument::parity_wasm::deserialize_buffer(&serialized_casper_module).unwrap();

        let backend = gas_metering::mutable_global::Injector::new("gas");
        let parity_rules = ParityRules(RuledOpcodeCosts::from(chainspec.wasm_config.opcode_costs()));
        let backend = gas_metering::inject(parity_wasm_module, backend, &parity_rules).expect("gas injected");

        let serialized_parity_module = wasm_instrument::parity_wasm::serialize(backend).unwrap();
        // let parity_wasm_module = gas_metering::inject(parity_wasm_module, &backend, &parity_rules).expect("gas injected");

        let wasm_module: casper_wasm::elements::Module = casper_wasm::deserialize_buffer(&serialized_parity_module)
            .expect("deserialized");
        wasm_module
        };
    }


    // Decode args based on export signature

    let mut arguments: Option<Vec<RuntimeValue>> = None;

    {
        let serialized_parity_module =
            casper_wasm::serialize(wasm_module.clone()).expect("serialized");

        let walrus_module = walrus::ModuleConfig::default()
            .parse(&serialized_parity_module)
            .expect("valid wasm");

        'outer: for export in walrus_module.exports.iter() {
            if export.name == export_name {
                match export.item {
                    walrus::ExportItem::Function(func_id) => {
                        let func = walrus_module.funcs.get(func_id);
                        // func.ty().
                        // let func_ty = func.ty();

                        let func_params = walrus_module.types.params(func.ty());
                        // dbg!(func_ty);
                        if func_params.len() != passed_args.len() {
                            panic!(
                                "invalid number of arguments expected {:?} got {:?}",
                                func_params, passed_args
                            );
                        }

                        let mut type_safe_args = Vec::with_capacity(func_params.len());

                        for (func_param, string_arg) in func_params.iter().zip(passed_args) {
                            let runtime_value = match func_param {
                                walrus::ValType::I32 => {
                                    let value = string_arg.parse::<i32>().expect("i32");
                                    RuntimeValue::I32(value)
                                }
                                walrus::ValType::I64 => {
                                    let value = string_arg.parse::<i64>().expect("i32");
                                    RuntimeValue::I64(value)
                                }
                                walrus::ValType::F32 => {
                                    let value = string_arg.parse::<f32>().expect("i32");
                                    RuntimeValue::F32(value.into())
                                }
                                walrus::ValType::F64 => {
                                    let value = string_arg.parse::<f64>().expect("i32");
                                    RuntimeValue::F64(value.into())
                                }
                                walrus::ValType::V128 => todo!(),
                                walrus::ValType::Externref => todo!(),
                                walrus::ValType::Funcref => todo!(),
                            };

                            type_safe_args.push(runtime_value);
                        }

                        arguments = Some(type_safe_args);
                        break 'outer;
                    }
                    other => panic!("expected function got {:?}", other),
                }
            }
        }
    }

    // walrus::
    //     'outer: for export in parity_module
    //         .export_section()
    //         .iter()
    //         .flat_map(|export| export.entries())
    //     {
    //         if export.field() == export_name {
    //             match export.internal() {
    //                 Internal::Function(func_idx) => {
    //                     for (func_entry_idx, func) in parity_module
    //                         .function_section()
    //                         .iter()
    //                         .flat_map(|function| function.entries())
    //                         .enumerate()
    //                     {
    //                         dbg!(func_entry_idx, func_idx, func);
    //                         if func_entry_idx == *func_idx as usize {
    //                             let func_type = func.type_ref();
    //                             let type_section = parity_module.type_section().expect("types");

    //                             let types = type_section.types();
    //                             let the_type = types.get(func_type as usize).expect("type to
    // exist");

    //                             match the_type {
    //                                 Type::Function(function_type) => {
    //                                     let mut type_safe_args = Vec::new();

    //                                     if function_type.params().len() != passed_args.len() {
    //                                         panic!("invalid number of arguments (exported
    // function has {} args but received {}", function_type.params().len(), passed_args.len());
    //                                     }

    //                                     for (value_type, string_arg) in
    //                                         function_type.params().iter().zip(passed_args)
    //                                     {
    //                                         let runtime_value = match value_type {
    //                                             ValueType::I32 => {
    //                                                 let value =
    // string_arg.parse::<i32>().expect("i32");
    // RuntimeValue::I32(value)                                             }
    //                                             ValueType::I64 => {
    //                                                 let value =
    // string_arg.parse::<i64>().expect("i64");
    // RuntimeValue::I64(value)                                             }
    //                                             ValueType::F32 => {
    //                                                 let value =
    // string_arg.parse::<f32>().expect("f32");
    // RuntimeValue::F32(value.into())                                             }
    //                                             ValueType::F64 => {
    //                                                 let value =
    // string_arg.parse::<f64>().expect("f64");
    // RuntimeValue::F64(value.into())                                             }
    //                                         };

    //                                         type_safe_args.push(runtime_value);
    //                                     }

    //                                     arguments = Some(type_safe_args);

    //                                     break 'outer;
    //                                 }
    //                             }

    //                             // dbg!(&func_idx, func_entry_idx, type1);
    //                         }
    //                     }
    //                 }

    //                 internal => panic!("expected function got {:?}", internal),
    //             }
    //         }
    //     }

    let type_safe_args = arguments.expect("expected the exported function to be found");

    // let (instance, memory) = utils::instance_and_memory(module.clone(), protocol_version,
    // wasm_config)?;
    let module = casper_wasmi::Module::from_parity_wasm_module(wasm_module).unwrap();

    // module.module()

    // let resolver = resolvers::create_module_resolver(protocol_version, wasm_config)?;
    let resolver = MinimalWasmiResolver::default();
    let mut imports = ImportsBuilder::new();
    imports.push_resolver("env", &resolver);
    let not_started_module = ModuleInstance::new(&module, &imports).unwrap();

    assert!(!not_started_module.has_start());

    let instance = not_started_module.not_started_instance();
    (instance.clone(), type_safe_args)
}

struct RunWasmInfo {
    elapsed: Duration,
    gas_used: u64,
}

fn run_wasm(
    module_bytes: Vec<u8>,
    chainspec: &ChainspecConfig,
    use_wasm_instrument: bool,
    func_name: &str,
    args: &[String],
) -> (
    Result<Option<RuntimeValue>, casper_wasmi::Error>,
    RunWasmInfo,
) {
    println!("Invoke export {:?} with args {:?}", func_name, args);

    let (instance, args) = prepare_instance(&module_bytes, &chainspec,use_wasm_instrument, func_name, args);

    if use_wasm_instrument {
        let globals = instance.globals();
        let g = globals.get(0).unwrap();

        g.set(RuntimeValue::I64(chainspec.transaction_config.block_gas_limit as i64)).unwrap();
    }

    let start = Instant::now();



    let mut externals = MinimalWasmiExternals::new(0, chainspec.transaction_config.block_gas_limit);
    let result: Result<Option<RuntimeValue>, casper_wasmi::Error> =
        instance
            .clone()
            .invoke_export(func_name, &args, &mut externals);

  dbg!(instance.globals());

    // let result = result.expect("valid output");
    let info = RunWasmInfo {
        elapsed: start.elapsed(),
        gas_used: externals.gas_used,
    };

    (result, info)
}
use clap::Parser;
use serde::Deserialize;
use wasm_instrument::gas_metering;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[clap(value_name = "MODULE")]
    wasm_file: PathBuf,
    #[clap(long = "invoke", value_name = "FUNCTION")]
    invoke: Option<String>,
    /// Arguments given to the Wasm module or the invoked function.
    #[clap(value_name = "ARGS")]
    args: Vec<String>,
    #[clap(short, long)]
    chainspec_file: Option<PathBuf>,
    #[clap(short, long)]
    use_wasm_instrument: Option<bool>,
}

fn load_wasm_file<P: AsRef<Path>>(path: P) -> Vec<u8> {
    let path = path.as_ref();
    let bytes = fs::read(path).expect("valid file");
    match path.extension() {
        Some(ext) if ext.to_ascii_lowercase() == "wat" => {
            wat::parse_bytes(&bytes).expect("valid wat").into_owned()
        }
        None | Some(_) => bytes,
    }
}

#[derive(Deserialize, Clone, Default, Debug)]
struct TransactionConfig {
    block_gas_limit: u64,
}

/// in the chainspec file, it can continue to be parsed as an `ChainspecConfig`.
#[derive(Deserialize, Clone, Default, Debug)]
struct ChainspecConfig {
    /// WasmConfig.
    #[serde(rename = "wasm")]
    pub wasm_config: WasmConfig,
    #[serde(rename = "transactions")]
    pub transaction_config: TransactionConfig,
}

fn main() {
    let args = Args::parse();

    let chainspec_file = args.chainspec_file.expect("chainspec file");
    println!("Using chainspec file {:?}", chainspec_file.display());
    let chainspec_data = fs::read_to_string(chainspec_file.as_path()).expect("valid file");
    let chainspec_config: ChainspecConfig =
        toml::from_str(&chainspec_data).expect("valid chainspec");

    let wasm_bytes = load_wasm_file(args.wasm_file);

    if let Some(func_name) = args.invoke {
        let (result, info) = run_wasm(wasm_bytes, &chainspec_config, args.use_wasm_instrument.unwrap_or(false), &func_name, &args.args);

        println!("result: {:?}", result);
        println!("elapsed: {:?}", info.elapsed);
        println!("gas used: {}", info.gas_used);
    }
}

#[derive(Default)]
struct MinimalWasmiResolver;

#[derive(Debug)]
struct MinimalWasmiExternals {
    gas_used: u64,
    block_gas_limit: u64,
}

impl MinimalWasmiExternals {
    fn new(gas_used: u64, block_gas_limit: u64) -> Self {
        Self {
            gas_used,
            block_gas_limit,
        }
    }
}

const GAS_FUNC_IDX: usize = 0;

impl ModuleImportResolver for MinimalWasmiResolver {
    fn resolve_func(
        &self,
        field_name: &str,
        _signature: &casper_wasmi::Signature,
    ) -> Result<casper_wasmi::FuncRef, casper_wasmi::Error> {
        if field_name == "gas" {
            Ok(FuncInstance::alloc_host(
                Signature::new(&[casper_wasmi::ValueType::I32; 1][..], None),
                GAS_FUNC_IDX,
            ))
        } else {
            Err(casper_wasmi::Error::Instantiation(format!(
                "Export {} not found",
                field_name
            )))
        }
    }

    fn resolve_memory(
        &self,
        field_name: &str,
        memory_type: &casper_wasmi::MemoryDescriptor,
    ) -> Result<casper_wasmi::MemoryRef, casper_wasmi::Error> {
        if field_name == "memory" {
            Ok(MemoryInstance::alloc(
                Pages(memory_type.initial() as usize),
                memory_type.maximum().map(|x| Pages(x as usize)),
            )?)
        } else {
            panic!("invalid exported memory name {}", field_name);
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("gas limit")]
struct GasLimit;

impl HostError for GasLimit {}

impl Externals for MinimalWasmiExternals {
    fn invoke_index(
        &mut self,
        index: usize,
        args: casper_wasmi::RuntimeArgs,
    ) -> Result<Option<casper_wasmi::RuntimeValue>, casper_wasmi::Trap> {
        if index == GAS_FUNC_IDX {
            let gas_used: u32 = args.nth_checked(0)?;
            // match gas_used.checked_add(
            match self.gas_used.checked_add(gas_used.into()) {
                Some(new_gas_used) if new_gas_used > self.block_gas_limit => {
                    return Err(GasLimit.into());
                }
                Some(new_gas_used) => {
                    // dbg!(&new_gas_used, &self.block_gas_limit);
                    self.gas_used = new_gas_used;
                }
                None => {
                    unreachable!();
                }
            }
            Ok(None)
        } else {
            unreachable!();
        }
    }
}
