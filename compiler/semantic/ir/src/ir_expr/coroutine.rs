use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

/// Creates a coroutine that has not been resumed yet: the arguments of a
/// suspended call, stored without running any of the callee.
///
/// The type of the expression is the coroutine type, which names the
/// suspended function and instantiates its polymorphic parameters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Coroutine {
    arguments: Vec<IRExprID>,
}

impl Coroutine {
    #[must_use]
    pub const fn new(arguments: Vec<IRExprID>) -> Self { Self { arguments } }

    /// Returns the arguments the coroutine holds until it is first resumed,
    /// in parameter order.
    #[must_use]
    pub fn arguments(&self) -> &[IRExprID] { &self.arguments }
}
