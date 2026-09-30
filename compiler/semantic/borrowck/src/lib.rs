//! Borrow checking of the IR functions of a definition.

pub mod active_loans;
pub mod constraint;
pub mod live_loans;
pub mod region_liveness;
pub mod renumber;
pub mod variance;

#[cfg(test)]
mod test_util;
