use derive_more::From;
use qbice::{Decode, Encode, StableHash};
use rayc_ir::{address::Local, ir_expr::IRExprID};

/// The values that can be saved into a coroutine frame
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, From,
)]
pub enum Savable {
    Local(Local),
    Expression(IRExprID),
}
