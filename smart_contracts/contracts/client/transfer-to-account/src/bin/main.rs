#![no_main]

#[no_mangle]
pub extern "C" fn call() {
    transfer_to_account::delegate();
}
