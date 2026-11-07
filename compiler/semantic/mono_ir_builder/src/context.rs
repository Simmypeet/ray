//! The read-only context of lowering one semantic IR function into one
//! `MonoIR` function. The `lower` module lowers on the [`Emitter`] it is
//! emitted with, passing this context along.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
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
    MonoCoroutineInstance, MonoEffectInstance, cfg::BlockID, function::MonoFunctionID,
    operand::Operand, place::Place, ty::MonoType,
};
use rayc_semantic_element::parameter::ParameterID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::{
    emitter::Emitter,
    fragment::{CallbackStyleFunctionABI, EnvironmentAbi, Fragment},
};

pub(crate) mod lowering_style;
pub(crate) mod source_map;

use lowering_style::{CoroutineForm, LoweringStyle};
use source_map::SourceMap;

/// What lowering one function reads, which stays the same while it is
/// lowered: the fragment it is part of, its source function, where the
/// values and blocks of the source went, and how it performs its effects.
pub(crate) struct Context<'a> {
    fragment: &'a Fragment,
    source: &'a IRFunction,
    map: SourceMap,
    style: LoweringStyle,
}

impl<'a> Context<'a> {
    /// The context of a function that performs its effects through handlers,
    /// whose parameters, values and blocks `emit` creates.
    pub(crate) async fn new_callback(
        fragment: &'a Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
    ) -> Self {
        let (map, environment) = SourceMap::new_callback(fragment, source_id, emit).await;
        let style = LoweringStyle::new_callback(fragment, source_id, emit, environment.as_ref());

        Self { fragment, source: fragment.source_function(source_id), map, style }
    }

    /// The context of a function lowered into the `coroutine_resume` of
    /// `instance`, whose values and blocks `emit` creates.
    pub(crate) async fn new_coroutine(
        fragment: &'a Fragment,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        let style = LoweringStyle::new_coroutine(fragment, source_id, emit, instance).await;
        let map = SourceMap::new_coroutine(fragment, source_id, emit, style.coroutine_form()).await;

        Self { fragment, source: fragment.source_function(source_id), map, style }
    }

    pub(crate) fn function_abi(&self, function_id: IRFunctionID) -> &'a CallbackStyleFunctionABI {
        self.fragment.function_abi(function_id)
    }

    pub(crate) fn function_environment_abi(&self, function_id: IRFunctionID) -> &'a EnvironmentAbi {
        self.fragment.function_environment_abi(function_id)
    }

    pub(crate) fn capture_environment_abi(
        &self,
        capture_map_id: CaptureMapID,
    ) -> &'a EnvironmentAbi {
        self.fragment.capture_environment_abi(capture_map_id)
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

    pub(crate) fn assert_function_uses_capture_environment(
        &self,
        function_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) {
        self.fragment.assert_function_uses_capture_environment(function_id, capture_map_id);
    }

    pub(crate) fn target_function_id(&self, function_id: IRFunctionID) -> MonoFunctionID {
        self.fragment.target_function_id(function_id)
    }

    pub(crate) fn apply_owner_substitution(&self, substitution: &mut Subst) {
        self.fragment.apply_owner_substitution(substitution);
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.fragment.instantiate_effect(effect_id, substitution).await
    }

    pub(crate) async fn lower_handled_effect(
        &self,
        handle: &Handle,
        operation_id: GlobalSymbolID,
    ) -> MonoEffectInstance {
        self.fragment.lower_handled_effect(handle, operation_id).await
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

    /// The place of the coroutine that the function waits for while it is
    /// suspended at `point`.
    pub(crate) fn awaitee_place(&self, point: SuspensionPointID) -> Place {
        self.coroutine_form().awaitee_place(point, self.fragment.engine())
    }

    pub(crate) fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        self.style.suspension_point(block)
    }
}
