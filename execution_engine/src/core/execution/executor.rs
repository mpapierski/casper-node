use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use parity_wasm::elements::Module;
use tracing::warn;
use wasmi::ModuleRef;

use casper_types::{
    account::AccountHash, bytesrepr::FromBytes, contracts::NamedKeys, AccessRights, BlockTime,
    CLTyped, CLValue, ContractPackage, EntryPoint, EntryPointType, Key, Phase, ProtocolVersion,
    RuntimeArgs,
};

use crate::{
    core::{
        engine_state::{
            execution_effect::ExecutionEffect, execution_result::ExecutionResult,
            system_contract_cache::SystemContractCache, EngineConfig,
        },
        execution::{address_generator::AddressGenerator, Error},
        runtime::{
            extract_access_rights_from_keys, extract_access_rights_from_urefs, instance_and_memory,
            Runtime,
        },
        runtime_context::{self, RuntimeContext},
        tracking_copy::TrackingCopy,
        Address,
    },
    shared::{account::Account, gas::Gas, newtypes::CorrelationId, stored_value::StoredValue},
    storage::{global_state::StateReader, protocol_data::ProtocolData},
};

macro_rules! on_fail_charge {
    ($fn:expr) => {
        match $fn {
            Ok(res) => res,
            Err(e) => {
                let exec_err: Error = e.into();
                warn!("Execution failed: {:?}", exec_err);
                return ExecutionResult::precondition_failure(exec_err.into());
            }
        }
    };
    ($fn:expr, $cost:expr) => {
        match $fn {
            Ok(res) => res,
            Err(e) => {
                let exec_err: Error = e.into();
                warn!("Execution failed: {:?}", exec_err);
                return ExecutionResult::Failure {
                    error: exec_err.into(),
                    effect: Default::default(),
                    cost: $cost,
                };
            }
        }
    };
    ($fn:expr, $cost:expr, $effect:expr) => {
        match $fn {
            Ok(res) => res,
            Err(e) => {
                let exec_err: Error = e.into();
                warn!("Execution failed: {:?}", exec_err);
                return ExecutionResult::Failure {
                    error: exec_err.into(),
                    effect: $effect,
                    cost: $cost,
                };
            }
        }
    };
}

pub struct Executor {
    config: EngineConfig,
}

#[allow(clippy::too_many_arguments)]
impl Executor {
    pub fn new(config: EngineConfig) -> Self {
        Executor { config }
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    pub fn exec<R>(
        &self,
        module: Module,
        entry_point: EntryPoint,
        args: RuntimeArgs,
        base_key: Key,
        account: &Account,
        named_keys: &mut NamedKeys,
        authorization_keys: BTreeSet<AccountHash>,
        blocktime: BlockTime,
        deploy_hash: [u8; 32],
        gas_limit: Gas,
        protocol_version: ProtocolVersion,
        correlation_id: CorrelationId,
        tracking_copy: Rc<RefCell<TrackingCopy<R>>>,
        phase: Phase,
        protocol_data: ProtocolData,
        system_contract_cache: SystemContractCache,
        contract_package: &ContractPackage,
    ) -> ExecutionResult
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<Error>,
    {
        let entry_point_name = entry_point.name();
        let entry_point_type = entry_point.entry_point_type();
        let entry_point_access = entry_point.access();

        let (instance, memory) =
            on_fail_charge!(instance_and_memory(module.clone(), protocol_version));

        let access_rights = {
            let keys: Vec<Key> = named_keys.values().cloned().collect();
            extract_access_rights_from_keys(keys)
        };

        let hash_address_generator = {
            let generator = AddressGenerator::new(&deploy_hash, phase);
            Rc::new(RefCell::new(generator))
        };
        let uref_address_generator = {
            let generator = AddressGenerator::new(&deploy_hash, phase);
            Rc::new(RefCell::new(generator))
        };
        let gas_counter: Gas = Gas::default();

        // Snapshot of effects before execution, so in case of error
        // only nonce update can be returned.
        let effects_snapshot = tracking_copy.borrow().effect();

        let context = RuntimeContext::new(
            tracking_copy,
            entry_point_type,
            named_keys,
            access_rights,
            args.clone(),
            authorization_keys,
            &account,
            base_key,
            blocktime,
            deploy_hash,
            gas_limit,
            gas_counter,
            hash_address_generator,
            uref_address_generator,
            protocol_version,
            correlation_id,
            phase,
            protocol_data,
        );

        let mut runtime = Runtime::new(self.config, system_contract_cache, memory, module, context);

        let accounts_access_rights = {
            let keys: Vec<Key> = account.named_keys().values().cloned().collect();
            extract_access_rights_from_keys(keys)
        };

        on_fail_charge!(runtime_context::validate_entry_point_access_with(
            &contract_package,
            entry_point_access,
            |uref| runtime_context::uref_has_access_rights(uref, &accounts_access_rights)
        ));

        if !self.config.use_system_contracts() {
            if runtime.is_mint(base_key) {
                match runtime.call_host_mint(
                    protocol_version,
                    entry_point.name(),
                    &mut runtime.context().named_keys().to_owned(),
                    &args,
                    Default::default(),
                ) {
                    Ok(_value) => {
                        return ExecutionResult::Success {
                            effect: runtime.context().effect(),
                            cost: runtime.context().gas_counter(),
                        };
                    }
                    Err(error) => {
                        return ExecutionResult::Failure {
                            error: error.into(),
                            effect: effects_snapshot,
                            cost: runtime.context().gas_counter(),
                        };
                    }
                }
            } else if runtime.is_proof_of_stake(base_key) {
                match runtime.call_host_proof_of_stake(
                    protocol_version,
                    entry_point.name(),
                    &mut runtime.context().named_keys().to_owned(),
                    &args,
                    Default::default(),
                ) {
                    Ok(_value) => {
                        return ExecutionResult::Success {
                            effect: runtime.context().effect(),
                            cost: runtime.context().gas_counter(),
                        };
                    }
                    Err(error) => {
                        return ExecutionResult::Failure {
                            error: error.into(),
                            effect: effects_snapshot,
                            cost: runtime.context().gas_counter(),
                        };
                    }
                }
            } else if runtime.is_auction(base_key) {
                match runtime.call_host_auction(
                    protocol_version,
                    entry_point.name(),
                    &mut runtime.context().named_keys().to_owned(),
                    &args,
                    Default::default(),
                ) {
                    Ok(_value) => {
                        return ExecutionResult::Success {
                            effect: runtime.context().effect(),
                            cost: runtime.context().gas_counter(),
                        }
                    }
                    Err(error) => {
                        return ExecutionResult::Failure {
                            error: error.into(),
                            effect: effects_snapshot,
                            cost: runtime.context().gas_counter(),
                        }
                    }
                }
            }
        }

        on_fail_charge!(
            instance.invoke_export(entry_point_name, &[], &mut runtime),
            runtime.context().gas_counter(),
            effects_snapshot
        );

        ExecutionResult::Success {
            effect: runtime.context().effect(),
            cost: runtime.context().gas_counter(),
        }
    }

    pub fn exec_system_contract<R, T>(
        &self,
        direct_system_contract_call: DirectSystemContractCall,
        module: Module,
        runtime_args: RuntimeArgs,
        named_keys: &mut NamedKeys,
        extra_keys: &[Key],
        base_key: Key,
        account: &Account,
        authorization_keys: BTreeSet<AccountHash>,
        blocktime: BlockTime,
        deploy_hash: [u8; 32],
        gas_limit: Gas,
        protocol_version: ProtocolVersion,
        correlation_id: CorrelationId,
        tracking_copy: Rc<RefCell<TrackingCopy<R>>>,
        phase: Phase,
        protocol_data: ProtocolData,
        system_contract_cache: SystemContractCache,
    ) -> (Option<T>, ExecutionResult)
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<Error>,
        T: FromBytes + CLTyped,
    {
        match direct_system_contract_call {
            DirectSystemContractCall::FinalizePayment => {
                if protocol_data.proof_of_stake() != base_key.into_seed() {
                    panic!(
                        "{} should only be called with the proof of stake contract",
                        direct_system_contract_call.entry_point_name()
                    );
                }
            }
            DirectSystemContractCall::CreatePurse | DirectSystemContractCall::Transfer => {
                if protocol_data.mint() != base_key.into_seed() {
                    panic!(
                        "{} should only be called with the mint contract",
                        direct_system_contract_call.entry_point_name()
                    );
                }
            }
        }

        let hash_address_generator = {
            let generator = AddressGenerator::new(&deploy_hash, phase);
            Rc::new(RefCell::new(generator))
        };
        let uref_address_generator = {
            let generator = AddressGenerator::new(&deploy_hash, phase);
            Rc::new(RefCell::new(generator))
        };
        let gas_counter = Gas::default(); // maybe const?

        // Snapshot of effects before execution, so in case of error only nonce update
        // can be returned.
        let effect_snapshot = tracking_copy.borrow().effect();

        let (instance, mut runtime) = self
            .create_runtime(
                module,
                EntryPointType::Contract,
                runtime_args.clone(),
                named_keys,
                extra_keys,
                base_key,
                account,
                authorization_keys,
                blocktime,
                deploy_hash,
                gas_limit,
                hash_address_generator,
                uref_address_generator,
                protocol_version,
                correlation_id,
                tracking_copy,
                phase,
                protocol_data,
                system_contract_cache,
            )
            .map_err(|e| {
                ExecutionResult::Failure {
                    effect: effect_snapshot.clone(),
                    cost: gas_counter,
                    error: e.into(),
                }
                .take_without_ret::<T>();
            })
            .unwrap();

        if !self.config.use_system_contracts() {
            let mut inner_named_keys = runtime.context().named_keys().clone();
            let ret = direct_system_contract_call.host_exec(
                runtime,
                protocol_version,
                &mut inner_named_keys,
                &runtime_args,
                extra_keys,
                effect_snapshot,
            );
            *named_keys = inner_named_keys;
            return ret;
        }

        let (maybe_ret, maybe_error, revert_effect): (Option<T>, Option<Error>, bool) = {
            match instance.invoke_export(
                direct_system_contract_call.entry_point_name(),
                &[],
                &mut runtime,
            ) {
                Err(error) => match error.as_host_error() {
                    Some(host_error) => match host_error.downcast_ref::<Error>().unwrap() {
                        Error::Ret(ref ret_urefs) => match runtime.take_host_buffer() {
                            Some(result) => match result.into_t() {
                                Ok(ret) => {
                                    let ret_urefs_map: HashMap<Address, HashSet<AccessRights>> =
                                        extract_access_rights_from_urefs(ret_urefs.clone());
                                    runtime.access_rights_extend(ret_urefs_map);

                                    (Some(ret), None, false)
                                }
                                Err(error) => (None, Some(Error::CLValue(error)), false),
                            },
                            None => (None, Some(Error::ExpectedReturnValue), false),
                        },
                        Error::Revert(api_error) => (None, Some(Error::Revert(*api_error)), true),
                        error => (None, Some(error.clone()), true),
                    },
                    None => (None, Some(Error::Interpreter(error.into())), false),
                },
                Ok(_) => {
                    match runtime.take_host_buffer() {
                        None => (None, None, false), // success, no ret
                        Some(result) => match result.into_t() {
                            Ok(ret) => (Some(ret), None, false),
                            Err(error) => (None, Some(Error::CLValue(error)), false),
                        },
                    }
                }
            }
        };

        let runtime_context = runtime.context();

        let cost = runtime_context.gas_counter();

        let effect = if revert_effect {
            effect_snapshot
        } else {
            runtime_context.effect()
        };

        let execution_result = match maybe_error {
            Some(error) => ExecutionResult::Failure {
                error: error.into(),
                effect,
                cost,
            },
            None => ExecutionResult::Success { effect, cost },
        };

        match maybe_ret {
            Some(ret) => execution_result.take_with_ret(ret),
            None => execution_result.take_without_ret(),
        }
    }

    /// Used to execute arbitrary wasm; necessary for running system contract installers / upgraders
    /// This is not meant to be used for executing system contracts.
    pub fn exec_wasm_direct<R, T>(
        &self,
        module: Module,
        entry_point_name: &str,
        args: RuntimeArgs,
        account: &mut Account,
        authorization_keys: BTreeSet<AccountHash>,
        blocktime: BlockTime,
        deploy_hash: [u8; 32],
        gas_limit: Gas,
        hash_address_generator: Rc<RefCell<AddressGenerator>>,
        uref_address_generator: Rc<RefCell<AddressGenerator>>,
        protocol_version: ProtocolVersion,
        correlation_id: CorrelationId,
        tracking_copy: Rc<RefCell<TrackingCopy<R>>>,
        phase: Phase,
        protocol_data: ProtocolData,
        system_contract_cache: SystemContractCache,
    ) -> Result<T, Error>
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<Error>,
        T: FromBytes + CLTyped,
    {
        let mut named_keys: NamedKeys = account.named_keys().clone();
        let base_key = account.account_hash().into();

        let (instance, mut runtime) = self.create_runtime(
            module,
            EntryPointType::Session,
            args,
            &mut named_keys,
            Default::default(),
            base_key,
            account,
            authorization_keys,
            blocktime,
            deploy_hash,
            gas_limit,
            hash_address_generator,
            uref_address_generator,
            protocol_version,
            correlation_id,
            tracking_copy,
            phase,
            protocol_data,
            system_contract_cache,
        )?;

        let error: wasmi::Error = match instance.invoke_export(entry_point_name, &[], &mut runtime)
        {
            Err(error) => error,
            Ok(_) => {
                // This duplicates the behavior of runtime sub_call.
                // If `instance.invoke_export` returns `Ok` and the `host_buffer` is `None`, the
                // contract's execution succeeded but did not explicitly call `runtime::ret()`.
                // Treat as though the execution returned the unit type `()` as per Rust
                // functions which don't specify a return value.
                let result = runtime.take_host_buffer().unwrap_or(CLValue::from_t(())?);
                let ret = result.into_t()?;
                *account.named_keys_mut() = named_keys;
                return Ok(ret);
            }
        };

        let return_value: CLValue = match error
            .as_host_error()
            .and_then(|host_error| host_error.downcast_ref::<Error>())
        {
            Some(Error::Ret(_)) => runtime
                .take_host_buffer()
                .ok_or(Error::ExpectedReturnValue)?,
            Some(Error::Revert(code)) => return Err(Error::Revert(*code)),
            Some(error) => return Err(error.clone()),
            _ => return Err(Error::Interpreter(error.into())),
        };

        let ret = return_value.into_t()?;
        *account.named_keys_mut() = named_keys;
        Ok(ret)
    }

    pub fn create_runtime<'a, R>(
        &self,
        module: Module,
        entry_point_type: EntryPointType,
        runtime_args: RuntimeArgs,
        named_keys: &'a mut NamedKeys,
        extra_keys: &[Key],
        base_key: Key,
        account: &'a Account,
        authorization_keys: BTreeSet<AccountHash>,
        blocktime: BlockTime,
        deploy_hash: [u8; 32],
        gas_limit: Gas,
        hash_address_generator: Rc<RefCell<AddressGenerator>>,
        uref_address_generator: Rc<RefCell<AddressGenerator>>,
        protocol_version: ProtocolVersion,
        correlation_id: CorrelationId,
        tracking_copy: Rc<RefCell<TrackingCopy<R>>>,
        phase: Phase,
        protocol_data: ProtocolData,
        system_contract_cache: SystemContractCache,
    ) -> Result<(ModuleRef, Runtime<'a, R>), Error>
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<Error>,
    {
        let access_rights = {
            let mut keys: Vec<Key> = named_keys.values().cloned().collect();
            keys.extend(extra_keys);
            extract_access_rights_from_keys(keys)
        };

        let gas_counter = Gas::default();

        let runtime_context = RuntimeContext::new(
            tracking_copy,
            entry_point_type,
            named_keys,
            access_rights,
            runtime_args,
            authorization_keys,
            account,
            base_key,
            blocktime,
            deploy_hash,
            gas_limit,
            gas_counter,
            hash_address_generator,
            uref_address_generator,
            protocol_version,
            correlation_id,
            phase,
            protocol_data,
        );

        let (instance, memory) = instance_and_memory(module.clone(), protocol_version)?;

        let runtime = Runtime::new(
            self.config,
            system_contract_cache,
            memory,
            module,
            runtime_context,
        );

        Ok((instance, runtime))
    }
}

pub enum DirectSystemContractCall {
    FinalizePayment,
    CreatePurse,
    Transfer,
}

impl DirectSystemContractCall {
    fn entry_point_name(&self) -> &str {
        match self {
            DirectSystemContractCall::FinalizePayment => "finalize_payment",
            DirectSystemContractCall::CreatePurse => "create",
            DirectSystemContractCall::Transfer => "transfer",
        }
    }

    fn host_exec<R, T>(
        &self,
        mut runtime: Runtime<R>,
        protocol_version: ProtocolVersion,
        named_keys: &mut NamedKeys,
        runtime_args: &RuntimeArgs,
        extra_keys: &[Key],
        execution_effect: ExecutionEffect,
    ) -> (Option<T>, ExecutionResult)
    where
        R: StateReader<Key, StoredValue>,
        R::Error: Into<Error>,
        T: FromBytes + CLTyped,
    {
        let entry_point_name = self.entry_point_name();
        let result = match self {
            DirectSystemContractCall::FinalizePayment => runtime.call_host_proof_of_stake(
                protocol_version,
                entry_point_name,
                named_keys,
                runtime_args,
                extra_keys,
            ),
            DirectSystemContractCall::CreatePurse | DirectSystemContractCall::Transfer => runtime
                .call_host_mint(
                    protocol_version,
                    entry_point_name,
                    named_keys,
                    runtime_args,
                    extra_keys,
                ),
        };

        match result {
            Ok(value) => match value.into_t() {
                Ok(ret) => ExecutionResult::Success {
                    effect: runtime.context().effect(),
                    cost: runtime.context().gas_counter(),
                }
                .take_with_ret(ret),
                Err(error) => ExecutionResult::Failure {
                    error: Error::CLValue(error).into(),
                    effect: execution_effect,
                    cost: runtime.context().gas_counter(),
                }
                .take_without_ret(),
            },
            Err(error) => ExecutionResult::Failure {
                error: error.into(),
                effect: execution_effect,
                cost: runtime.context().gas_counter(),
            }
            .take_without_ret(),
        }
    }
}
