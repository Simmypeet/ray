//! Lowers the expressions, blocks and terminators of one semantic IR function
//! on the `Emitter`, with the `Context` of the function.

mod address;
mod call;
mod callback_call;
mod callback_effect;
mod cfg;
mod closure;
mod coro;
mod coroutine;
mod drop;
mod environment;
mod expression;
mod forward;
mod function;
mod handle;
mod poll;
mod run_with_yield;
mod suspension;
