//! Operations occuring during execution.
use std::{
    default::Default,
    fmt::{self, Display, Formatter},
    ops::{Add, AddAssign},
};

use casper_types::Key;

use crate::shared::additive_map::{Apply, ApplyAssign};

/// Representation of a single operation during execution.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum Op {
    /// Read value from a `Key`.
    Read,
    /// Write value under a `Key`.
    Write,
    /// Add a value into a `Key`.
    Add,
    /// No operation.
    NoOp,
}

impl<K> Apply<K> for Op {
    type Output = Op;

    fn apply_add(self, _key: K, other: Op) -> Op {
        match (self, other) {
            (a, Op::NoOp) => a,
            (Op::NoOp, b) => b,
            (Op::Read, Op::Read) => Op::Read,
            (Op::Add, Op::Add) => Op::Add,
            _ => Op::Write,
        }
    }
}

impl ApplyAssign<Key> for Op {
    fn apply_assign(&mut self, key: Key, other: Self) {
        *self = self.apply_add(key, other);
    }
}

impl Display for Op {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Default for Op {
    fn default() -> Self {
        Op::NoOp
    }
}

impl From<&Op> for casper_types::OpKind {
    fn from(op: &Op) -> Self {
        match op {
            Op::Read => casper_types::OpKind::Read,
            Op::Write => casper_types::OpKind::Write,
            Op::Add => casper_types::OpKind::Add,
            Op::NoOp => casper_types::OpKind::NoOp,
        }
    }
}
