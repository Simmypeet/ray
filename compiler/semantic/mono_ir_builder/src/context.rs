//! The read-only context of lowering one semantic IR function into one
//! `MonoFunction`. The `lower` module lowers on the [`Emitter`] it is
//! emitted with, passing this context along.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{
    cfg::{
        BlockID as IRBlockID, Instruction as IRInstruction, Reachables, Terminator as IRTerminator,
    },
    ir_closure::{CaptureID, CaptureMapID, ClosureParameterID},
    ir_expr::{IRExpr, IRExprID, handle::Handle},
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    MonoBodyInstance, MonoCoroutineInstance, MonoEffectInstance, MonoEnvironmentInstance,
    MonoRunWithInstance,
    cfg::BlockID,
    layout::{EnvironmentLayout, build_environment_layout},
    operand::Operand,
    place::Place,
    ty::{FunctionSignature, MonoType},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_symbol::{GlobalSymbolID, parent::get_parent_global};
use rayc_type::subst::Subst;

use crate::{emitter::Emitter, resolver::Resolver};

pub(crate) mod lowering_style;
pub(crate) mod source_map;

use lowering_style::{CoroutineForm, Frame, LoweringStyle};
use source_map::SourceMap;

/// What lowering one function of a definition's IR reads, which stays the
/// same while it is lowered: the function and the instantiation it is lowered
/// in, the IR it is part of, where the values and blocks of the source went,
/// and how it performs its effects.
///
/// Every other function of the IR is lowered on its own, so the context only
/// refers to them, through the signature and environment layout queries.
pub(crate) struct Context<'a> {
    /// Dictionary and callee resolution in the instantiation of `body`.
    resolver: &'a Resolver,
    /// The function being lowered, and the instantiation its values are
    /// lowered in.
    body: MonoBodyInstance,
    /// The IR of the definition that `body` is a function of.
    source: Interned<IRFunctionMap>,
    map: SourceMap,
    style: LoweringStyle,
}

impl<'a> Context<'a> {
    /// The context of `body`, a function of `source`, performing its effects
    /// through handlers. `emit` creates its parameters, values and blocks.
    pub(crate) async fn new_callback(
        resolver: &'a Resolver,
        body: MonoBodyInstance,
        source: Interned<IRFunctionMap>,
        emit: &mut Emitter<'_>,
    ) -> Self {
        let environment = match source.try_get_capture_map_id(body.ir_function_id()) {
            Some(capture_map) => {
                Some(environment_layout(resolver.engine(), &body, capture_map).await)
            }
            None => None,
        };
        let (map, environment_place) =
            SourceMap::new_callback(&source, &body, emit, environment.as_ref()).await;
        let style =
            LoweringStyle::new_callback(emit, environment.as_ref().zip(environment_place.as_ref()));

        Self { resolver, body, source, map, style }
    }

    /// The context of the function that `instance` suspends, lowered into the
    /// `coroutine_resume` of `instance`. `emit` creates its values and blocks.
    pub(crate) async fn new_coroutine(
        resolver: &'a Resolver,
        instance: &MonoCoroutineInstance,
        source: Interned<IRFunctionMap>,
        emit: &mut Emitter<'_>,
    ) -> Self {
        let body = instance.body().clone();
        let function = source.get_function(body.ir_function_id());
        let style = LoweringStyle::new_coroutine(function, emit, instance).await;
        let map = SourceMap::new_coroutine(&source, &body, emit, style.frame()).await;

        Self { resolver, body, source, map, style }
    }

    /// The context of the function that `instance` suspends, lowered into the
    /// `coroutine_drop` of `instance`. `emit` creates its blocks.
    ///
    /// Only the abandon block of each suspension point is lowered, so only
    /// those blocks and the values held in the frame are mapped.
    pub(crate) async fn new_coroutine_drop(
        resolver: &'a Resolver,
        instance: &MonoCoroutineInstance,
        source: Interned<IRFunctionMap>,
        emit: &mut Emitter<'_>,
    ) -> Self {
        let body = instance.body().clone();
        let function = source.get_function(body.ir_function_id());
        let style = LoweringStyle::new_coroutine_drop(emit, instance).await;
        let frame = style.frame();
        let abandon_blocks = frame.suspension_points().map(|point| abandon_block(function, point));
        let map = SourceMap::new_coroutine_drop(emit, frame, abandon_blocks);

        Self { resolver, body, source, map, style }
    }

    const fn engine(&self) -> &TrackedEngine { self.resolver.engine() }

    /// The function being lowered.
    fn source_function(&self) -> &IRFunction {
        self.source.get_function(self.body.ir_function_id())
    }

    /// The function `function_id` of the same IR, in the same instantiation,
    /// such as a closure or the body of a `run ... with` that this function
    /// creates.
    pub(crate) fn sibling_body(&self, function_id: IRFunctionID) -> MonoBodyInstance {
        self.body.with_ir_function(function_id)
    }

    /// The `run ... with` expression `expression` of the function being
    /// lowered.
    pub(crate) fn run_with_instance(&self, expression: IRExprID) -> MonoRunWithInstance {
        MonoRunWithInstance::new(self.body.clone(), expression)
    }

    /// The calling signature of the function `function_id` of the same IR.
    pub(crate) async fn sibling_signature(&self, function_id: IRFunctionID) -> FunctionSignature {
        self.resolver.callback_signature(&self.sibling_body(function_id)).await
    }

    /// The layout of the environment of the function `function_id` of the same
    /// IR, which must have captures.
    pub(crate) async fn function_environment_layout(
        &self,
        function_id: IRFunctionID,
    ) -> Interned<EnvironmentLayout> {
        self.capture_environment_layout(self.source.get_capture_map_id(function_id)).await
    }

    /// The layout of the environment that the functions of `capture_map_id`
    /// share.
    pub(crate) async fn capture_environment_layout(
        &self,
        capture_map_id: CaptureMapID,
    ) -> Interned<EnvironmentLayout> {
        environment_layout(self.engine(), &self.body, capture_map_id).await
    }

    pub(crate) fn source_expression(&self, expression: IRExprID) -> &IRExpr {
        self.source_function().get_expression(expression)
    }

    pub(crate) fn source_entry_block(&self) -> IRBlockID { self.source_function().entry_block() }

    pub(crate) fn source_direct_reachables(&self) -> Reachables {
        self.source_function().direct_reachables()
    }

    pub(crate) fn source_block_instructions(&self, block: IRBlockID) -> &[IRInstruction] {
        self.source_function().block_instructions(block)
    }

    pub(crate) fn source_block_terminator(&self, block: IRBlockID) -> Option<&IRTerminator> {
        self.source_function().block_terminator(block)
    }

    /// The block that the function takes when it is dropped while it is
    /// suspended at `point`.
    pub(crate) fn source_abandon_block(&self, point: SuspensionPointID) -> IRBlockID {
        abandon_block(self.source_function(), point)
    }

    /// The expression that the function evaluates at `point`, which waits for
    /// what the function holds while it is suspended there.
    pub(crate) fn suspending_expression(&self, point: SuspensionPointID) -> IRExprID {
        self.source_block_terminator(point.block_id())
            .and_then(IRTerminator::suspending_expr)
            .expect("a suspension point evaluates an expression")
    }

    /// Asserts that the function `function_id` of the same IR uses the
    /// environment of `capture_map_id`.
    pub(crate) fn assert_function_uses_capture_environment(
        &self,
        function_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) {
        assert_eq!(
            self.source.try_get_capture_map_id(function_id),
            Some(capture_map_id),
            "operation handlers in one handler record should share an environment"
        );
    }

    pub(crate) fn apply_owner_substitution(&self, substitution: &mut Subst) {
        substitution.compose(self.body.substitution(), self.engine());
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.resolver.instantiate_effect(effect_id, substitution, self.body.substitution()).await
    }

    /// The effect of `operation_id`, one of the effects that `handle` handles,
    /// as an effect row of the definition would spell it.
    pub(crate) async fn lower_handled_effect(
        &self,
        handle: &Handle,
        operation_id: GlobalSymbolID,
    ) -> MonoEffectInstance {
        let effect_id = self
            .engine()
            .get_parent_global(operation_id)
            .await
            .expect("an operation should belong to an effect");

        self.instantiate_effect(effect_id, handle.handled_effect(effect_id).substitution()).await
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

    /// The frame of the coroutine that the function works on.
    pub(crate) const fn frame(&self) -> &Frame { self.style.frame() }

    /// The place of the coroutine that the function waits for while it is
    /// suspended at `point`.
    pub(crate) fn awaitee_place(&self, point: SuspensionPointID) -> Place {
        self.frame().awaitee_place(point, self.engine())
    }

    pub(crate) fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        self.style.suspension_point(block)
    }
}

/// The abandon block of `point`, a suspension point of `function`.
fn abandon_block(function: &IRFunction, point: SuspensionPointID) -> IRBlockID {
    function
        .block_terminator(point.block_id())
        .and_then(IRTerminator::abandon_block)
        .expect("a suspension point has an abandon block")
}

/// The layout of the environment of `capture_map`, a capture map of the IR that
/// `body` is a function of, in the instantiation of `body`.
async fn environment_layout(
    engine: &TrackedEngine,
    body: &MonoBodyInstance,
    capture_map: CaptureMapID,
) -> Interned<EnvironmentLayout> {
    engine.build_environment_layout(MonoEnvironmentInstance::new(body, capture_map)).await
}
