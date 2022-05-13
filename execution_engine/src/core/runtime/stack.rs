//! Runtime stacks.

use casper_types::{account::AccountHash, system::CallStackElement, PublicKey};

/// Representation of a context of given call stack.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExecutionContext {
    /// Call stack frame is invoked by a host.
    ///
    /// For example if user is executing a mint through a Wasm host function then a new mint's call
    /// frame will be marked as Host.
    Host,
    /// Call stack frame is created by a user.
    User,
}

/// A runtime stack frame.
///
/// Currently it aliases to a [`CallStackElement`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RuntimeStackFrame {
    execution_context: ExecutionContext,
    call_stack_element: CallStackElement,
}

impl RuntimeStackFrame {
    /// Creates new runtime stack frame object.
    pub fn new(execution_context: ExecutionContext, call_stack_element: CallStackElement) -> Self {
        Self {
            execution_context,
            call_stack_element,
        }
    }

    /// Get the runtime stack frame's call stack element.
    #[must_use]
    pub fn call_stack_element(&self) -> &CallStackElement {
        &self.call_stack_element
    }

    /// Get the runtime stack frame's execution context.
    #[must_use]
    pub fn execution_context(&self) -> ExecutionContext {
        self.execution_context
    }
}

/// The runtime stack.
#[derive(Clone)]
pub struct RuntimeStack {
    frames: Vec<RuntimeStackFrame>,
    max_height: usize,
}

/// Error returned on an attempt to pop off an empty stack.
#[cfg(test)]
#[derive(Debug)]
struct RuntimeStackUnderflow;

/// Error returned on an attempt to push to a stack already at the maximum height.
#[derive(Debug)]
pub struct RuntimeStackOverflow;

impl RuntimeStack {
    /// Creates an empty stack.
    pub fn new(max_height: usize) -> Self {
        Self {
            frames: Vec::with_capacity(max_height),
            max_height,
        }
    }

    /// Creates a stack with one entry.
    pub fn new_with_frame(max_height: usize, frame: RuntimeStackFrame) -> Self {
        let mut frames = Vec::with_capacity(max_height);
        frames.push(frame);
        Self { frames, max_height }
    }

    /// Creates a new call instance that starts with a system account.
    pub(crate) fn new_system_call_stack(max_height: usize) -> Self {
        RuntimeStack::new_with_frame(
            max_height,
            RuntimeStackFrame::new(
                ExecutionContext::Host,
                CallStackElement::session(PublicKey::System.to_account_hash()),
            ),
        )
    }

    /// Is the stack empty?
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The height of the stack.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// The current stack frame.
    pub fn current_frame(&self) -> Option<&RuntimeStackFrame> {
        self.frames.last()
    }

    /// The previous stack frame.
    pub fn previous_frame(&self) -> Option<&RuntimeStackFrame> {
        self.frames.iter().nth_back(1)
    }

    /// The first stack frame.
    pub fn first_frame(&self) -> Option<&RuntimeStackFrame> {
        self.frames.first()
    }

    /// Pops the current frame from the stack.
    #[cfg(test)]
    fn pop(&mut self) -> Result<(), RuntimeStackUnderflow> {
        self.frames.pop().ok_or(RuntimeStackUnderflow)?;
        Ok(())
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.frames.capacity()
    }

    /// Pushes a frame onto the stack.
    pub fn push(&mut self, frame: RuntimeStackFrame) -> Result<(), RuntimeStackOverflow> {
        if self.len() < self.max_height {
            self.frames.push(frame);
            Ok(())
        } else {
            Err(RuntimeStackOverflow)
        }
    }

    /// A view of the stack in the format readable by Wasm.
    pub fn call_stack_elements(&self) -> impl Iterator<Item = &CallStackElement> {
        self.frames
            .iter()
            .map(|runtime_frame| runtime_frame.call_stack_element())
    }

    /// Returns a stack with exactly one session element with the associated account hash.
    pub fn from_account_hash(
        account_hash: AccountHash,
        max_height: usize,
    ) -> Result<Self, RuntimeStackOverflow> {
        let mut runtime_stack = Self::new(max_height);

        let frame = {
            let session = CallStackElement::session(account_hash);
            RuntimeStackFrame::new(ExecutionContext::User, session)
        };
        runtime_stack.push(frame)?;

        Ok(runtime_stack)
    }
}

#[cfg(test)]
mod test {
    use core::convert::TryInto;

    use casper_types::account::{AccountHash, ACCOUNT_HASH_LENGTH};

    use super::*;

    const MAX_HEIGHT: usize = 6;

    fn nth_frame(n: usize) -> RuntimeStackFrame {
        let mut bytes = [0_u8; ACCOUNT_HASH_LENGTH];
        let n: u32 = n.try_into().unwrap();
        bytes[0..4].copy_from_slice(&n.to_le_bytes());
        RuntimeStackFrame::new(
            ExecutionContext::User,
            CallStackElement::session(AccountHash::new(bytes)),
        )
    }

    #[allow(clippy::redundant_clone)]
    #[test]
    fn stack_should_respect_max_height_after_clone() {
        const MAX_HEIGHT: usize = 3;
        let mut stack = RuntimeStack::new(MAX_HEIGHT);
        stack.push(nth_frame(1)).unwrap();

        let mut stack2 = stack.clone();
        stack2.push(nth_frame(2)).unwrap();
        stack2.push(nth_frame(3)).unwrap();
        stack2.push(nth_frame(4)).unwrap_err();
        assert_eq!(stack2.len(), MAX_HEIGHT);
    }

    #[test]
    fn stack_should_work_as_expected() {
        let mut stack = RuntimeStack::new(MAX_HEIGHT);
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.current_frame(), None);
        assert_eq!(stack.previous_frame(), None);
        assert_eq!(stack.first_frame(), None);

        stack.push(nth_frame(0)).unwrap();
        assert!(!stack.is_empty());
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.current_frame(), Some(&nth_frame(0)));
        assert_eq!(stack.previous_frame(), None);
        assert_eq!(stack.first_frame(), Some(&nth_frame(0)));

        let mut n: usize = 1;
        while stack.push(nth_frame(n)).is_ok() {
            n += 1;
            assert!(!stack.is_empty());
            assert_eq!(stack.len(), n);
            assert_eq!(stack.current_frame(), Some(&nth_frame(n - 1)));
            assert_eq!(stack.previous_frame(), Some(&nth_frame(n - 2)));
            assert_eq!(stack.first_frame(), Some(&nth_frame(0)));
        }
        assert!(!stack.is_empty());
        assert_eq!(stack.len(), MAX_HEIGHT);
        assert_eq!(stack.current_frame(), Some(&nth_frame(MAX_HEIGHT - 1)));
        assert_eq!(stack.previous_frame(), Some(&nth_frame(MAX_HEIGHT - 2)));
        assert_eq!(stack.first_frame(), Some(&nth_frame(0)));

        while stack.len() >= 3 {
            stack.pop().unwrap();
            n = n.checked_sub(1).unwrap();
            assert!(!stack.is_empty());
            assert_eq!(stack.len(), n);
            assert_eq!(stack.current_frame(), Some(&nth_frame(n - 1)));
            assert_eq!(stack.previous_frame(), Some(&nth_frame(n - 2)));
            assert_eq!(stack.first_frame(), Some(&nth_frame(0)));
        }

        stack.pop().unwrap();
        assert!(!stack.is_empty());
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.current_frame(), Some(&nth_frame(0)));
        assert_eq!(stack.previous_frame(), None);
        assert_eq!(stack.first_frame(), Some(&nth_frame(0)));

        stack.pop().unwrap();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.current_frame(), None);
        assert_eq!(stack.previous_frame(), None);
        assert_eq!(stack.first_frame(), None);

        assert!(stack.pop().is_err());
    }

    #[test]
    fn should_have_correct_capacity_when_created_from_account_hash() {
        let runtime_stack =
            RuntimeStack::from_account_hash(AccountHash::new([0; 32]), MAX_HEIGHT).unwrap();
        assert_eq!(runtime_stack.capacity(), MAX_HEIGHT);
    }
}
