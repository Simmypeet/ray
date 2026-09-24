use std::mem;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load, tuple::Tuple},
    ir_function::{FunctionID as IrFunctionID, IRFunctionMap},
    ir_lambda::{
        Capture, CaptureID, CaptureMapID, LambdaParameter as IrLambdaParameter, LambdaParameterID,
    },
    ir_operation_handler::{
        OperationHandlerParameter as IrOperationHandlerParameter,
        OperationHandlerParameterID as IrOperationHandlerParameterID,
    },
    ir_variable::IRVariableID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{capture::CaptureMode, ty::Ty};
use rayc_typed_ast::{
    capture_plan::FunctionCapturePlan, name_binding::Source, typed_function::TypedFunctionID,
    typed_lambda::LambdaParameterID as TypedLambdaParameterID,
    typed_operation_handler::OperationHandlerParameterID as TypedOperationHandlerParameterID,
    typed_variable::TypedVariableID,
};

use self::scope_tracker::ScopeTracker;
use super::Builder;
use crate::{
    builder::function_build_state::scope_tracker::ScopeKind,
    context::LoweringContext,
    diagnostic::{Diagnostic, NotAllPathsReturnValue},
    statement::LoopTarget,
};

pub mod scope_tracker;

pub(super) struct FunctionBuildState {
    ir_function_id: IrFunctionID,
    current_block: BlockID,
    scopes: ScopeTracker,
    typed_function_id: TypedFunctionID,
    return_ty: Interned<Ty>,
    diagnostic_span: Option<RelativeSpan>,

    variables: FxHashMap<TypedVariableID, IRVariableID>,
    lambda_parameters: FxHashMap<TypedLambdaParameterID, LambdaParameterID>,
    operation_handler_parameters:
        FxHashMap<TypedOperationHandlerParameterID, IrOperationHandlerParameterID>,

    captures: FxHashMap<Source, CaptureID>,
    loop_targets: Vec<LoopTarget>,
}

impl FunctionBuildState {
    fn new_def(
        context: &LoweringContext<'_>,
        ir_functions: &mut IRFunctionMap,
        return_ty: Interned<Ty>,
        diagnostic_span: Option<RelativeSpan>,
    ) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let _def_context = context.typed_function_context().assert_as_def_context();
        assert_eq!(
            typed_function_id,
            context.root_typed_function_id(),
            "only the root TypedAST function should be a def"
        );
        assert_eq!(
            capture_plan.captures().len(),
            0,
            "root TypedAST function should not capture a source"
        );
        let ir_function_id = ir_functions.root_id();
        let current_block = ir_functions.entry_block(ir_function_id);
        let scopes = Self::initialize_scopes(ir_functions, ir_function_id, current_block);
        Self {
            ir_function_id,
            current_block,
            scopes,
            typed_function_id,
            return_ty,
            diagnostic_span,
            variables: FxHashMap::default(),
            lambda_parameters: FxHashMap::default(),
            operation_handler_parameters: FxHashMap::default(),
            captures: FxHashMap::default(),
            loop_targets: Vec::new(),
        }
    }

    fn new_lambda(
        context: &LoweringContext<'_>,
        ir_functions: &mut IRFunctionMap,
        return_ty: Interned<Ty>,
        diagnostic_span: RelativeSpan,
    ) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let lambda_context = context.typed_function_context().assert_as_lambda_context();
        assert_ne!(
            typed_function_id,
            context.root_typed_function_id(),
            "root TypedAST function should not be a lambda"
        );
        let mut lambda_parameters = FxHashMap::default();
        let (capture_map, captures) = Self::insert_capture_map(capture_plan, ir_functions);
        let ir_function_id = ir_functions.insert_lambda(
            return_ty.clone(),
            context.function_effect().clone(),
            capture_map,
        );
        for (typed_id, parameter) in lambda_context.parameters() {
            let ir_id = ir_functions.insert_lambda_parameter(
                ir_function_id,
                IrLambdaParameter::new(parameter.ty().clone(), parameter.span()),
            );
            assert!(lambda_parameters.insert(typed_id, ir_id).is_none());
        }
        let current_block = ir_functions.entry_block(ir_function_id);
        let scopes = Self::initialize_scopes(ir_functions, ir_function_id, current_block);
        Self {
            ir_function_id,
            current_block,
            scopes,
            typed_function_id,
            return_ty,
            diagnostic_span: Some(diagnostic_span),
            variables: FxHashMap::default(),
            lambda_parameters,
            operation_handler_parameters: FxHashMap::default(),
            captures,
            loop_targets: Vec::new(),
        }
    }

    fn new_thunk(
        context: &LoweringContext<'_>,
        ir_functions: &mut IRFunctionMap,
        diagnostic_span: RelativeSpan,
    ) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let thunk_context = context.typed_function_context().assert_as_thunk_context();
        let return_ty = thunk_context.return_type().clone();
        let (capture_map, captures) = Self::insert_capture_map(capture_plan, ir_functions);
        let ir_function_id = ir_functions.insert_thunk(
            return_ty.clone(),
            context.function_effect().clone(),
            capture_map,
        );
        let current_block = ir_functions.entry_block(ir_function_id);
        let scopes = Self::initialize_scopes(ir_functions, ir_function_id, current_block);
        Self {
            ir_function_id,
            current_block,
            scopes,
            typed_function_id,
            return_ty,
            diagnostic_span: Some(diagnostic_span),
            variables: FxHashMap::default(),
            lambda_parameters: FxHashMap::default(),
            operation_handler_parameters: FxHashMap::default(),
            captures,
            loop_targets: Vec::new(),
        }
    }

    fn new_operation_handler(
        context: &LoweringContext<'_>,
        ir_functions: &mut IRFunctionMap,
        diagnostic_span: RelativeSpan,
        capture_map: CaptureMapID,
        captures: FxHashMap<Source, CaptureID>,
    ) -> Self {
        let typed_function_id = context.typed_function_id();
        let handler_context =
            context.typed_function_context().assert_as_operation_handler_context();
        let return_ty = handler_context.return_type().clone();
        let ir_function_id = ir_functions.insert_operation_handler(
            handler_context.operation(),
            return_ty.clone(),
            context.function_effect().clone(),
            capture_map,
        );
        let mut operation_handler_parameters = FxHashMap::default();
        for (typed_id, parameter) in handler_context.parameters() {
            let ir_id = ir_functions.insert_operation_handler_parameter(
                ir_function_id,
                IrOperationHandlerParameter::new(parameter.ty().clone(), parameter.span()),
            );
            assert!(operation_handler_parameters.insert(typed_id, ir_id).is_none());
        }
        let current_block = ir_functions.entry_block(ir_function_id);
        let scopes = Self::initialize_scopes(ir_functions, ir_function_id, current_block);
        Self {
            ir_function_id,
            current_block,
            scopes,
            typed_function_id,
            return_ty,
            diagnostic_span: Some(diagnostic_span),
            variables: FxHashMap::default(),
            lambda_parameters: FxHashMap::default(),
            operation_handler_parameters,
            captures,
            loop_targets: Vec::new(),
        }
    }

    fn insert_capture_map(
        capture_plan: &FunctionCapturePlan,
        ir_functions: &mut IRFunctionMap,
    ) -> (CaptureMapID, FxHashMap<Source, CaptureID>) {
        let capture_map_id = ir_functions.new_capture_map();
        let mut captures = FxHashMap::default();

        for (_, requirement) in capture_plan.captures() {
            let ir_id = ir_functions.insert_capture(
                capture_map_id,
                Capture::new(
                    requirement.binding_ty().clone(),
                    requirement.mode(),
                    requirement.span(),
                ),
            );
            assert!(captures.insert(requirement.source(), ir_id).is_none());
        }

        (capture_map_id, captures)
    }

    fn initialize_scopes(
        ir_functions: &mut IRFunctionMap,
        function_id: IrFunctionID,
        entry_block: BlockID,
    ) -> ScopeTracker {
        let root = ir_functions.root_scope_id(function_id);
        ir_functions.push_scope_push_instruction(function_id, entry_block, root);
        ScopeTracker::new(root)
    }

    pub(super) fn push_loop_target(&mut self, target: LoopTarget) {
        self.loop_targets.push(target);
    }

    pub(super) fn pop_loop_target(&mut self) {
        self.loop_targets.pop().expect("a loop target should be active");
    }

    pub(super) fn current_loop_target(&self) -> Option<LoopTarget> {
        self.loop_targets.last().copied()
    }
}

impl Builder {
    pub(crate) fn enter_scope(&mut self, kind: ScopeKind) {
        let function_id = self.building_function.ir_function_id;
        let parent = self.building_function.scopes.current();
        let scope_id = self.ir_functions.insert_scope(function_id, parent);
        self.enter_existing_scope(scope_id, kind);
    }

    pub(crate) fn create_scope_branch(
        &mut self,
        branch_count: usize,
    ) -> Vec<rayc_ir::scope::ScopeID> {
        let function_id = self.building_function.ir_function_id;
        let parent = self.building_function.scopes.current();
        self.ir_functions.insert_scope_branch(function_id, parent, branch_count)
    }

    pub(crate) fn enter_existing_scope(
        &mut self,
        scope_id: rayc_ir::scope::ScopeID,
        kind: ScopeKind,
    ) {
        self.building_function.scopes.push(scope_id, kind);
        self.ir_functions.push_scope_push_instruction(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            scope_id,
        );
    }

    pub(crate) fn exit_scope(&mut self) {
        let scope_id = self.building_function.scopes.pop();
        if !self.is_terminated() {
            self.emit_scope_pop(scope_id);
        }
    }

    pub(crate) const fn scope_depth(&self) -> usize { self.building_function.scopes.depth() }

    fn emit_scope_pop(&mut self, scope_id: rayc_ir::scope::ScopeID) {
        self.ir_functions.push_scope_pop_instruction(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            scope_id,
        );
    }

    pub async fn new(
        engine: TrackedEngine,
        def_id: GlobalSymbolID,
        context: &LoweringContext<'_>,
        return_ty: Interned<Ty>,
        diagnostic_span: Option<RelativeSpan>,
    ) -> Self {
        let mut ir_functions = IRFunctionMap::new(context.function_effect().clone());
        let building_function =
            FunctionBuildState::new_def(context, &mut ir_functions, return_ty, diagnostic_span);

        Self {
            solver: Solver::new(engine.clone(), def_id).await,
            engine,
            ir_functions,
            building_function,
            suspended_functions: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub async fn lower(
        mut self,
        context: &LoweringContext<'_>,
    ) -> (IRFunctionMap, Vec<Diagnostic>) {
        self.lower_current_function(context).await;
        assert!(
            self.suspended_functions.is_empty(),
            "all suspended IR functions should be restored before finishing lowering"
        );
        assert_eq!(
            self.building_function.typed_function_id,
            context.root_typed_function_id(),
            "root IR function should be active when lowering finishes"
        );
        (self.ir_functions, self.diagnostics)
    }

    pub async fn lower_lambda_function(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
        return_ty: Interned<Ty>,
        diagnostic_span: RelativeSpan,
    ) -> IrFunctionID {
        let lambda_context = context.for_function(typed_function_id);
        self.start_lambda(&lambda_context, return_ty, diagnostic_span);
        self.lower_current_function(&lambda_context).await;
        self.finish_lambda()
    }

    pub async fn lower_thunk_function(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
        diagnostic_span: RelativeSpan,
    ) -> IrFunctionID {
        let thunk_context = context.for_function(typed_function_id);
        self.start_thunk(&thunk_context, diagnostic_span);
        self.lower_current_function(&thunk_context).await;
        self.finish_nested_function()
    }

    pub async fn lower_operation_handler_function(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
        diagnostic_span: RelativeSpan,
        capture_map: CaptureMapID,
        captures: FxHashMap<Source, CaptureID>,
    ) -> IrFunctionID {
        let handler_context = context.for_function(typed_function_id);
        self.start_operation_handler(&handler_context, diagnostic_span, capture_map, captures);
        self.lower_current_function(&handler_context).await;
        self.finish_nested_function()
    }

    pub fn lower_capture_map(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
    ) -> (CaptureMapID, FxHashMap<Source, CaptureID>) {
        FunctionBuildState::insert_capture_map(
            context.capture_plan(typed_function_id),
            &mut self.ir_functions,
        )
    }

    async fn lower_current_function(&mut self, context: &LoweringContext<'_>) {
        self.lower_statements(context).await;
        self.finish_current_function();
    }

    fn start_lambda(
        &mut self,
        context: &LoweringContext<'_>,
        return_ty: Interned<Ty>,
        diagnostic_span: RelativeSpan,
    ) {
        let lambda = FunctionBuildState::new_lambda(
            context,
            &mut self.ir_functions,
            return_ty,
            diagnostic_span,
        );
        let enclosing = mem::replace(&mut self.building_function, lambda);
        self.suspended_functions.push(enclosing);
    }

    fn start_thunk(&mut self, context: &LoweringContext<'_>, diagnostic_span: RelativeSpan) {
        let thunk = FunctionBuildState::new_thunk(context, &mut self.ir_functions, diagnostic_span);
        let enclosing = mem::replace(&mut self.building_function, thunk);
        self.suspended_functions.push(enclosing);
    }

    fn start_operation_handler(
        &mut self,
        context: &LoweringContext<'_>,
        diagnostic_span: RelativeSpan,
        capture_map: CaptureMapID,
        captures: FxHashMap<Source, CaptureID>,
    ) {
        let handler = FunctionBuildState::new_operation_handler(
            context,
            &mut self.ir_functions,
            diagnostic_span,
            capture_map,
            captures,
        );
        let enclosing = mem::replace(&mut self.building_function, handler);
        self.suspended_functions.push(enclosing);
    }

    fn finish_lambda(&mut self) -> IrFunctionID { self.finish_nested_function() }

    fn finish_nested_function(&mut self) -> IrFunctionID {
        let enclosing = self
            .suspended_functions
            .pop()
            .expect("a lambda should suspend its enclosing IR function");
        let lambda = mem::replace(&mut self.building_function, enclosing);
        lambda.ir_function_id
    }

    pub fn lower_capture_operands(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
    ) -> Vec<IRExprID> {
        context
            .capture_plan(typed_function_id)
            .captures()
            .map(|(_, requirement)| {
                let address = self.source_address(requirement.source());
                let (kind, ty) = match requirement.mode() {
                    // A capture moved inside the nested function must leave
                    // the enclosing function even when it is `Copy`.
                    CaptureMode::Value(kind) => (
                        IRExprKind::Load(Load::with_kind(address, kind.into())),
                        requirement.binding_ty().clone(),
                    ),
                    CaptureMode::Reference(mutability) => (
                        IRExprKind::RefOf(rayc_ir::ir_expr::ref_of::RefOf::new(address)),
                        self.pointer_ty(requirement.binding_ty().clone(), mutability),
                    ),
                };
                self.emit_expression(IRExpr::new(kind, requirement.span(), ty))
            })
            .collect()
    }

    fn finish_current_function(&mut self) {
        let function_id = self.building_function.ir_function_id;

        // Functions with only explicit returns have already unwound their root scope.
        if self.ir_functions.get_function(function_id).unterminated_blocks().next().is_none() {
            return;
        }

        // if the function requires a return value (non-unit type)
        if !self.building_function.return_ty.is_unit_type()
            && let Some(span) = self.building_function.diagnostic_span
        {
            self.diagnostics.push(NotAllPathsReturnValue::builder().span(span).build().into());
        }

        // Every implicit return closes the function root scope first.
        let root_scope = self.ir_functions.root_scope_id(function_id);
        let unterminated: Vec<_> =
            self.ir_functions.get_function(function_id).unterminated_blocks().collect();
        for block_id in unterminated {
            self.ir_functions.push_scope_pop_instruction(function_id, block_id, root_scope);
        }
        self.ir_functions.fill_return_on_unterminated_blocks(function_id);
    }

    pub fn emit_expression(&mut self, expression: IRExpr) -> IRExprID {
        let function_id = self.building_function.ir_function_id;
        let expression_id = self.ir_functions.insert_expression(function_id, expression);
        self.ir_functions.push_expression(
            function_id,
            self.building_function.current_block,
            expression_id,
        );
        expression_id
    }

    /// Emits the unit value `()` of type `ty`.
    pub(crate) fn emit_unit(&mut self, span: RelativeSpan, ty: Interned<Ty>) -> IRExprID {
        self.emit_expression(IRExpr::new(IRExprKind::Tuple(Tuple::new(Vec::new())), span, ty))
    }

    pub fn emit_expr_discard(&mut self, expression: IRExprID, drop_instance: Interned<Ty>) {
        self.ir_functions.push_expr_discard(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            expression,
            drop_instance,
        );
    }

    pub fn emit_store(&mut self, address: Address, value: IRExprID, span: RelativeSpan) {
        self.ir_functions.push_store(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            address,
            value,
            span,
        );
    }

    pub fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> IRVariableID {
        let function_id = self.building_function.ir_function_id;
        let scope_id = self.building_function.scopes.current();
        self.ir_functions.create_variable_in_scope(function_id, scope_id, ty, span)
    }

    pub fn register_source_variable(
        &mut self,
        context: &LoweringContext<'_>,
        typed_id: TypedVariableID,
    ) -> IRVariableID {
        let variable = context.variable(typed_id);
        let function_id = self.building_function.ir_function_id;
        let scope_id = self.building_function.scopes.current_lexical();
        let ir_id = self.ir_functions.create_variable_in_scope(
            function_id,
            scope_id,
            variable.ty().clone(),
            variable.span(),
        );
        self.building_function.variables.insert(typed_id, ir_id);
        ir_id
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.ir_functions.set_terminator(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            terminator,
        );
    }

    /// Adds a jump to the specified target block and returns the current block
    /// as the predecessor of the jump.
    pub fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.building_function.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub fn is_terminated(&self) -> bool {
        self.ir_functions
            .block_terminator(
                self.building_function.ir_function_id,
                self.building_function.current_block,
            )
            .is_some()
    }

    pub fn create_block(&mut self) -> BlockID {
        self.ir_functions.create_block(self.building_function.ir_function_id)
    }

    pub const fn select_block(&mut self, block: BlockID) {
        self.building_function.current_block = block;
    }

    pub fn source_address(&mut self, source: Source) -> Address {
        if source.function_id() == self.building_function.typed_function_id {
            return match source {
                Source::Variable(id) => {
                    let variable_id = self
                        .building_function
                        .variables
                        .get(&id.local_id())
                        .copied()
                        .expect("local source variable should be registered before use");
                    self.variable_address(variable_id)
                }
                Source::Parameter(id) => self.parameter_address(id.local_id()),
                Source::LambdaParameter(id) => {
                    let parameter_id = self
                        .building_function
                        .lambda_parameters
                        .get(&id.local_id())
                        .copied()
                        .expect("lambda parameter should be registered before use");
                    self.lambda_parameter_address(parameter_id)
                }
                Source::OperationHandlerParameter(id) => {
                    let parameter_id = self
                        .building_function
                        .operation_handler_parameters
                        .get(&id.local_id())
                        .copied()
                        .expect("operation handler parameter should be registered before use");
                    self.operation_handler_parameter_address(parameter_id)
                }
            };
        }

        let capture_id = self
            .building_function
            .captures
            .get(&source)
            .copied()
            .expect("non-local source should have an analyzed capture");

        // Value captures are already stored directly in the environment, while
        // reference captures store a pointer to the captured place.
        let mut address = self.capture_address(capture_id);
        match self
            .ir_functions
            .get_capture(self.building_function.ir_function_id, capture_id)
            .mode()
        {
            CaptureMode::Value(_) => {}
            CaptureMode::Reference(_) => self.project_raw_deref(&mut address),
        }
        address
    }
}
