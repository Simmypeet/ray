use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_qbice::TrackedEngine;

use crate::{
    cfg::{BasicBlock, BlockID, MonoCfg, Terminator},
    instance::{MonoEffectInstance, MonoFunctionInstance},
    instruction::Instruction,
    ty::{FunctionSignature, MonoType},
};

pub type LocalID = ID<Local>;

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

/// One independently lowered function, with a parameter local for each slot of
/// its signature.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoFunction {
    instance: MonoFunctionInstance,
    signature: FunctionSignature,

    environment: Option<LocalID>,
    parameters: Vec<LocalID>,
    /// One local per handler-record slot, in the order of the slots.
    handler_records: Vec<LocalID>,

    locals: OrderedArena<Local>,
    cfg: MonoCfg,
}

impl MonoFunction {
    /// An empty function of `instance` with `signature`, whose entry block is
    /// yet to be filled.
    #[must_use]
    pub fn new(
        instance: impl Into<MonoFunctionInstance>,
        signature: FunctionSignature,
        engine: &TrackedEngine,
    ) -> Self {
        // The parameter locals are created in C order, so they are the first
        // locals of the function in that order.
        let mut locals = OrderedArena::new();
        let mut parameter = |ty| locals.insert(Local::new(ty, LocalKind::Parameter));

        let environment =
            signature.environment_slot().map(|slot| parameter(engine.intern(slot.ty())));
        let parameters = signature.parameter_types().iter().cloned().map(&mut parameter).collect();
        let handler_records = signature
            .handler_record_slots()
            .iter()
            .map(|effect| parameter(MonoType::new_handler_pointer(effect.clone(), engine)))
            .collect();

        Self {
            instance: instance.into(),
            signature,
            environment,
            parameters,
            handler_records,
            locals,
            cfg: MonoCfg::new(),
        }
    }

    /// What the function was lowered from, which is also how other functions
    /// refer to it.
    #[must_use]
    pub const fn instance(&self) -> &MonoFunctionInstance { &self.instance }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }

    /// The local that receives the environment, if the function has an
    /// environment slot.
    #[must_use]
    pub const fn environment(&self) -> Option<LocalID> { self.environment }

    /// The locals of the source parameters, without the environment and the
    /// handler records.
    #[must_use]
    pub fn parameters(&self) -> impl ExactSizeIterator<Item = LocalID> + '_ {
        self.parameters.iter().copied()
    }

    /// The local that receives each handler record, with its effect.
    #[must_use]
    pub fn handler_records(
        &self,
    ) -> impl ExactSizeIterator<Item = (&MonoEffectInstance, LocalID)> + '_ {
        self.signature.handler_record_slots().iter().zip(self.handler_records.iter().copied())
    }

    /// Every parameter local in C order: the environment, the source
    /// parameters, then the handler records.
    pub fn abi_parameters(&self) -> impl Iterator<Item = LocalID> + '_ {
        self.environment
            .into_iter()
            .chain(self.parameters.iter().copied())
            .chain(self.handler_records.iter().copied())
    }

    #[must_use]
    pub fn get_local(&self, local_id: LocalID) -> &Local {
        self.locals.get(local_id).expect("MonoFunction local should exist")
    }

    #[must_use]
    pub fn locals(&self) -> impl ExactSizeIterator<Item = (LocalID, &Local)> { self.locals.iter() }

    #[must_use]
    pub fn insert_local(&mut self, local: Local) -> LocalID { self.locals.insert(local) }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    pub fn push_instruction(&mut self, block_id: BlockID, instruction: Instruction) {
        self.cfg.push_instruction(block_id, instruction);
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn get_block(&self, block_id: BlockID) -> &BasicBlock { self.cfg.get_block(block_id) }

    #[must_use]
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = (BlockID, &BasicBlock)> {
        self.cfg.blocks()
    }
}
