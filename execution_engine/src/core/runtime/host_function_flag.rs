use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex, RwLock},
};

use tracing::error;

/// A flag to indicate whether the current runtime call is made within the scope of a host function.
///
/// The flag is backed by an `Rc<Cell<u64>>`, meaning that clones will all share state.
#[derive(Default, Clone)]
pub(crate) struct HostFunctionFlag {
    /// A counter which, if non-zero, indicates that the `HostFunctionFlag` is `true`.
    counter: Arc<RwLock<u64>>,
}

impl HostFunctionFlag {
    /// Returns `true` if this `HostFunctionFlag` has entered any number of host function scopes
    /// without having exited them all.
    pub(crate) fn is_in_host_function_scope(&self) -> bool {
        *self.counter.read().unwrap() != 0
    }

    /// Must be called when entering a host function scope.
    ///
    /// The returned `ScopedHostFunctionFlag` must be kept alive for the duration of the host
    /// function call.  While at least one such `ScopedHostFunctionFlag` exists,
    /// `is_in_host_function_scope()` returns `true`.
    #[must_use]
    pub(crate) fn enter_host_function_scope(&self) -> ScopedHostFunctionFlag {
        let new_count = self
            .counter
            .read()
            .unwrap()
            .checked_add(1)
            .unwrap_or_else(|| {
                error!("checked_add failure in host function flag counter");
                debug_assert!(false, "checked_add failure in host function flag counter");
                u64::MAX
            });
        *self.counter.write().unwrap() = new_count;
        ScopedHostFunctionFlag {
            counter: self.counter.clone(),
        }
    }
}

pub(crate) struct ScopedHostFunctionFlag {
    counter: Arc<RwLock<u64>>,
}

impl Drop for ScopedHostFunctionFlag {
    fn drop(&mut self) {
        let new_count = self
            .counter
            .read()
            .unwrap()
            .checked_sub(1)
            .unwrap_or_else(|| {
                error!("checked_sub failure in host function flag counter");
                debug_assert!(false, "checked_sub failure in host function flag counter");
                0
            });
        *self.counter.write().unwrap() = new_count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_handle_multiple_scopes() {
        let flag = HostFunctionFlag::default();
        assert!(!flag.is_in_host_function_scope());

        {
            let _outer_scope = flag.enter_host_function_scope();
            assert_eq!(*flag.counter.read().unwrap(), 1);
            assert!(flag.is_in_host_function_scope());

            {
                let _inner_scope = flag.enter_host_function_scope();
                assert_eq!(*flag.counter.read().unwrap(), 2);
                assert!(flag.is_in_host_function_scope());
            }

            assert_eq!(*flag.counter.read().unwrap(), 1);
            assert!(flag.is_in_host_function_scope());

            {
                let cloned_flag = flag.clone();
                assert_eq!(*cloned_flag.counter.read().unwrap(), 1);
                assert!(cloned_flag.is_in_host_function_scope());
                assert!(flag.is_in_host_function_scope());

                let _inner_scope = cloned_flag.enter_host_function_scope();
                assert_eq!(*cloned_flag.counter.read().unwrap(), 2);
                assert!(cloned_flag.is_in_host_function_scope());
                assert!(flag.is_in_host_function_scope());
            }

            assert_eq!(*flag.counter.read().unwrap(), 1);
            assert!(flag.is_in_host_function_scope());
        }

        assert_eq!(*flag.counter.read().unwrap(), 0);
        assert!(!flag.is_in_host_function_scope());
    }
}
