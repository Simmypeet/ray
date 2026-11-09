use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::ir_closure::CaptureID;
use rayc_mono_ir::{
    MonoEffectInstance, MonoFunctionInstance,
    cfg::{BlockID, Switch, Terminator},
    function::{Local, LocalID, LocalKind, MonoFunction},
    instruction::{Assign, Call, Instruction},
    layout::HandlerRecordID,
    operand::{Constant, Operand},
    place::{FieldIndex, Place, PlaceProjector, Projection},
    rvalue::Rvalue,
    signature::CoroutineSignature,
    ty::{AggregateType, EnvironmentSlot, MonoType},
};
use rayc_qbice::TrackedEngine;

use crate::{context::Context, resolver::Resolver};

/// The function being emitted, and the block that instructions go to.
pub(crate) struct Emitter<'a> {
    resolver: &'a Resolver,
    output: &'a mut MonoFunction,
    block: BlockID,
}

impl<'a> Emitter<'a> {
    pub(crate) const fn new(resolver: &'a Resolver, output: &'a mut MonoFunction) -> Self {
        let block = output.entry_block();
        Self { resolver, output, block }
    }

    pub(crate) const fn resolver(&self) -> &'a Resolver { self.resolver }

    pub(crate) const fn engine(&self) -> &'a TrackedEngine { self.resolver.engine() }

    /// The local that receives the environment, if the function has an
    /// environment slot.
    pub(crate) const fn environment_parameter(&self) -> Option<LocalID> {
        self.output.environment()
    }

    /// How the function receives its environment, if it has an environment
    /// slot.
    pub(crate) const fn environment_slot(&self) -> Option<&EnvironmentSlot> {
        self.output.signature().environment_slot()
    }

    /// The locals of the source parameters.
    pub(crate) fn parameter_ids(&self) -> impl ExactSizeIterator<Item = LocalID> + '_ {
        self.output.parameters()
    }

    /// The local that receives each handler record, with its effect.
    pub(crate) fn handler_record_parameters(
        &self,
    ) -> impl ExactSizeIterator<Item = (&MonoEffectInstance, LocalID)> + '_ {
        self.output.handler_records()
    }

    /// The place of a whole local.
    pub(crate) fn local_place(&self, local: LocalID) -> Place { Place::new(local, self.engine()) }

    /// Starts a projection of `place`.
    pub(crate) const fn project<'p>(&self, place: &'p Place) -> PlaceProjector<'p>
    where
        'a: 'p,
    {
        place.project(self.engine())
    }

    pub(crate) fn new_temporary_local(&mut self, ty: Interned<MonoType>) -> LocalID {
        self.output.insert_local(Local::new(ty, LocalKind::Temporary))
    }

    pub(crate) fn new_temporary_place(&mut self, ty: Interned<MonoType>) -> Place {
        let local = self.new_temporary_local(ty);
        self.local_place(local)
    }

    pub(crate) fn new_variable_place(&mut self, ty: Interned<MonoType>) -> Place {
        let local = self.output.insert_local(Local::new(ty, LocalKind::Variable));
        self.local_place(local)
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

    pub(crate) const fn entry_block(&self) -> BlockID { self.output.entry_block() }

    pub(crate) fn create_block(&mut self) -> BlockID { self.output.create_block() }

    pub(crate) const fn select_block(&mut self, block: BlockID) { self.block = block; }

    pub(crate) fn push_instruction(&mut self, instruction: Instruction) {
        self.output.push_instruction(self.block, instruction);
    }

    pub(crate) fn assign(&mut self, destination: Place, value: Rvalue) {
        self.push_instruction(Instruction::Assign(Assign::new(destination, value)));
    }

    /// Allocates uninitialized heap memory for a value of type `ty`, and stores
    /// the pointer to it in `pointer`.
    ///
    /// ```c
    ///     pointer = ray_alloc(sizeof(ty));
    /// ```
    pub(crate) fn heap_allocate(&mut self, pointer: Place, ty: Interned<MonoType>) {
        self.assign(pointer, Rvalue::HeapAllocate(ty));
    }

    /// Frees the heap memory that `pointer` points to, without dropping what
    /// it holds.
    ///
    /// ```c
    ///     ray_free(pointer);
    /// ```
    pub(crate) fn heap_free(&mut self, pointer: Place) {
        self.push_instruction(Instruction::HeapFree(Operand::Copy(pointer)));
    }

    /// Stores the value of `operand` in `destination`.
    pub(crate) fn store(&mut self, destination: Place, operand: Operand) {
        self.assign(destination, Rvalue::Use(operand));
    }

    pub(crate) fn store_constant(&mut self, destination: Place, constant: Constant) {
        self.store(destination, Operand::Constant(constant));
    }

    /// Initializes the aggregate of type `ty` at `destination` by assigning
    /// each of `members` to its projection of `destination`.
    ///
    /// No operand may read `destination`, since each assignment is visible to
    /// the ones after it. An aggregate without members is assigned as a whole,
    /// so that every aggregate is initialized when it is later copied.
    pub(crate) fn initialize_aggregate(
        &mut self,
        destination: &Place,
        members: impl IntoIterator<Item = (Projection, Operand)>,
    ) {
        for (projection, operand) in members {
            self.store(self.project(destination).with_projection(projection), operand);
        }
    }

    /// Initializes the tuple of type `ty` at `destination` from one operand per
    /// element.
    pub(crate) fn initialize_tuple(
        &mut self,
        destination: &Place,
        fields: impl IntoIterator<Item = Operand>,
    ) {
        let members = fields.into_iter().enumerate().map(|(index, operand)| {
            (Projection::TupleFieldIndex(FieldIndex::from_usize(index)), operand)
        });
        self.initialize_aggregate(destination, members);
    }

    /// Initializes the environment at `destination` from an operand per
    /// capture and per handler record.
    pub(crate) fn initialize_environment(
        &mut self,
        destination: &Place,
        captures: impl IntoIterator<Item = (CaptureID, Operand)>,
        handler_records: impl IntoIterator<Item = (HandlerRecordID, Operand)>,
    ) {
        let captures = captures
            .into_iter()
            .map(|(capture, operand)| (Projection::EnvironmentCapture(capture), operand));
        let handler_records = handler_records
            .into_iter()
            .map(|(record, operand)| (Projection::EnvironmentHandlerRecord(record), operand));
        self.initialize_aggregate(destination, captures.chain(handler_records));
    }

    pub(crate) fn push_call(&mut self, call: Call) {
        self.push_instruction(Instruction::Call(call));
    }

    /// Calls `callee`, passing `handler_records` to its handler-record slots.
    pub(crate) async fn lower_call_with_args(
        &mut self,
        callee: MonoFunctionInstance,
        arguments: Vec<Operand>,
        handler_records: Vec<Operand>,
        destination: Place,
    ) {
        let callee_signature = self.resolver().function_signature(&callee).await;
        let is_void = callee_signature.return_type().is_void();
        let callee = Operand::Function(callee);

        // if the function has `void` return type, which is mostly from `extern
        // def`, we don't need to assign the return value to the
        // destination place
        let call_destination = (!is_void).then(|| destination.clone());

        self.push_call(
            Call::builder()
                .maybe_destination(call_destination)
                .callee(callee)
                .arguments(arguments)
                .handler_records(handler_records)
                .build(),
        );

        // if we are calling a `void`  function, we need to assign "fake" unit
        // value to the destination place. (Actually, we don't need to
        // assign anything, since unit type has only one value, and we
        // can just use uninitialized value)
        if is_void {
            self.store_constant(destination, Constant::Unit);
        }
    }

    pub(crate) fn set_terminator(&mut self, terminator: Terminator) {
        self.output.set_terminator(self.block, terminator);
    }

    /// Ends the selected block with a switch on `value`, whose values other
    /// than those of `targets` never happen.
    pub(crate) fn set_switch_terminator(
        &mut self,
        value: Operand,
        targets: Vec<(Constant, BlockID)>,
    ) {
        let otherwise = self.in_new_block(|emit| emit.set_terminator(Terminator::Unreachable));
        self.set_terminator(Terminator::Switch(Switch::new(value, targets, otherwise)));
    }

    /// Creates a block and fills it with `emit`, which should terminate it. The
    /// current block stays selected afterwards.
    pub(crate) fn in_new_block(&mut self, emit: impl FnOnce(&mut Self)) -> BlockID {
        let block = self.create_block();
        self.in_block(block, emit);

        block
    }

    /// Fills `block` with `emit`, and returns what it returns. The current
    /// block stays selected afterwards.
    pub(crate) fn in_block<R>(&mut self, block: BlockID, emit: impl FnOnce(&mut Self) -> R) -> R {
        let previous = self.block;
        self.block = block;
        let result = emit(self);
        self.block = previous;

        result
    }

    /// The block that instructions go to.
    pub(crate) const fn selected_block(&self) -> BlockID { self.block }

    /// Fills the resume entry of `point` with `emit`, which should terminate
    /// it. The dispatch of `coroutine_resume` enters that block when the frame
    /// is suspended at `point`. The current block stays selected afterwards.
    pub(crate) fn fill_resume_entry(
        &mut self,
        point: SuspensionPointID,
        ctx: &Context<'_>,
        emit: impl FnOnce(&mut Self),
    ) {
        let block = ctx.coroutine_form().claim_resume_entry(point);
        self.in_block(block, emit);
    }
}
