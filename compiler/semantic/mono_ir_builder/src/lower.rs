//! Lowers the expressions, blocks and terminators of one semantic IR function
//! on top of the state in `function_lowering`.

mod address;
mod call;
mod cfg;
mod closure;
mod coro;
mod dispatch_resume;
mod drop;
mod effect;
mod environment;
mod expression;
mod function;
mod handle;
mod r#yield;
