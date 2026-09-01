//! Concrete, C-like control-flow IR produced after semantic IR.
//!
//! A [`MonoIR`] is deliberately scoped to one concrete source-definition
//! instance. It owns that definition's root function and all nested functions
//! produced from its lambdas, thunks, and operation handlers. Calls to other
//! definitions retain [`MonoDefInstance`] keys so a future incremental
//! orchestrator can request and reuse their independently cached fragments.
//!
//! This crate only models the IR. It intentionally defines neither lowering
//! nor a qbice query.

use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_arena::Arena;

use crate::{
    cfg::{BlockID, Terminator},
    function::{Local, LocalID, MonoFunction, MonoFunctionID, MonoFunctionKind},
    instruction::Instruction,
    ty::{FunctionSignature, HandlerLayout},
};

pub mod cfg;
pub mod function;
pub mod instance;
pub mod instruction;
pub mod operand;
pub mod place;
pub mod rvalue;
pub mod ty;

pub use instance::{MonoDefInstance, MonoEffectInstance};

/// The independently cacheable `MonoIR` fragment for one concrete definition.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoIR {
    instance: MonoDefInstance,
    functions: Arena<MonoFunction>,
    root: MonoFunctionID,
    /// Layouts required by nominal handler types referenced by this fragment.
    handler_layouts: Vec<HandlerLayout>,
}

impl MonoIR {
    #[must_use]
    pub fn new(instance: MonoDefInstance, root_signature: FunctionSignature) -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(MonoFunction::new(MonoFunctionKind::Def, root_signature));
        Self { instance, functions, root, handler_layouts: Vec::new() }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoDefInstance { &self.instance }

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

    /// Returns the concrete handler-record layouts needed to emit this
    /// fragment.
    ///
    /// Keeping reachable layouts beside the functions makes an independently
    /// cached fragment self-describing. A program orchestrator may deduplicate
    /// equal layouts collected from multiple fragments before C code
    /// generation.
    #[must_use]
    pub fn handler_layouts(&self) -> &[HandlerLayout] { &self.handler_layouts }

    pub fn insert_handler_layout(&mut self, layout: HandlerLayout) {
        if let Some(existing) =
            self.handler_layouts.iter().find(|existing| existing.instance() == layout.instance())
        {
            assert_eq!(existing, &layout, "a concrete effect must have one handler layout");
            return;
        }
        self.handler_layouts.push(layout);
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
