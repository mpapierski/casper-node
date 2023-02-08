use std::{cell::RefCell, collections::HashMap};

use once_cell::sync::Lazy;
use thiserror::Error;
use wasmi::{
    memory_units::Pages, Error, Externals, FuncInstance, FuncRef, HostError, MemoryInstance,
    MemoryRef, ModuleImportResolver, RuntimeValue, Signature, Trap, ValueType,
};

use crate::for_each_host_function;

use super::{host_interface::WasmHostInterface, FunctionContext, RuntimeError};

pub(crate) mod interop;
use interop::{ToWasmiOptionalValueType, ToWasmiResult, ToWasmiValueTypes};

#[derive(Error, Debug)]
#[error("{}", source)]
pub(crate) struct IndirectHostError<E: std::error::Error + Send + Sync + 'static> {
    #[source]
    pub(crate) source: E,
}

impl<E: std::error::Error + Send + Sync + 'static> HostError for IndirectHostError<E> {}

pub(crate) fn make_wasmi_host_error<E: std::error::Error + Send + Sync + 'static>(
    error: E,
) -> impl HostError {
    IndirectHostError { source: error }
}

pub struct WasmiAdapter {
    memory: MemoryRef,
}

impl WasmiAdapter {
    pub fn new(memory: MemoryRef) -> Self {
        Self { memory }
    }
}

impl FunctionContext for WasmiAdapter {
    fn memory_read(&self, offset: u32, size: usize) -> Result<Vec<u8>, RuntimeError> {
        Ok(self.memory.get(offset, size)?)
    }

    fn memory_write(&mut self, offset: u32, data: &[u8]) -> Result<(), RuntimeError> {
        self.memory.set(offset, data)?;
        Ok(())
    }
}

macro_rules! visit_host_function_enum {
    ($( $(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        #[allow(non_camel_case_types)]
        #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
        enum HostFunctionIndex {
        $(
            $(#[$cfg])?
            $name,
        )*
        }
    }
}

for_each_host_function!(visit_host_function_enum);

#[derive(Debug)]
struct WasmiHostFunction {
    /// Name of the host function
    name: &'static str,
    /// List of parameters a host function accepts.
    params: &'static [ValueType],
    /// Return type of the host function.
    return_type: Option<ValueType>,
    /// A value used in [`WasmiExternals`] to delegate calls in a match statement.
    index: HostFunctionIndex,
}

macro_rules! visit_host_function {
    (@optional_ret) => { () };
    (@optional_ret $ret:tt) => { $ret };

    ($( $(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {
        &[ $(
            $(#[$cfg])?
            {
                let params = <( $( $($argty,)*) ?)>::VALUE_TYPES;
                let return_type = <visit_host_function!(@optional_ret $($ret)?)>::OPTIONAL_VALUE_TYPE;

                WasmiHostFunction { name: stringify!($name), params, return_type, index: HostFunctionIndex::$name }
            }
        ,)* ]
    }
}

const HOST_FUNCTIONS: &'static [WasmiHostFunction] = for_each_host_function!(visit_host_function);

pub(crate) struct WasmiResolver {
    memory: RefCell<Option<MemoryRef>>,
    max_memory: u32,
}

impl WasmiResolver {
    pub(crate) fn new(max_memory: u32) -> Self {
        Self {
            memory: RefCell::new(None),
            max_memory,
        }
    }

    pub(crate) fn memory(&self) -> Option<MemoryRef> {
        self.memory.borrow().as_ref().map(Clone::clone)
    }
}

/// Lookup table from host function name to a definition to allow O(1) lookups.
static NAME_LOOKUP_TABLE: Lazy<HashMap<&'static str, &'static WasmiHostFunction>> =
    Lazy::new(|| {
        let mut map = HashMap::new();
        for func in HOST_FUNCTIONS.iter() {
            map.insert(func.name, func);
        }
        map
    });

impl ModuleImportResolver for WasmiResolver {
    fn resolve_func(&self, field_name: &str, _signature: &Signature) -> Result<FuncRef, Error> {
        let host_func = NAME_LOOKUP_TABLE
            .get(field_name)
            .ok_or_else(|| Error::Instantiation(format!("Export {} not found", field_name)))?;
        let signature = Signature::new(host_func.params, host_func.return_type);
        Ok(FuncInstance::alloc_host(
            signature,
            host_func.index as usize,
        ))
    }

    fn resolve_memory(
        &self,
        field_name: &str,
        descriptor: &wasmi::MemoryDescriptor,
    ) -> Result<MemoryRef, Error> {
        if field_name == "memory" {
            match &mut *self.memory.borrow_mut() {
                Some(_) => {
                    // Even though most wat -> wasm compilers don't allow multiple memory entries,
                    // we should make sure we won't accidentally allocate twice.
                    return Err(Error::Instantiation(
                        "Memory is already instantiated".into(),
                    ));
                }
                memory_ref @ None => {
                    // Any memory entry in the wasm file without max specified is changed into an
                    // entry with hardcoded max value. This way `maximum` below is never
                    // unspecified, but for safety reasons we'll still default it.
                    let descriptor_max = descriptor.maximum().unwrap_or(self.max_memory);
                    // Checks if wasm's memory entry has too much initial memory or non-default max
                    // memory pages exceeds the limit.
                    if descriptor.initial() > descriptor_max || descriptor_max > self.max_memory {
                        return Err(Error::Instantiation(
                            "Module requested too much memory".into(),
                        ));
                    }
                    // Note: each "page" is 64 KiB
                    let mem = MemoryInstance::alloc(
                        Pages(descriptor.initial() as usize),
                        descriptor.maximum().map(|x| Pages(x as usize)),
                    )?;
                    *memory_ref = Some(mem.clone());
                    Ok(mem)
                }
            }
        } else {
            Err(Error::Instantiation(format!(
                "Export {} not found",
                field_name
            )))
        }
    }

    fn resolve_table(
        &self,
        field_name: &str,
        _table_type: &wasmi::TableDescriptor,
    ) -> Result<wasmi::TableRef, Error> {
        Err(Error::Instantiation(format!(
            "Export {} not found",
            field_name
        )))
    }
}

pub(crate) struct WasmiExternals<'a, H>
where
    H: WasmHostInterface,
{
    pub(crate) host: &'a mut H,
    pub(crate) memory: MemoryRef,
}

impl<'a, H> Externals for WasmiExternals<'a, H>
where
    H: WasmHostInterface,
    H::Error: std::error::Error + Send + Sync + 'static,
{
    #[allow(unused_assignments)]
    fn invoke_index(
        &mut self,
        index: usize,
        args: wasmi::RuntimeArgs,
    ) -> Result<Option<RuntimeValue>, Trap> {
        let wasmi_adapter = WasmiAdapter::new(self.memory.clone());

        macro_rules! visit_host_function {
            (@optional_ret) => { () };
            (@optional_ret $ret:tt) => { $ret };

            ($($(#[$cfg:meta])? fn $name:ident $(( $($arg:ident: $argty:ty),* ))? $(-> $ret:tt)?;)*) => {{
                let host_func = HOST_FUNCTIONS.get(index).unwrap(); // SAFETY: resolver returns an index in the table
                match host_func.index {
                    $(
                    $(#[$cfg])?
                    HostFunctionIndex::$name => {
                        let mut param_idx = 0;
                        let res: visit_host_function!(@optional_ret $($ret)?) = self.host.$name(
                            wasmi_adapter,
                            $($({
                            let $arg: $argty = args.nth_checked(param_idx)?;
                            param_idx += 1;
                            $arg
                        } ),*)?
                        ).map_err(make_wasmi_host_error)?;
                        res.to_wasmi_result()
                    }
                    )+
                }
            }};
        }

        let result = for_each_host_function!(visit_host_function);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_attribute_is_respected_in_macro() {
        if cfg!(feature = "test-support") {
            assert!(NAME_LOOKUP_TABLE.contains_key(&"casper_print"));
        } else {
            assert!(!NAME_LOOKUP_TABLE.contains_key(&"casper_print"));
        }
    }
}
