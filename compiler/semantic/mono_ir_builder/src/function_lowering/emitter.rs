//! Emits instructions and blocks into one `MonoIR` function.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoDefInstance, MonoIR,
    cfg::{BlockID, Terminator},
    function::{Local, LocalID, LocalKind, MonoFunctionID},
    instance::FunctionReference,
    instruction::{Assign, Call, Instruction},
    operand::{Constant, Operand},
    place::{Place, PlaceProjector},
    rvalue::Rvalue,
    signature::CoroutineSignature,
    ty::{AggregateType, MonoType},
};
use rayc_qbice::TrackedEngine;

use crate::resolver::Resolver;

/// The function being emitted, and the block that instructions go to.
pub(crate) struct Emitter<'a> {
    resolver: &'a Resolver,
    output: &'a mut MonoIR,
    target_id: MonoFunctionID,
    block: BlockID,
}

impl<'a> Emitter<'a> {
    pub(crate) fn new(
        resolver: &'a Resolver,
        output: &'a mut MonoIR,
        target_id: MonoFunctionID,
    ) -> Self {
        let block = output.entry_block(target_id);
        Self { resolver, output, target_id, block }
    }

    pub(crate) const fn resolver(&self) -> &'a Resolver { self.resolver }

    pub(crate) const fn engine(&self) -> &'a TrackedEngine { self.resolver.engine() }

    pub(crate) fn parameter_ids(&self) -> impl ExactSizeIterator<Item = LocalID> + '_ {
        self.output.get_function(self.target_id).parameters()
    }

    /// The place of a whole local.
    pub(crate) fn place(&self, local: LocalID) -> Place { Place::new(local, self.engine()) }

    /// Starts a projection of `place`.
    pub(crate) const fn project<'p>(&self, place: &'p Place) -> PlaceProjector<'p>
    where
        'a: 'p,
    {
        place.project(self.engine())
    }

    pub(crate) fn new_temporary_local(&mut self, ty: Interned<MonoType>) -> LocalID {
        self.output.insert_local(self.target_id, Local::new(ty, LocalKind::Temporary))
    }

    pub(crate) fn new_temporary_place(&mut self, ty: Interned<MonoType>) -> Place {
        let local = self.new_temporary_local(ty);
        self.place(local)
    }

    pub(crate) fn new_variable_place(&mut self, ty: Interned<MonoType>) -> Place {
        let local = self.output.insert_local(self.target_id, Local::new(ty, LocalKind::Variable));
        self.place(local)
    }

    /// A temporary for what a `coroutine_resume` with `signature` returns.
    pub(crate) fn new_progress_temporary_place(&mut self, signature: &CoroutineSignature) -> Place {
        let progress = AggregateType::CoroutineProgress(signature.progress().clone());
        self.new_temporary_place(MonoType::new_aggregate(progress, self.engine()))
    }

    /// A temporary for the unit result of calls whose result nothing reads.
    pub(crate) fn new_unit_place(&mut self) -> Place {
        self.new_temporary_place(MonoType::new_unit(self.engine()))
    }

    pub(crate) fn entry_block(&self) -> BlockID { self.output.entry_block(self.target_id) }

    pub(crate) fn create_block(&mut self) -> BlockID { self.output.create_block(self.target_id) }

    pub(crate) const fn select_block(&mut self, block: BlockID) { self.block = block; }

    /// Creates a block and fills it with `emit`, which should terminate it. The
    /// current block stays selected afterwards.
    pub(crate) fn in_new_block(&mut self, emit: impl FnOnce(&mut Self)) -> BlockID {
        let block = self.create_block();
        self.in_block(block, emit);

        block
    }

    /// Fills `block` with `emit`, which should terminate it. The current block
    /// stays selected afterwards.
    pub(crate) fn in_block(&mut self, block: BlockID, emit: impl FnOnce(&mut Self)) {
        let previous = self.block;
        self.block = block;
        emit(self);
        self.block = previous;
    }

    pub(crate) fn push_instruction(&mut self, instruction: Instruction) {
        self.output.push_instruction(self.target_id, self.block, instruction);
    }

    pub(crate) fn assign(&mut self, destination: Place, value: Rvalue) {
        self.push_instruction(Instruction::Assign(Assign::new(destination, value)));
    }

    /// Stores the value of `operand` in `destination`.
    pub(crate) fn store(&mut self, destination: Place, operand: Operand) {
        self.assign(destination, Rvalue::Use(operand));
    }

    pub(crate) fn store_constant(&mut self, destination: Place, constant: Constant) {
        self.store(destination, Operand::Constant(constant));
    }

    pub(crate) fn call(
        &mut self,
        destination: Option<Place>,
        callee: Operand,
        arguments: Vec<Operand>,
    ) {
        self.push_instruction(Instruction::Call(Call::new(destination, callee, arguments)));
    }

    pub(crate) async fn lower_call_with_args(
        &mut self,
        callee: MonoDefInstance,
        arguments: Vec<Operand>,
        destination: Place,
    ) {
        let callee_signature = self.resolver().def_signature(&callee).await;
        let is_void = callee_signature.signature().return_type().is_void();
        let callee = Operand::Function(FunctionReference::Global(callee));

        // if the function has `void` return type, which is mostly from `extern
        // def`, we don't need to assign the return value to the
        // destination place
        let call_destination = (!is_void).then(|| destination.clone());

        self.call(call_destination, callee, arguments);

        // if we are calling a `void`  function, we need to assign "fake" unit
        // value to the destination place. (Actually, we don't need to
        // assign anything, since unit type has only one value, and we
        // can just use uninitialized value)
        if is_void {
            self.store_constant(destination, Constant::Unit);
        }
    }

    pub(crate) fn set_terminator(&mut self, terminator: Terminator) {
        self.output.set_terminator(self.target_id, self.block, terminator);
    }
}
