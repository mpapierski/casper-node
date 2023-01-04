//! Contains definitions for panic and allocation error handlers.

/// A panic handler for use in a `no_std` environment which simply aborts the process.
#[cfg(all(target_arch = "wasm32", not(feature = "std")))]
#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// An out-of-memory allocation error handler for use in a `no_std` environment which simply aborts
/// the process.
#[cfg(all(target_arch = "wasm32", feature = "nightly"))]
#[alloc_error_handler]
pub fn oom(_: core::alloc::Layout) -> ! {
    core::arch::wasm32::unreachable()
}

#[cfg(feature = "nightly")]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}
