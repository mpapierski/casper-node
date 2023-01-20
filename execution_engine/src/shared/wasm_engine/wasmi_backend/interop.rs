//! Various useful traits to convert rust types into wasmi`s specific value or runtime types.
//!
//! This way host functions generated using [`for_each_host_function`] have minimal boilerplate, and
//! relies on generic types to perform conversions before delegating to [`WasmHostInterface`].
use impl_trait_for_tuples::impl_for_tuples;
use wasmi::{RuntimeValue, ValueType};

pub(crate) trait ToWasmiValueType {
    const VALUE_TYPE: ValueType;
}

impl ToWasmiValueType for u32 {
    const VALUE_TYPE: ValueType = ValueType::I32;
}

impl ToWasmiValueType for i32 {
    const VALUE_TYPE: ValueType = ValueType::I32;
}

impl ToWasmiValueType for u64 {
    const VALUE_TYPE: ValueType = ValueType::I64;
}

impl ToWasmiValueType for i64 {
    const VALUE_TYPE: ValueType = ValueType::I64;
}

pub(crate) trait ToWasmiOptionalValueType {
    const OPTIONAL_VALUE_TYPE: Option<ValueType>;
}

impl ToWasmiOptionalValueType for () {
    const OPTIONAL_VALUE_TYPE: Option<ValueType> = None;
}

impl<T: ToWasmiValueType> ToWasmiOptionalValueType for T {
    const OPTIONAL_VALUE_TYPE: Option<ValueType> = Some(T::VALUE_TYPE);
}

pub(crate) trait ToWasmiValueTypes {
    const VALUE_TYPES: &'static [ValueType];
}

#[impl_for_tuples(1, 10)]
#[tuple_types_custom_trait_bound(ToWasmiValueType)]
impl ToWasmiValueTypes for Tuple {
    for_tuples!( const VALUE_TYPES: &'static [ValueType] = &[ #( Tuple::VALUE_TYPE ),* ]; );
}

pub(crate) trait ToRuntimeValue {
    fn to_runtime_value(&self) -> Option<RuntimeValue>;
}

impl ToRuntimeValue for () {
    fn to_runtime_value(&self) -> Option<RuntimeValue> {
        None
    }
}

impl ToRuntimeValue for u32 {
    fn to_runtime_value(&self) -> Option<RuntimeValue> {
        Some(RuntimeValue::I32(*self as i32))
    }
}

impl ToRuntimeValue for i32 {
    fn to_runtime_value(&self) -> Option<RuntimeValue> {
        Some(RuntimeValue::I32(*self))
    }
}

impl ToRuntimeValue for u64 {
    fn to_runtime_value(&self) -> Option<RuntimeValue> {
        Some(RuntimeValue::I64(*self as i64))
    }
}

impl ToRuntimeValue for i64 {
    fn to_runtime_value(&self) -> Option<RuntimeValue> {
        Some(RuntimeValue::I64(*self))
    }
}
