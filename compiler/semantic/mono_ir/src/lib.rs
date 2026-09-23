//! Concrete, C-like control-flow IR produced after semantic IR.
//!
//! A [`MonoIR`] is deliberately scoped to one concrete source-definition
//! instance. It owns that definition's root function and all nested functions
//! produced from its lambdas, thunks, and operation handlers. A fragment can
//! instead hold a compiler-generated nominal `Drop.drop` body. Calls to other
//! fragments retain [`MonoFragmentInstance`] keys so a future incremental
//! orchestrator can request and reuse their independently cached fragments.
//!
//! This crate models the IR and exposes the cached query for concrete handler
//! layouts. Definition lowering lives in `rayc_mono_ir_builder`.

use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_arena::Arena;
use rayc_hash::FxHashMap;
use rayc_type::ty::application::ClosureID;

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
pub mod operand;
pub mod place;
pub mod rvalue;
pub mod ty;

pub use instance::{
    MonoClosureInstance, MonoDefInstance, MonoEffectInstance, MonoFragmentInstance,
    MonoNominalDropInstance, MonoStructInstance,
};

/// The independently cacheable `MonoIR` fragment for one concrete definition
/// or generated nominal Drop body.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoIR {
    instance: MonoFragmentInstance,
    functions: Arena<MonoFunction>,
    root: MonoFunctionID,
    closures: FxHashMap<ClosureID, MonoFunctionID>,
}

impl MonoIR {
    #[must_use]
    pub fn new(
        instance: impl Into<MonoFragmentInstance>,
        root_signature: FunctionSignature,
    ) -> Self {
        let instance = instance.into();
        let mut functions = Arena::new();
        let root = functions.insert(MonoFunction::new(MonoFunctionKind::Def, root_signature));
        Self { instance, functions, root, closures: FxHashMap::default() }
    }

    pub fn register_closure(&mut self, closure: ClosureID, function: MonoFunctionID) {
        assert!(
            matches!(self.instance, MonoFragmentInstance::Definition(_)),
            "only definition fragments own source closures"
        );
        let _ = self.get_function(function);
        assert!(!self.closures.values().any(|id| *id == function));
        assert!(self.closures.insert(closure, function).is_none());
    }

    #[must_use]
    pub fn closure_function(&self, closure: ClosureID) -> Option<MonoFunctionID> {
        self.closures.get(&closure).copied()
    }

    #[must_use]
    pub fn closure_instance(&self, function: MonoFunctionID) -> Option<MonoClosureInstance> {
        let owner = match &self.instance {
            MonoFragmentInstance::Definition(owner) => owner,
            MonoFragmentInstance::NominalDrop(_) => return None,
        };
        self.closures.iter().find_map(|(closure, id)| {
            (*id == function).then(|| MonoClosureInstance::new(owner.clone(), *closure))
        })
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
        assert_ne!(kind, MonoFunctionKind::Def, "a MonoIR fragment has exactly one root def");
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
