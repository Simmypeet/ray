//! Lowers the expressions, blocks and terminators of one semantic IR function
//! on top of the state in `function_lowering`.

mod call;
mod cfg;
mod closure;
mod coroutine;
mod drop;
mod effect;
mod expression;
mod function;
