//! Macro that describes all the host functions in the system.

/// Macro that when executed lists all host functions.
///
/// This can be useful to generate boilerplate code to implement imports table for various VM
/// backends. Due to the fact that types are preserved there is no need for a runtime dispatch of
/// arguments which will decrease performance.
///
/// Always make sure this list of host functions is sorted in alphabetical for possible performance
/// improvements necessary for different backends.
#[macro_export]
macro_rules! for_each_host_function {
    ($hf:ident) => {
        $hf! {
            fn casper_add(key_ptr: u32, key_size: u32, value_ptr: u32, value_size: u32);
            fn casper_add_associated_key(account_hash_ptr: u32, account_hash_size: u32, weight: i32) -> i32;
            fn casper_add_contract_version(
                contract_package_hash_ptr: u32,
                contract_package_hash_size: u32,
                version_ptr: u32,
                entry_points_ptr: u32,
                entry_points_size: u32,
                named_keys_ptr: u32,
                named_keys_size: u32,
                output_ptr: u32,
                output_size: u32,
                bytes_written_ptr: u32
            ) -> i32;
            fn casper_blake2b(in_ptr: u32, in_size: u32, out_ptr: u32, out_size: u32) -> i32;
            fn casper_call_contract(
                contract_hash_ptr: u32,
                contract_hash_size: u32,
                entry_point_name_ptr: u32,
                entry_point_name_size: u32,
                args_ptr: u32,
                args_size: u32,
                result_size_ptr: u32
            ) -> i32;
            fn casper_call_versioned_contract(
                contract_package_hash_ptr: u32,
                contract_package_hash_size: u32,
                contract_version_ptr: u32,
                contract_package_size: u32,
                entry_point_name_ptr: u32,
                entry_point_name_size: u32,
                args_ptr: u32,
                args_size: u32,
                result_size_ptr: u32
            ) -> i32;
            fn casper_create_contract_package_at_hash(
                hash_dest_ptr: u32,
                access_dest_ptr: u32,
                is_locked: u32
            );
            fn casper_create_contract_user_group(
                package_key_ptr: u32,
                package_key_size: u32,
                label_ptr: u32,
                label_size: u32,
                num_new_urefs: u32,
                existing_urefs_ptr: u32,
                existing_urefs_size: u32,
                output_size_ptr: u32
            ) -> i32;
            fn casper_create_purse(dest_ptr: u32, dest_size: u32) -> i32;
            fn casper_dictionary_get(
                uref_ptr: u32,
                uref_size: u32,
                key_bytes_ptr: u32,
                key_bytes_size: u32,
                output_size_ptr: u32
            ) -> i32;
            fn casper_dictionary_put(
                uref_ptr: u32,
                uref_size: u32,
                key_bytes_ptr: u32,
                key_bytes_size: u32,
                value_ptr: u32,
                value_ptr_size: u32
            ) -> i32;
            fn casper_dictionary_read(key_ptr: u32, key_size: u32, output_size_ptr: u32) -> i32;
            fn casper_disable_contract_version(
                package_key_ptr: u32,
                package_key_size: u32,
                contract_hash_ptr: u32,
                contract_hash_size: u32
            ) -> i32;
            fn casper_get_balance(ptr: u32, ptr_size: u32, output_size_ptr: u32) -> i32;
            fn casper_get_blocktime(dest_ptr: u32);
            fn casper_get_caller(output_size_ptr: u32) ->i32;
            fn casper_get_key(
                name_ptr: u32,
                name_size: u32,
                output_ptr: u32,
                output_size: u32,
                bytes_written: u32
            ) -> i32;
            fn casper_get_main_purse(dest_ptr: u32);
            fn casper_get_named_arg(name_ptr: u32, name_size: u32, dest_ptr: u32, dest_size: u32) -> i32;
            fn casper_get_named_arg_size(name_ptr: u32, name_size: u32, size_ptr: u32) -> i32;
            fn casper_get_phase(dest_ptr: u32);
            fn casper_get_system_contract(system_contract_index: u32, dest_ptr: u32, dest_size: u32) -> i32;
            fn casper_has_key(name_ptr: u32, name_size: u32) -> i32;
            fn casper_is_valid_uref(uref_ptr: u32, uref_size: u32) -> i32;
            fn casper_load_authorization_keys(len_ptr: u32, result_size_ptr: u32) -> i32;
            fn casper_load_call_stack(call_stack_len_ptr: u32, result_size_ptr: u32) -> i32;
            fn casper_load_named_keys(total_keys_ptr: u32, result_size_ptr: u32) -> i32;
            fn casper_new_dictionary(output_size_ptr: u32) -> i32;
            fn casper_new_uref(uref_ptr: u32, value_ptr: u32, value_size: u32);
            #[cfg(feature = "test-support")]
            fn casper_print(text_ptr: u32, text_size: u32);
            fn casper_provision_contract_user_group_uref(
                package_ptr: u32,
                package_size: u32,
                label_ptr: u32,
                label_size: u32,
                value_size_ptr: u32
            ) -> i32;
            fn casper_put_key(name_ptr: u32, name_size: u32, key_ptr: u32, key_size: u32);
            fn casper_random_bytes(out_ptr: u32, out_size: u32) -> i32;
            fn casper_read_host_buffer(dest_ptr: u32, dest_size: u32, bytes_written_ptr: u32) -> i32;
            fn casper_read_value(key_ptr: u32, key_size: u32, output_size_ptr: u32) -> i32;
            fn casper_remove_associated_key(account_hash_ptr: u32, account_hash_size: u32) -> i32;
            fn casper_remove_contract_user_group(
                package_key_ptr: u32,
                package_key_size: u32,
                label_ptr: u32,
                label_size: u32
            ) -> i32;
            fn casper_remove_contract_user_group_urefs(
                package_ptr: u32,
                package_size: u32,
                label_ptr: u32,
                label_size: u32,
                urefs_ptr: u32,
                urefs_size: u32
            ) -> i32;
            fn casper_remove_key(name_ptr: u32, name_size: u32);
            fn casper_ret(value_ptr: u32, value_size: u32);
            fn casper_revert(param: u32);
            fn casper_set_action_threshold(permission_level: u32, permission_threshold: u32) -> i32;
            fn casper_transfer_from_purse_to_account(
                source_ptr: u32,
                source_size: u32,
                key_ptr: u32,
                key_size: u32,
                amount_ptr: u32,
                amount_size: u32,
                id_ptr: u32,
                id_size: u32,
                result_ptr: u32
            ) -> i32;
            fn casper_transfer_from_purse_to_purse(
                source_ptr: u32,
                source_size: u32,
                target_ptr: u32,
                target_size: u32,
                amount_ptr: u32,
                amount_size: u32,
                id_ptr: u32,
                id_size: u32
            ) -> i32;
            fn casper_transfer_to_account(
                key_ptr: u32,
                key_size: u32,
                amount_ptr: u32,
                amount_size: u32,
                id_ptr: u32,
                id_size: u32,
                result_ptr: u32
            ) -> i32;
            fn casper_update_associated_key(account_hash_ptr: u32, account_hash_size: u32, weight: i32) -> i32;
            fn casper_write(key_ptr: u32, key_size: u32, value_ptr: u32, value_size: u32);
            fn gas(param: u32);
        }
    };
}
