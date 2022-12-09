//! Support for Wasm opcode costs.
use std::num::NonZeroU32;

use datasize::DataSize;
use rand::{distributions::Standard, prelude::*, Rng};
use serde::{Deserialize, Serialize};
use wasm_instrument::{
    gas_metering::{self, MemoryGrowCost, Rules},
    parity_wasm::elements::Instruction,
};

use casper_types::bytesrepr::{self, FromBytes, ToBytes, U32_SERIALIZED_LENGTH};

/// Default cost of the `bit` Wasm opcode.
pub const DEFAULT_BIT_COST: u32 = 300;
/// Default cost of the `add` Wasm opcode.
pub const DEFAULT_ADD_COST: u32 = 210;
/// Default cost of the `mul` Wasm opcode.
pub const DEFAULT_MUL_COST: u32 = 240;
/// Default cost of the `div` Wasm opcode.
pub const DEFAULT_DIV_COST: u32 = 320;
/// Default cost of the `load` Wasm opcode.
pub const DEFAULT_LOAD_COST: u32 = 2_500;
/// Default cost of the `store` Wasm opcode.
pub const DEFAULT_STORE_COST: u32 = 4_700;
/// Default cost of the `const` Wasm opcode.
pub const DEFAULT_CONST_COST: u32 = 110;
/// Default cost of the `local` Wasm opcode.
pub const DEFAULT_LOCAL_COST: u32 = 390;
/// Default cost of the `global` Wasm opcode.
pub const DEFAULT_GLOBAL_COST: u32 = 390;
/// Default cost of the `control_flow` Wasm opcode.
pub const DEFAULT_CONTROL_FLOW_COST: u32 = 440;
/// Default cost of the `integer_comparison` Wasm opcode.
pub const DEFAULT_INTEGER_COMPARISON_COST: u32 = 250;
/// Default cost of the `conversion` Wasm opcode.
pub const DEFAULT_CONVERSION_COST: u32 = 420;
/// Default cost of the `unreachable` Wasm opcode.
pub const DEFAULT_UNREACHABLE_COST: u32 = 270;
/// Default cost of the `nop` Wasm opcode.
// TODO: This value is not researched.
pub const DEFAULT_NOP_COST: u32 = 200;
/// Default cost of the `current_memory` Wasm opcode.
pub const DEFAULT_CURRENT_MEMORY_COST: u32 = 290;
/// Default cost of the `grow_memory` Wasm opcode.
pub const DEFAULT_GROW_MEMORY_COST: u32 = 240_000;
/// Default cost of the `regular` Wasm opcode.
pub const DEFAULT_REGULAR_COST: u32 = 210;

const NUM_FIELDS: usize = 17;
const OPCODE_COSTS_SERIALIZED_LENGTH: usize = NUM_FIELDS * U32_SERIALIZED_LENGTH;

/// Definition of a cost table for Wasm opcodes.
///
/// This is taken (partially) from parity-ethereum.
#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Debug, DataSize)]
pub struct OpcodeCosts {
    /// Bit operations multiplier.
    pub bit: u32,
    /// Arithmetic add operations multiplier.
    pub add: u32,
    /// Mul operations multiplier.
    pub mul: u32,
    /// Div operations multiplier.
    pub div: u32,
    /// Memory load operation multiplier.
    pub load: u32,
    /// Memory store operation multiplier.
    pub store: u32,
    /// Const operation multiplier.
    #[serde(rename = "const")]
    pub op_const: u32,
    /// Local operations multiplier.
    pub local: u32,
    /// Global operations multiplier.
    pub global: u32,
    /// Control flow operations multiplier.
    pub control_flow: u32,
    /// Integer operations multiplier.
    pub integer_comparison: u32,
    /// Conversion operations multiplier.
    pub conversion: u32,
    /// Unreachable operation multiplier.
    pub unreachable: u32,
    /// Nop operation multiplier.
    pub nop: u32,
    /// Get current memory operation multiplier.
    pub current_memory: u32,
    /// Grow memory cost, per page (64kb)
    pub grow_memory: u32,
    /// Regular opcode cost
    pub regular: u32,
}

// struct GasMeteringRules;

impl Rules for OpcodeCosts {
    fn instruction_cost(&self, instruction: &Instruction) -> Option<u32> {
        // Based on https://github.com/paritytech/wasm-utils/blob/c20633c4b4b8dc7c6101bb25a435ed6f05d3c4a5/src/rules.rs#L111-L303

        use Instruction::*;

        match instruction {
            Unreachable => Some(self.unreachable),
            Nop => Some(self.nop),
            Block(_) => Some(self.control_flow),
            Loop(_) => Some(self.control_flow),
            If(_) => Some(self.control_flow),
            Else => Some(self.control_flow),
            End => Some(self.control_flow),
            Br(_) => Some(self.control_flow),
            BrIf(_) => Some(self.control_flow),
            BrTable(_) => Some(self.control_flow),
            Return => Some(self.control_flow),
            Call(_) => Some(self.control_flow),
            CallIndirect(_, _) => Some(self.control_flow),
            Drop => Some(self.control_flow),
            Select => Some(self.control_flow),

            GetLocal(_) => Some(self.local),
            SetLocal(_) => Some(self.local),
            TeeLocal(_) => Some(self.local),
            GetGlobal(_) => Some(self.global),
            SetGlobal(_) => Some(self.global),

            I32Load(_, _) => Some(self.load),
            I64Load(_, _) => Some(self.load),
            F32Load(_, _) => Some(self.load),
            F64Load(_, _) => Some(self.load),
            I32Load8S(_, _) => Some(self.load),
            I32Load8U(_, _) => Some(self.load),
            I32Load16S(_, _) => Some(self.load),
            I32Load16U(_, _) => Some(self.load),
            I64Load8S(_, _) => Some(self.load),
            I64Load8U(_, _) => Some(self.load),
            I64Load16S(_, _) => Some(self.load),
            I64Load16U(_, _) => Some(self.load),
            I64Load32S(_, _) => Some(self.load),
            I64Load32U(_, _) => Some(self.load),

            I32Store(_, _) => Some(self.store),
            I64Store(_, _) => Some(self.store),
            F32Store(_, _) => Some(self.store),
            F64Store(_, _) => Some(self.store),
            I32Store8(_, _) => Some(self.store),
            I32Store16(_, _) => Some(self.store),
            I64Store8(_, _) => Some(self.store),
            I64Store16(_, _) => Some(self.store),
            I64Store32(_, _) => Some(self.store),

            CurrentMemory(_) => Some(self.current_memory),
            GrowMemory(_) => Some(self.grow_memory),

            I32Const(_) => Some(self.op_const),
            I64Const(_) => Some(self.op_const),

            F32Const(_) => None, // InstructionType::FloatConst
            F64Const(_) => None, // InstructionType::FloatConst

            I32Eqz => Some(self.integer_comparison),
            I32Eq => Some(self.integer_comparison),
            I32Ne => Some(self.integer_comparison),
            I32LtS => Some(self.integer_comparison),
            I32LtU => Some(self.integer_comparison),
            I32GtS => Some(self.integer_comparison),
            I32GtU => Some(self.integer_comparison),
            I32LeS => Some(self.integer_comparison),
            I32LeU => Some(self.integer_comparison),
            I32GeS => Some(self.integer_comparison),
            I32GeU => Some(self.integer_comparison),

            I64Eqz => Some(self.integer_comparison),
            I64Eq => Some(self.integer_comparison),
            I64Ne => Some(self.integer_comparison),
            I64LtS => Some(self.integer_comparison),
            I64LtU => Some(self.integer_comparison),
            I64GtS => Some(self.integer_comparison),
            I64GtU => Some(self.integer_comparison),
            I64LeS => Some(self.integer_comparison),
            I64LeU => Some(self.integer_comparison),
            I64GeS => Some(self.integer_comparison),
            I64GeU => Some(self.integer_comparison),

            F32Eq => None, // InstructionType::FloatComparison
            F32Ne => None, // InstructionType::FloatComparison
            F32Lt => None, // InstructionType::FloatComparison
            F32Gt => None, // InstructionType::FloatComparison
            F32Le => None, // InstructionType::FloatComparison
            F32Ge => None, // InstructionType::FloatComparison

            F64Eq => None, // InstructionType::FloatComparison
            F64Ne => None, // InstructionType::FloatComparison
            F64Lt => None, // InstructionType::FloatComparison
            F64Gt => None, // InstructionType::FloatComparison
            F64Le => None, // InstructionType::FloatComparison
            F64Ge => None, // InstructionType::FloatComparison

            I32Clz => Some(self.bit),
            I32Ctz => Some(self.bit),
            I32Popcnt => Some(self.bit),
            I32Add => Some(self.add),
            I32Sub => Some(self.add),
            I32Mul => Some(self.mul),
            I32DivS => Some(self.div),
            I32DivU => Some(self.div),
            I32RemS => Some(self.div),
            I32RemU => Some(self.div),
            I32And => Some(self.bit),
            I32Or => Some(self.bit),
            I32Xor => Some(self.bit),
            I32Shl => Some(self.bit),
            I32ShrS => Some(self.bit),
            I32ShrU => Some(self.bit),
            I32Rotl => Some(self.bit),
            I32Rotr => Some(self.bit),

            I64Clz => Some(self.bit),
            I64Ctz => Some(self.bit),
            I64Popcnt => Some(self.bit),
            I64Add => Some(self.add),
            I64Sub => Some(self.add),
            I64Mul => Some(self.mul),
            I64DivS => Some(self.div),
            I64DivU => Some(self.div),
            I64RemS => Some(self.div),
            I64RemU => Some(self.div),
            I64And => Some(self.bit),
            I64Or => Some(self.bit),
            I64Xor => Some(self.bit),
            I64Shl => Some(self.bit),
            I64ShrS => Some(self.bit),
            I64ShrU => Some(self.bit),
            I64Rotl => Some(self.bit),
            I64Rotr => Some(self.bit),

            F32Abs => None,      // InstructionType::Float
            F32Neg => None,      // InstructionType::Float
            F32Ceil => None,     // InstructionType::Float
            F32Floor => None,    // InstructionType::Float
            F32Trunc => None,    // InstructionType::Float
            F32Nearest => None,  // InstructionType::Float
            F32Sqrt => None,     // InstructionType::Float
            F32Add => None,      // InstructionType::Float
            F32Sub => None,      // InstructionType::Float
            F32Mul => None,      // InstructionType::Float
            F32Div => None,      // InstructionType::Float
            F32Min => None,      // InstructionType::Float
            F32Max => None,      // InstructionType::Float
            F32Copysign => None, // InstructionType::Float
            F64Abs => None,      // InstructionType::Float
            F64Neg => None,      // InstructionType::Float
            F64Ceil => None,     // InstructionType::Float
            F64Floor => None,    // InstructionType::Float
            F64Trunc => None,    // InstructionType::Float
            F64Nearest => None,  // InstructionType::Float
            F64Sqrt => None,     // InstructionType::Float
            F64Add => None,      // InstructionType::Float
            F64Sub => None,      // InstructionType::Float
            F64Mul => None,      // InstructionType::Float
            F64Div => None,      // InstructionType::Float
            F64Min => None,      // InstructionType::Float
            F64Max => None,      // InstructionType::Float
            F64Copysign => None, // InstructionType::Float

            I32WrapI64 => Some(self.conversion),
            I64ExtendSI32 => Some(self.conversion),
            I64ExtendUI32 => Some(self.conversion),

            I32TruncSF32 => None,   // InstructionType::FloatConversion,
            I32TruncUF32 => None,   // InstructionType::FloatConversion,
            I32TruncSF64 => None,   // InstructionType::FloatConversion,
            I32TruncUF64 => None,   // InstructionType::FloatConversion,
            I64TruncSF32 => None,   // InstructionType::FloatConversion,
            I64TruncUF32 => None,   // InstructionType::FloatConversion,
            I64TruncSF64 => None,   // InstructionType::FloatConversion,
            I64TruncUF64 => None,   // InstructionType::FloatConversion,
            F32ConvertSI32 => None, // InstructionType::FloatConversion,
            F32ConvertUI32 => None, // InstructionType::FloatConversion,
            F32ConvertSI64 => None, // InstructionType::FloatConversion,
            F32ConvertUI64 => None, // InstructionType::FloatConversion,
            F32DemoteF64 => None,   // InstructionType::FloatConversion,
            F64ConvertSI32 => None, // InstructionType::FloatConversion,
            F64ConvertUI32 => None, // InstructionType::FloatConversion,
            F64ConvertSI64 => None, // InstructionType::FloatConversion,
            F64ConvertUI64 => None, // InstructionType::FloatConversion,
            F64PromoteF32 => None,  // InstructionType::FloatConversion,

            I32ReinterpretF32 => None, // InstructionType::Reinterpretation
            I64ReinterpretF64 => None, // InstructionType::Reinterpretation
            F32ReinterpretI32 => None, // InstructionType::Reinterpretation
            F64ReinterpretI64 => None, // InstructionType::Reinterpretation
        }
    }

    fn memory_grow_cost(&self) -> MemoryGrowCost {
        NonZeroU32::new(self.grow_memory).map_or(MemoryGrowCost::Free, MemoryGrowCost::Linear)
    }

    fn call_per_local_cost(&self) -> u32 {
        // NOTE: We currently don't charge per amount of locals defined in each function block.l
        0
    }
}

impl Default for OpcodeCosts {
    fn default() -> Self {
        OpcodeCosts {
            bit: DEFAULT_BIT_COST,
            add: DEFAULT_ADD_COST,
            mul: DEFAULT_MUL_COST,
            div: DEFAULT_DIV_COST,
            load: DEFAULT_LOAD_COST,
            store: DEFAULT_STORE_COST,
            op_const: DEFAULT_CONST_COST,
            local: DEFAULT_LOCAL_COST,
            global: DEFAULT_GLOBAL_COST,
            control_flow: DEFAULT_CONTROL_FLOW_COST,
            integer_comparison: DEFAULT_INTEGER_COMPARISON_COST,
            conversion: DEFAULT_CONVERSION_COST,
            unreachable: DEFAULT_UNREACHABLE_COST,
            nop: DEFAULT_NOP_COST,
            current_memory: DEFAULT_CURRENT_MEMORY_COST,
            grow_memory: DEFAULT_GROW_MEMORY_COST,
            regular: DEFAULT_REGULAR_COST,
        }
    }
}

impl Distribution<OpcodeCosts> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> OpcodeCosts {
        OpcodeCosts {
            bit: rng.gen(),
            add: rng.gen(),
            mul: rng.gen(),
            div: rng.gen(),
            load: rng.gen(),
            store: rng.gen(),
            op_const: rng.gen(),
            local: rng.gen(),
            global: rng.gen(),
            control_flow: rng.gen(),
            integer_comparison: rng.gen(),
            conversion: rng.gen(),
            unreachable: rng.gen(),
            nop: rng.gen(),
            current_memory: rng.gen(),
            grow_memory: rng.gen(),
            regular: rng.gen(),
        }
    }
}

impl ToBytes for OpcodeCosts {
    fn to_bytes(&self) -> Result<Vec<u8>, bytesrepr::Error> {
        let mut ret = bytesrepr::unchecked_allocate_buffer(self);

        ret.append(&mut self.bit.to_bytes()?);
        ret.append(&mut self.add.to_bytes()?);
        ret.append(&mut self.mul.to_bytes()?);
        ret.append(&mut self.div.to_bytes()?);
        ret.append(&mut self.load.to_bytes()?);
        ret.append(&mut self.store.to_bytes()?);
        ret.append(&mut self.op_const.to_bytes()?);
        ret.append(&mut self.local.to_bytes()?);
        ret.append(&mut self.global.to_bytes()?);
        ret.append(&mut self.control_flow.to_bytes()?);
        ret.append(&mut self.integer_comparison.to_bytes()?);
        ret.append(&mut self.conversion.to_bytes()?);
        ret.append(&mut self.unreachable.to_bytes()?);
        ret.append(&mut self.nop.to_bytes()?);
        ret.append(&mut self.current_memory.to_bytes()?);
        ret.append(&mut self.grow_memory.to_bytes()?);
        ret.append(&mut self.regular.to_bytes()?);

        Ok(ret)
    }

    fn serialized_length(&self) -> usize {
        OPCODE_COSTS_SERIALIZED_LENGTH
    }
}

impl FromBytes for OpcodeCosts {
    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), bytesrepr::Error> {
        let (bit, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (add, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (mul, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (div, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (load, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (store, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (const_, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (local, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (global, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (control_flow, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (integer_comparison, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (conversion, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (unreachable, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (nop, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (current_memory, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (grow_memory, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let (regular, bytes): (_, &[u8]) = FromBytes::from_bytes(bytes)?;
        let opcode_costs = OpcodeCosts {
            bit,
            add,
            mul,
            div,
            load,
            store,
            op_const: const_,
            local,
            global,
            control_flow,
            integer_comparison,
            conversion,
            unreachable,
            nop,
            current_memory,
            grow_memory,
            regular,
        };
        Ok((opcode_costs, bytes))
    }
}

#[doc(hidden)]
#[cfg(any(feature = "gens", test))]
pub mod gens {
    use proptest::{num, prop_compose};

    use crate::shared::opcode_costs::OpcodeCosts;

    prop_compose! {
        pub fn opcode_costs_arb()(
            bit in num::u32::ANY,
            add in num::u32::ANY,
            mul in num::u32::ANY,
            div in num::u32::ANY,
            load in num::u32::ANY,
            store in num::u32::ANY,
            op_const in num::u32::ANY,
            local in num::u32::ANY,
            global in num::u32::ANY,
            control_flow in num::u32::ANY,
            integer_comparison in num::u32::ANY,
            conversion in num::u32::ANY,
            unreachable in num::u32::ANY,
            nop in num::u32::ANY,
            current_memory in num::u32::ANY,
            grow_memory in num::u32::ANY,
            regular in num::u32::ANY,
        ) -> OpcodeCosts {
            OpcodeCosts {
                bit,
                add,
                mul,
                div,
                load,
                store,
                op_const,
                local,
                global,
                control_flow,
                integer_comparison,
                conversion,
                unreachable,
                nop,
                current_memory,
                grow_memory,
                regular,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::proptest;

    use casper_types::bytesrepr;

    use super::gens;

    proptest! {
        #[test]
        fn should_serialize_and_deserialize_with_arbitrary_values(
            opcode_costs in gens::opcode_costs_arb()
        ) {
            bytesrepr::test_serialization_roundtrip(&opcode_costs);
        }
    }
}
