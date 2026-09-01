use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};

use crate::{
    cfg::{BasicBlock, BlockID, MonoCfg, Terminator},
    instruction::Instruction,
    ty::{FunctionSignature, MonoType},
};

pub type MonoFunctionID = ID<MonoFunction>;
pub type LocalID = ID<Local>;

/// Descriptive origin of a function within a monomorphic definition fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum MonoFunctionKind {
    Def,
    Lambda,
    Thunk,
    OperationHandler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum LocalKind {
    Parameter,
    Variable,
    Temporary,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Local {
    ty: Interned<MonoType>,
    kind: LocalKind,
}

impl Local {
    #[must_use]
    pub const fn new(ty: Interned<MonoType>, kind: LocalKind) -> Self { Self { ty, kind } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<MonoType> { &self.ty }

    #[must_use]
    pub const fn kind(&self) -> LocalKind { self.kind }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoFunction {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
    parameters: Vec<LocalID>,
    locals: OrderedArena<Local>,
    cfg: MonoCfg,
}

impl MonoFunction {
    pub(crate) fn new(kind: MonoFunctionKind, signature: FunctionSignature) -> Self {
        let mut locals = OrderedArena::new();
        let parameters = signature
            .parameter_types()
            .iter()
            .map(|ty| locals.insert(Local::new(ty.clone(), LocalKind::Parameter)))
            .collect();

        Self { kind, signature, parameters, locals, cfg: MonoCfg::new() }
    }

    #[must_use]
    pub const fn kind(&self) -> MonoFunctionKind { self.kind }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }

    #[must_use]
    pub fn parameters(&self) -> impl ExactSizeIterator<Item = LocalID> + '_ {
        self.parameters.iter().copied()
    }

    #[must_use]
    pub fn get_local(&self, local_id: LocalID) -> &Local {
        self.locals.get(local_id).expect("MonoIR local should exist")
    }

    #[must_use]
    pub fn locals(&self) -> impl ExactSizeIterator<Item = (LocalID, &Local)> { self.locals.iter() }

    pub(crate) fn insert_local(&mut self, local: Local) -> LocalID { self.locals.insert(local) }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry() }

    pub(crate) fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    pub(crate) fn push_instruction(&mut self, block_id: BlockID, instruction: Instruction) {
        self.cfg.push_instruction(block_id, instruction);
    }

    pub(crate) fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn get_block(&self, block_id: BlockID) -> &BasicBlock { self.cfg.get_block(block_id) }

    #[must_use]
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = (BlockID, &BasicBlock)> {
        self.cfg.blocks()
    }
}
