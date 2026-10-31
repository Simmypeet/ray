//! The state of lowering one semantic IR function into one `MonoIR` function.
//! The `lower` module does the lowering on top of it, through the methods here.

use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::{
        BlockID as IRBlockID, Instruction as IRInstruction, Reachables, Terminator as IRTerminator,
    },
    ir_closure::{CaptureID, CaptureMapID, ClosureParameterID},
    ir_expr::{IRExpr, IRExprID, handle::Handle},
    ir_function::{IRFunction, IRFunctionID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoDefInstance, MonoEffectInstance, MonoGeneratedAdtDropInstance,
    MonoIR,
    cfg::{BlockID, Terminator},
    function::{LocalID, MonoFunctionID},
    operand::{Constant, Operand},
    place::{FieldIndex, Place, PlaceProjector},
    rvalue::Rvalue,
    signature::CoroutineSignature,
    ty::MonoType,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{subst::Subst, ty::Ty};

use crate::{
    context::{CallbackStyleFunctionABI, Context, EnvironmentAbi},
    resolver::Resolver,
};

pub(crate) mod emitter;
pub(crate) mod lowering_style;
pub(crate) mod source_map;

use emitter::Emitter;
use lowering_style::{CoroutineForm, LoweringStyle};
use source_map::SourceMap;

/// The state of lowering one function.
pub(crate) struct FunctionLowering<'a> {
    context: &'a Context,
    source: &'a IRFunction,
    emit: Emitter<'a>,
    map: SourceMap,
    style: LoweringStyle,
}

impl<'a> FunctionLowering<'a> {
    /// Starts lowering a function that performs its effects through handlers.
    pub(crate) async fn new_callback(
        context: &'a Context,
        output: &'a mut MonoIR,
        target_id: MonoFunctionID,
        source_id: IRFunctionID,
    ) -> Self {
        let mut emit = Emitter::new(context.resolver(), output, target_id);
        let (map, environment) = SourceMap::new_callback(context, source_id, &mut emit).await;
        let style = LoweringStyle::new_callback(context, source_id, &emit, environment.as_ref());

        Self { context, source: context.source_function(source_id), emit, map, style }
    }

    /// Starts lowering a function into the `coroutine_resume` of `instance`.
    pub(crate) async fn new_coroutine(
        context: &'a Context,
        output: &'a mut MonoIR,
        target_id: MonoFunctionID,
        source_id: IRFunctionID,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        let mut emit = Emitter::new(context.resolver(), output, target_id);
        let style = LoweringStyle::new_coroutine(context, source_id, &mut emit, instance).await;
        let map =
            SourceMap::new_coroutine(context, source_id, &mut emit, style.coroutine_form()).await;

        Self { context, source: context.source_function(source_id), emit, map, style }
    }

    pub(crate) const fn resolver(&self) -> &'a Resolver { self.context.resolver() }

    pub(crate) const fn engine(&self) -> &'a TrackedEngine { self.emit.engine() }

    pub(crate) fn function_abi(&self, function_id: IRFunctionID) -> &'a CallbackStyleFunctionABI {
        self.context.function_abi(function_id)
    }

    pub(crate) fn function_environment_abi(&self, function_id: IRFunctionID) -> &'a EnvironmentAbi {
        self.context.function_environment_abi(function_id)
    }

    pub(crate) fn capture_environment_abi(
        &self,
        capture_map_id: CaptureMapID,
    ) -> &'a EnvironmentAbi {
        self.context.capture_environment_abi(capture_map_id)
    }

    pub(crate) fn source_expression(&self, expression: IRExprID) -> &'a IRExpr {
        self.source.get_expression(expression)
    }

    pub(crate) const fn source_entry_block(&self) -> IRBlockID { self.source.entry_block() }

    pub(crate) fn source_direct_reachables(&self) -> Reachables { self.source.direct_reachables() }

    pub(crate) fn source_block_instructions(&self, block: IRBlockID) -> &'a [IRInstruction] {
        self.source.block_instructions(block)
    }

    pub(crate) fn source_block_terminator(&self, block: IRBlockID) -> Option<&'a IRTerminator> {
        self.source.block_terminator(block)
    }

    /// Fills a new block with `emit`, which should terminate it. The current
    /// block stays selected afterwards.
    pub(crate) fn in_new_block(&mut self, emit: impl FnOnce(&mut Emitter<'a>)) -> BlockID {
        self.emit.in_new_block(emit)
    }

    /// Fills `block` with `emit`, which should terminate it. The current block
    /// stays selected afterwards.
    pub(crate) fn in_block(&mut self, block: BlockID, emit: impl FnOnce(&mut Emitter<'a>)) {
        self.emit.in_block(block, emit);
    }

    /// Starts a projection of `place`.
    pub(crate) const fn project<'p>(&self, place: &'p Place) -> PlaceProjector<'p>
    where
        'a: 'p,
    {
        self.emit.project(place)
    }
}

impl FunctionLowering<'_> {
    pub(crate) fn assert_function_uses_capture_environment(
        &self,
        function_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) {
        self.context.assert_function_uses_capture_environment(function_id, capture_map_id);
    }

    pub(crate) fn target_function_id(&self, function_id: IRFunctionID) -> MonoFunctionID {
        self.context.target_function_id(function_id)
    }

    pub(crate) fn apply_owner_substitution(&self, substitution: &mut Subst) {
        self.context.apply_owner_substitution(substitution);
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.context.instantiate_effect(effect_id, substitution).await
    }

    pub(crate) async fn lower_handled_effect(
        &self,
        handle: &Handle,
        operation_id: GlobalSymbolID,
    ) -> MonoEffectInstance {
        self.context.lower_handled_effect(handle, operation_id).await
    }

    pub(crate) fn entry_block(&self) -> BlockID { self.emit.entry_block() }

    pub(crate) const fn select_block(&mut self, block: BlockID) { self.emit.select_block(block); }

    pub(crate) fn place(&self, local: LocalID) -> Place { self.emit.place(local) }

    pub(crate) fn temporary(&mut self, ty: Interned<MonoType>) -> Place {
        self.emit.new_temporary_place(ty)
    }

    pub(crate) fn temporary_local(&mut self, ty: Interned<MonoType>) -> LocalID {
        self.emit.new_temporary_local(ty)
    }

    pub(crate) fn progress_temporary(&mut self, signature: &CoroutineSignature) -> Place {
        self.emit.new_progress_temporary_place(signature)
    }

    pub(crate) fn scratch_unit(&mut self) -> Place { self.emit.new_unit_place() }

    pub(crate) fn assign(&mut self, destination: Place, value: Rvalue) {
        self.emit.assign(destination, value);
    }

    pub(crate) fn store(&mut self, destination: Place, operand: Operand) {
        self.emit.store(destination, operand);
    }

    pub(crate) fn store_constant(&mut self, destination: Place, constant: Constant) {
        self.emit.store_constant(destination, constant);
    }

    pub(crate) fn call(
        &mut self,
        destination: Option<Place>,
        callee: Operand,
        arguments: Vec<Operand>,
    ) {
        self.emit.call(destination, callee, arguments);
    }

    pub(crate) fn set_terminator(&mut self, terminator: Terminator) {
        self.emit.set_terminator(terminator);
    }

    /// Drops `value` with the implementation selected by `dictionary`, passing
    /// on this function's handlers.
    pub(crate) async fn lower_drop(
        &mut self,
        value: Place,
        dictionary: &Interned<Ty>,
        destination: Place,
    ) {
        self.emit.lower_drop(value, dictionary, destination).await;
    }

    /// Expands a built-in tuple `Drop` call, passing on this function's
    /// handlers.
    pub(crate) async fn lower_tuple_drop(
        &mut self,
        tuple: &Place,
        element_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        self.emit.lower_tuple_drop(tuple, element_instances, destination).await;
    }

    /// Expands a built-in closure `Drop` call, passing on this function's
    /// handlers.
    pub(crate) async fn lower_closure_drop(
        &mut self,
        closure: &Place,
        capture_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        self.emit.lower_closure_drop(closure, capture_instances, destination).await;
    }

    pub(crate) async fn lower_call_with_args(
        &mut self,
        callee: MonoDefInstance,
        arguments: Vec<Operand>,
        destination: Place,
    ) {
        self.emit.lower_call_with_args(callee, arguments, destination).await;
    }

    /// Calls a generated nominal Drop fragment.
    pub(crate) fn lower_nominal_drop_call(
        &mut self,
        instance: MonoGeneratedAdtDropInstance,
        arguments: Vec<Operand>,
        destination: Place,
    ) {
        self.emit.lower_nominal_drop_call(instance, arguments, destination);
    }

    pub(crate) fn block(&self, block: IRBlockID) -> BlockID { self.map.block(block) }

    pub(crate) fn expression_place(&self, expression: IRExprID) -> Place {
        self.map.expression_place(expression)
    }

    pub(crate) fn expression_type(&self, expression: IRExprID) -> &Interned<MonoType> {
        self.map.expression_type(expression)
    }

    pub(crate) fn expression_operand(&self, expression: IRExprID) -> Operand {
        self.map.expression_operand(expression)
    }

    pub(crate) fn variable_place(&self, variable: IRVariableID) -> Place {
        self.map.variable_place(variable)
    }

    pub(crate) fn parameter_place(&self, parameter: ParameterID) -> Place {
        self.map.parameter_place(parameter)
    }

    pub(crate) fn closure_parameter_place(&self, parameter: ClosureParameterID) -> Place {
        self.map.closure_parameter_place(parameter)
    }

    pub(crate) fn operation_parameter_place(
        &self,
        parameter: OperationHandlerParameterID,
    ) -> Place {
        self.map.operation_parameter_place(parameter)
    }

    pub(crate) fn capture_place(&self, capture: CaptureID) -> Place {
        self.map.capture_place(capture)
    }

    pub(crate) const fn is_coroutine(&self) -> bool { self.style.is_coroutine() }

    pub(crate) fn handler_place(&self, effect: &MonoEffectInstance) -> Place {
        self.style.handler_place(effect)
    }

    pub(crate) fn get_effect_instance_handler_operand(
        &self,
        effect: &MonoEffectInstance,
    ) -> Operand {
        self.style.get_effect_instance_handler_operand(effect)
    }

    pub(crate) const fn coroutine_form(&self) -> &CoroutineForm { self.style.coroutine_form() }

    pub(crate) fn suspension_variant(&self, block: IRBlockID) -> Option<FieldIndex> {
        self.style.suspension_variant(block)
    }
}
