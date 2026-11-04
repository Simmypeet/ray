//! Lowers the expressions, blocks and terminators of one semantic IR function
//! on top of the state in `function_lowering`.

mod address;
mod call;
mod cfg;
mod closure;
mod coro;
mod coroutine;
mod drop;
mod callback_effect;
mod environment;
mod expression;
mod forward;
mod function;
mod handle;
mod poll;
mod suspension;
