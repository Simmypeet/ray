//! Concrete, C-like control-flow IR produced after semantic IR.
//!
//! A [`MonoIR`] is deliberately scoped to one concrete source definition or
//! closure. It owns that function as its root and the nested functions that
//! only it refers to: the thunks and operation handlers of its `run ... with`
//! expressions. A closure it creates is a fragment of its own. A fragment can
//! instead hold a compiler-generated nominal `Drop.drop` body or the
//! `coroutine_resume` function of a coroutine. Calls to other fragments retain
//! [`MonoFragmentInstance`] keys so a future incremental orchestrator can
//! request and reuse their independently cached fragments.
//!
//! This crate models the IR and exposes the cached queries for the layouts of
//! concrete aggregates. Definition lowering lives in `rayc_mono_ir_builder`.

use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_arena::Arena;

use crate::{
    cfg::{BlockID, Terminator},
    function::{Local, LocalID, MonoFunction, MonoFunctionID, MonoFunctionKind},
    instruction::Instruction,
    ty::FunctionSignature,
};

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
    MonoClosureInstance, MonoCoroutineInstance, MonoDefInstance, MonoEffectInstance,
    MonoFragmentInstance, MonoGeneratedAdtDropInstance, MonoStructInstance,
};

/// The independently cacheable `MonoIR` fragment for one concrete definition
/// or generated nominal Drop body.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoIR {
    instance: MonoFragmentInstance,
    functions: Arena<MonoFunction>,
    root: MonoFunctionID,
}

impl MonoIR {
    #[must_use]
    pub fn new(
        instance: impl Into<MonoFragmentInstance>,
        root_signature: FunctionSignature,
    ) -> Self {
        let instance = instance.into();
        let mut functions = Arena::new();
        let root = functions.insert(MonoFunction::new(instance.root_kind(), root_signature));
        Self { instance, functions, root }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoFragmentInstance { &self.instance }

    #[must_use]
    pub const fn root_id(&self) -> MonoFunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &MonoFunction {
        self.functions.get(self.root).expect("root MonoIR function should exist")
    }

    #[must_use]
    pub fn get_function(&self, function_id: MonoFunctionID) -> &MonoFunction {
        self.functions.get(function_id).expect("MonoIR function should exist")
    }

    fn get_function_mut(&mut self, function_id: MonoFunctionID) -> &mut MonoFunction {
        self.functions.get_mut(function_id).expect("MonoIR function should exist")
    }

    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (MonoFunctionID, &MonoFunction)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_function(
        &mut self,
        kind: MonoFunctionKind,
        signature: FunctionSignature,
    ) -> MonoFunctionID {
        assert!(
            matches!(kind, MonoFunctionKind::Thunk | MonoFunctionKind::OperationHandler),
            "only the thunks and operation handlers of a fragment are nested in it"
        );
        self.functions.insert(MonoFunction::new(kind, signature))
    }

    #[must_use]
    pub fn insert_local(&mut self, function_id: MonoFunctionID, local: Local) -> LocalID {
        self.get_function_mut(function_id).insert_local(local)
    }

    #[must_use]
    pub fn entry_block(&self, function_id: MonoFunctionID) -> BlockID {
        self.get_function(function_id).entry_block()
    }

    #[must_use]
    pub fn create_block(&mut self, function_id: MonoFunctionID) -> BlockID {
        self.get_function_mut(function_id).create_block()
    }

    pub fn push_instruction(
        &mut self,
        function_id: MonoFunctionID,
        block_id: BlockID,
        instruction: Instruction,
    ) {
        self.get_function_mut(function_id).push_instruction(block_id, instruction);
    }

    pub fn set_terminator(
        &mut self,
        function_id: MonoFunctionID,
        block_id: BlockID,
        terminator: Terminator,
    ) {
        self.get_function_mut(function_id).set_terminator(block_id, terminator);
    }
}
