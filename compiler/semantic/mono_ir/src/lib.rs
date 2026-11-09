//! Concrete, C-like control-flow IR produced after semantic IR.
//!
//! Every [`MonoFunction`] is lowered on its own, keyed by its
//! [`MonoFunctionInstance`]: one function of the IR of a concrete source
//! definition (the definition itself, a closure body, the body of a
//! `run ... with` or one of its operation handlers), a compiler-generated
//! nominal `Drop.drop` body, or the `coroutine_resume` function of a coroutine.
//! A function refers to every other function by its instance, so a future
//! incremental orchestrator can request and reuse each independently cached
//! function.
//!
//! [`MonoFunction`]: function::MonoFunction
//!
//! This crate models the IR and exposes the cached queries for the layouts of
//! concrete aggregates. Definition lowering lives in `rayc_mono_ir_builder`.

pub mod cfg;
pub mod function;
pub mod instance;
pub mod instruction;
pub mod layout;
pub mod operand;
pub mod place;
pub mod rvalue;
pub mod signature;
pub mod ty;

pub use instance::{
    MonoBodyInstance, MonoCoroutineInstance, MonoEffectInstance, MonoEnvironmentInstance,
    MonoFunctionInstance, MonoGeneratedAdtDropInstance, MonoRunWithInstance, MonoStructInstance,
};
