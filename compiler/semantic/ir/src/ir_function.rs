use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::effect_row::get_effect_row;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Ty, application::ClosureID};

use crate::{
    address::Address,
    cfg::{
        BlockID, Cfg, ControlFlowEdge, Instruction, InstructionInsertion, Point, Reachables,
        Terminator,
    },
    dataflow::{DataflowProblem, DataflowSolution, solve},
    ir_expr::{IRExpr, IRExprID, IRExpressionMap},
    ir_lambda::{
        Capture, CaptureID, CaptureMap, CaptureMapID, CaptureMode, IRLambdaContext, IRThunkContext,
        LambdaParameter, LambdaParameterID,
    },
    ir_operation_handler::{
        IROperationHandlerContext, OperationHandlerParameter, OperationHandlerParameterID,
    },
    ir_variable::{IRVariable, IRVariableID, IRVariableMap},
    scope::{Scope, ScopeID, ScopeMap},
    visit::{
        ExprVisitor, TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitExpr,
        VisitType, VisitTypeMut, VisitTypeMutAsync,
    },
};

pub type FunctionID = ID<IRFunction>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunctionMap {
    /// The definition whose body this map lowers.
    def_id: GlobalSymbolID,
    functions: Arena<IRFunction>,
    /// Capture layouts referenced by nested-function contexts. Operation
    /// handlers belonging to one handler record share an entry.
    capture_maps: Arena<CaptureMap>,
    root: FunctionID,
    closures: FxHashMap<ClosureID, FunctionID>,
}

impl IRFunctionMap {
    #[must_use]
    pub fn new(def_id: GlobalSymbolID) -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(IRFunction::new_def());
        Self { def_id, functions, capture_maps: Arena::new(), root, closures: FxHashMap::default() }
    }

    /// Returns the effect row of a function.
    ///
    /// The definition function takes the effect row declared in the
    /// definition's signature, while a nested function stores its own.
    pub async fn effect_of(&self, function_id: FunctionID, engine: &TrackedEngine) -> Interned<Ty> {
        match self.get_function(function_id).context().nested_effect() {
            Some(effect) => effect.clone(),
            None => engine.get_effect_row(self.def_id).await,
        }
    }

    #[must_use]
    pub fn root_scope_id(&self, function_id: FunctionID) -> ScopeID {
        self.get_function(function_id).root_scope_id()
    }

    #[must_use]
    pub fn get_scope(&self, function_id: FunctionID, scope_id: ScopeID) -> &Scope {
        self.get_function(function_id).get_scope(scope_id)
    }

    #[must_use]
    pub fn insert_scope(&mut self, function_id: FunctionID, parent: ScopeID) -> ScopeID {
        self.get_function_mut(function_id).insert_scope(parent)
    }

    #[must_use]
    pub fn insert_scope_branch(
        &mut self,
        function_id: FunctionID,
        parent: ScopeID,
        branch_count: usize,
    ) -> Vec<ScopeID> {
        self.get_function_mut(function_id).insert_scope_branch(parent, branch_count)
    }

    /// Associates a source closure identity with its lowered local function.
    pub fn register_closure(&mut self, closure_id: ClosureID, function_id: FunctionID) {
        let _ = self.get_function(function_id).context().assert_as_lambda_context();
        assert!(self.closures.insert(closure_id, function_id).is_none());
    }

    #[must_use]
    pub fn closure_function(&self, closure_id: ClosureID) -> Option<FunctionID> {
        self.closures.get(&closure_id).copied()
    }

    /// Iterates over nominal closure identities and their lowered functions.
    #[must_use]
    pub fn closures(&self) -> impl ExactSizeIterator<Item = (ClosureID, FunctionID)> {
        self.closures.iter().map(|(closure_id, function_id)| (*closure_id, *function_id))
    }

    pub fn fill_return_on_unterminated_blocks(&mut self, function_id: FunctionID) {
        self.get_function_mut(function_id).cfg.fill_return_on_unterminated_blocks();
    }

    /// Returns the definition whose body this map lowers.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    #[must_use]
    pub const fn root_id(&self) -> FunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &IRFunction {
        self.functions.get(self.root).expect("Root IR function should exist")
    }

    #[must_use]
    pub fn get_function(&self, id: FunctionID) -> &IRFunction {
        self.functions.get(id).expect("IR function should exist")
    }

    fn get_function_mut(&mut self, id: FunctionID) -> &mut IRFunction {
        self.functions.get_mut(id).expect("IR function should exist")
    }

    /// Iterates over all IR functions belonging to a source def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (FunctionID, &IRFunction)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_lambda(
        &mut self,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> FunctionID {
        self.functions.insert(IRFunction::new_lambda(return_ty, effect, capture_map))
    }

    #[must_use]
    pub fn insert_thunk(
        &mut self,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> FunctionID {
        self.functions.insert(IRFunction::new_thunk(return_ty, effect, capture_map))
    }

    #[must_use]
    pub fn insert_operation_handler(
        &mut self,
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> FunctionID {
        self.functions.insert(IRFunction::new_operation_handler(
            operation,
            return_ty,
            effect,
            capture_map,
        ))
    }

    #[must_use]
    pub fn insert_lambda_parameter(
        &mut self,
        function_id: FunctionID,
        parameter: LambdaParameter,
    ) -> LambdaParameterID {
        self.get_function_mut(function_id).insert_lambda_parameter(parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        function_id: FunctionID,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.get_function_mut(function_id).insert_operation_handler_parameter(parameter)
    }

    #[must_use]
    pub fn new_capture_map(&mut self) -> CaptureMapID {
        self.capture_maps.insert(CaptureMap::default())
    }

    #[must_use]
    pub fn insert_capture(&mut self, capture_map_id: CaptureMapID, capture: Capture) -> CaptureID {
        self.capture_maps
            .get_mut(capture_map_id)
            .expect("IR capture map should exist")
            .insert_capture(capture)
    }

    #[must_use]
    pub fn get_capture(&self, function_id: FunctionID, capture_id: CaptureID) -> &Capture {
        self.capture_map(function_id).get_capture(capture_id)
    }

    #[must_use]
    pub fn captures(
        &self,
        function_id: FunctionID,
    ) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.capture_map(function_id).iter()
    }

    /// Returns the capture layout owned by a nested function context.
    ///
    /// Definition functions do not capture values and therefore return `None`.
    #[must_use]
    pub fn captures_for_function(&self, function_id: FunctionID) -> Option<&CaptureMap> {
        let function = self.get_function(function_id);
        match function.context() {
            IRContext::Def => None,
            IRContext::Lambda(_) | IRContext::Thunk(_) | IRContext::OperationHandler(_) => {
                Some(self.capture_map(function_id))
            }
        }
    }

    /// Returns the arena-owned capture layout referenced by a nested function.
    #[must_use]
    pub fn capture_map_id(&self, function_id: FunctionID) -> CaptureMapID {
        self.get_function(function_id).context().capture_map()
    }

    /// Returns the capture layout of a nested function, or `None` for the
    /// definition function, which does not capture values.
    #[must_use]
    pub fn nested_capture_map_id(&self, function_id: FunctionID) -> Option<CaptureMapID> {
        self.get_function(function_id).context().nested_capture_map()
    }

    /// Visits every type stored in this map, together with where it is
    /// stored.
    ///
    /// The order is the same as that of [`Self::visit_types_mut`].
    pub fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter() {
            capture_map.visit_types(TypeSite::Capture(capture_map_id), visitor);
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions() {
            function.visit_types(function_id, visitor);
        }
    }

    /// Visits every type stored in this map mutably, together with where it
    /// is stored.
    ///
    /// Every capture layout is visited before any function, so a visitor
    /// sees the captures of a nested function before its signature. The
    /// types of one function's signature are visited together, before the
    /// types of its body. Otherwise, the iteration order is not stable.
    pub fn visit_types_mut<V: TypeVisitorMut>(&mut self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter_mut() {
            capture_map.visit_types_mut(TypeSite::Capture(capture_map_id), visitor);
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions.iter_mut() {
            function.visit_types_mut(function_id, visitor);
        }
    }

    /// Like [`Self::visit_types_mut`], but with a visitor that may await,
    /// for example to query the engine. The order is the same.
    pub async fn visit_types_mut_async<V: TypeVisitorMutAsync>(&mut self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter_mut() {
            capture_map.visit_types_mut_async(TypeSite::Capture(capture_map_id), visitor).await;
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions.iter_mut() {
            function.visit_types_mut_async(function_id, visitor).await;
        }
    }

    fn capture_map(&self, function_id: FunctionID) -> &CaptureMap {
        let capture_map = self.capture_map_id(function_id);
        self.capture_maps.get(capture_map).expect("IR capture map should exist")
    }

    #[must_use]
    pub fn entry_block(&self, function_id: FunctionID) -> BlockID {
        self.get_function(function_id).entry_block()
    }

    #[must_use]
    pub fn create_block(&mut self, function_id: FunctionID) -> BlockID {
        self.get_function_mut(function_id).create_block()
    }

    #[must_use]
    pub fn insert_expression(&mut self, function_id: FunctionID, expression: IRExpr) -> IRExprID {
        self.get_function_mut(function_id).insert_expression(expression)
    }

    #[must_use]
    pub fn create_variable_in_scope(
        &mut self,
        function_id: FunctionID,
        scope_id: ScopeID,
        ty: Interned<Ty>,
        span: RelativeSpan,
    ) -> IRVariableID {
        self.get_function_mut(function_id).create_variable_in_scope(scope_id, ty, span)
    }

    pub fn push_expression(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        expression: IRExprID,
    ) {
        self.get_function_mut(function_id).push_expression(block_id, expression);
    }

    pub fn push_expr_discard(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        expression: IRExprID,
        drop_instance: Interned<Ty>,
    ) {
        self.get_function_mut(function_id).push_expr_discard(block_id, expression, drop_instance);
    }

    pub fn push_scope_push_instruction(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        scope_id: ScopeID,
    ) {
        self.get_function_mut(function_id).push_scope_push_instruction(block_id, scope_id);
    }

    pub fn push_scope_pop_instruction(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        scope_id: ScopeID,
    ) {
        self.get_function_mut(function_id).push_scope_pop_instruction(block_id, scope_id);
    }

    pub fn push_store(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        address: Address,
        value: IRExprID,
        span: RelativeSpan,
    ) {
        self.get_function_mut(function_id).push_store(block_id, address, value, span);
    }

    /// Applies every instruction queued in `insertion` to `function_id`.
    pub fn insert_instructions(
        &mut self,
        function_id: FunctionID,
        insertion: InstructionInsertion,
    ) {
        self.get_function_mut(function_id).insert_instructions(insertion);
    }

    /// Redirects `edge` of `function_id` through a new empty block and
    /// returns that block, rewiring the phis of its target to take their
    /// value from the new block.
    pub fn split_edge(&mut self, function_id: FunctionID, edge: ControlFlowEdge) -> BlockID {
        self.get_function_mut(function_id).split_edge(edge)
    }

    pub fn set_terminator(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        terminator: Terminator,
    ) {
        self.get_function_mut(function_id).set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_terminator(
        &self,
        function_id: FunctionID,
        block_id: BlockID,
    ) -> Option<&Terminator> {
        self.get_function(function_id).block_terminator(block_id)
    }
}

impl VisitExpr for IRFunctionMap {
    fn visit_exprs<V: ExprVisitor>(&self, visitor: &mut V) {
        for (function_id, function) in self.functions() {
            for (expression_id, expression) in function.expression_map.expressions() {
                visitor.visit_expr(function_id, expression_id, expression);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum IRContext {
    Def,
    Lambda(IRLambdaContext),
    Thunk(IRThunkContext),
    OperationHandler(IROperationHandlerContext),
}

impl IRContext {
    #[track_caller]
    pub fn assert_as_def_context(&self) {
        match self {
            Self::Def => {}
            Self::Lambda(_) | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a def context, found a nested function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_lambda_context(&self) -> &IRLambdaContext {
        match self {
            Self::Def | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a lambda context, found a non-lambda context")
            }
            Self::Lambda(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_lambda_context_mut(&mut self) -> &mut IRLambdaContext {
        match self {
            Self::Def | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a lambda context, found a non-lambda context")
            }
            Self::Lambda(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_thunk_context(&self) -> &IRThunkContext {
        match self {
            Self::Thunk(context) => context,
            Self::Def | Self::Lambda(_) | Self::OperationHandler(_) => {
                panic!("expected a thunk context, found another function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_operation_handler_context(&self) -> &IROperationHandlerContext {
        match self {
            Self::OperationHandler(context) => context,
            Self::Def | Self::Lambda(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
        }
    }

    const fn capture_map(&self) -> CaptureMapID {
        self.nested_capture_map().expect("a def context does not have captures")
    }

    /// Returns the effect row stored by a nested function context, or `None`
    /// for a def context, whose effect row is declared by the definition.
    const fn nested_effect(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::Def => None,
            Self::Lambda(context) => Some(context.effect()),
            Self::Thunk(context) => Some(context.effect()),
            Self::OperationHandler(context) => Some(context.effect()),
        }
    }

    /// Returns the return type stored by a nested function context, or
    /// `None` for a def context, whose return type is declared by the
    /// definition.
    #[must_use]
    pub const fn nested_return_ty(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::Def => None,
            Self::Lambda(context) => Some(context.return_ty()),
            Self::Thunk(context) => Some(context.return_ty()),
            Self::OperationHandler(context) => Some(context.return_ty()),
        }
    }

    /// Returns the capture layout of a nested function context, or `None`
    /// for a def context.
    const fn nested_capture_map(&self) -> Option<CaptureMapID> {
        match self {
            Self::Def => None,
            Self::Lambda(context) => Some(context.capture_map()),
            Self::Thunk(context) => Some(context.capture_map()),
            Self::OperationHandler(context) => Some(context.capture_map()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunction {
    cfg: Cfg,
    scope_map: ScopeMap,
    variable_map: IRVariableMap,
    expression_map: IRExpressionMap,
    context: IRContext,
}

impl IRFunction {
    #[must_use]
    pub fn new_def() -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Def,
        }
    }

    #[must_use]
    pub fn new_lambda(
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Lambda(IRLambdaContext::new(return_ty, effect, capture_map)),
        }
    }

    #[must_use]
    pub fn new_thunk(
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Thunk(IRThunkContext::new(return_ty, effect, capture_map)),
        }
    }

    #[must_use]
    pub fn new_operation_handler(
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::OperationHandler(IROperationHandlerContext::new(
                operation,
                return_ty,
                effect,
                capture_map,
            )),
        }
    }

    #[must_use]
    pub const fn context(&self) -> &IRContext { &self.context }

    #[must_use]
    pub fn insert_lambda_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        self.context.assert_as_lambda_context_mut().insert_parameter(parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        match &mut self.context {
            IRContext::OperationHandler(context) => context.insert_parameter(parameter),
            IRContext::Def | IRContext::Lambda(_) | IRContext::Thunk(_) => {
                panic!("operation handler parameters require an operation handler context")
            }
        }
    }

    #[must_use]
    pub fn get_expression(&self, id: IRExprID) -> &IRExpr { self.expression_map.get_expression(id) }

    #[must_use]
    pub fn get_variable(&self, id: IRVariableID) -> &IRVariable {
        self.variable_map.get_variable(id)
    }

    /// Iterates over function-local storage and its IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn variables(&self) -> impl ExactSizeIterator<Item = (IRVariableID, &IRVariable)> {
        self.variable_map.variables()
    }

    #[must_use]
    pub const fn root_scope_id(&self) -> ScopeID { self.scope_map.root_id() }

    #[must_use]
    pub fn declared_variables(
        &self,
        scope_id: ScopeID,
    ) -> impl ExactSizeIterator<Item = IRVariableID> + '_ {
        self.get_scope(scope_id).declared_variables()
    }

    #[must_use]
    fn get_scope(&self, id: ScopeID) -> &Scope { self.scope_map.get_scope(id) }

    #[must_use]
    fn insert_scope(&mut self, parent: ScopeID) -> ScopeID { self.scope_map.insert_scope(parent) }

    #[must_use]
    fn insert_scope_branch(&mut self, parent: ScopeID, branch_count: usize) -> Vec<ScopeID> {
        self.scope_map.insert_branch(parent, branch_count)
    }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry_block() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: IRExpr) -> IRExprID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    fn create_variable_in_scope(
        &mut self,
        scope_id: ScopeID,
        ty: Interned<Ty>,
        span: RelativeSpan,
    ) -> IRVariableID {
        let variable_id = self.variable_map.insert_variable(ty, span, scope_id);
        self.scope_map.register_variable(scope_id, variable_id);
        variable_id
    }

    pub fn push_expression(&mut self, block_id: BlockID, expression: IRExprID) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_expr_discard(
        &mut self,
        block_id: BlockID,
        expression: IRExprID,
        drop_instance: Interned<Ty>,
    ) {
        self.cfg.push_expr_discard(block_id, expression, drop_instance);
    }

    pub fn push_scope_push_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.cfg.push_scope_push_instruction(block_id, scope_id);
    }

    pub fn push_scope_pop_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.cfg.push_scope_pop_instruction(block_id, scope_id);
    }

    pub fn push_store(
        &mut self,
        block_id: BlockID,
        address: Address,
        value: IRExprID,
        span: RelativeSpan,
    ) {
        self.cfg.push_store(block_id, address, value, span);
    }

    /// Applies every instruction queued in `insertion`.
    pub fn insert_instructions(&mut self, insertion: InstructionInsertion) {
        self.cfg.insert_instructions(insertion);
    }

    /// Redirects `edge` through a new empty block and returns that block.
    ///
    /// Every phi in the edge's target then takes the value that flowed in
    /// along the edge from the new block.
    pub fn split_edge(&mut self, edge: ControlFlowEdge) -> BlockID {
        let split_block = self.cfg.split_edge(edge);

        // The source may still reach the target through another edge, as when
        // both arms of a conditional target it. The phis then keep the value
        // on that edge as well.
        let source_still_incoming = self
            .cfg
            .terminator(edge.source())
            .is_some_and(|terminator| terminator.jump_targets().any(|t| t == edge.target()));

        // Move the value the phis took from the source onto the new block.
        for instruction in self.cfg.instructions(edge.target()) {
            let Instruction::Expression(expression_id) = instruction else {
                continue;
            };
            if let Some(phi) = self.expression_map.phi_mut(*expression_id) {
                phi.split_incoming(edge.source(), split_block, source_still_incoming);
            }
        }

        split_block
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_instructions(&self, block_id: BlockID) -> &[Instruction] {
        self.cfg.instructions(block_id)
    }

    /// Iterates over the instructions of a block, in order, each with the
    /// point it sits at.
    #[must_use]
    pub fn block_instructions_with_points(
        &self,
        block_id: BlockID,
    ) -> impl ExactSizeIterator<Item = (Point, &Instruction)> {
        self.cfg.instructions_with_points(block_id)
    }

    /// Iterates over the points control reaches right after `point`; see
    /// [`Cfg::successor_points`].
    pub fn successor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        self.cfg.successor_points(point)
    }

    /// Iterates over the points control comes from right before `point`; see
    /// [`Cfg::predecessor_points`].
    pub fn predecessor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        self.cfg.predecessor_points(point)
    }

    #[must_use]
    pub fn block_terminator(&self, block_id: BlockID) -> Option<&Terminator> {
        self.cfg.terminator(block_id)
    }

    /// Returns the point that stands for the terminator of `block_id`: one
    /// past its last instruction.
    #[must_use]
    pub fn terminator_point(&self, block_id: BlockID) -> Point {
        Point::builder()
            .block_id(block_id)
            .instruction_idx(self.cfg.instructions(block_id).len())
            .build()
    }

    /// Iterates over blocks that do not have a terminator.
    pub fn unterminated_blocks(&self) -> impl Iterator<Item = BlockID> + '_ {
        self.cfg.unterminated_blocks()
    }

    #[must_use]
    pub fn reachables(&self) -> Reachables { self.cfg.reachables() }

    /// Solves a dataflow problem over this function's control-flow graph.
    pub async fn solve_dataflow<P: DataflowProblem>(
        &self,
        problem: &mut P,
    ) -> Result<DataflowSolution<P::JoinLattice>, P::Error> {
        solve(problem, &self.cfg).await
    }
}

impl IRFunction {
    /// Visits every type of the function `function_id`: its signature, then
    /// its body.
    ///
    /// The definition function stores no signature types.
    fn visit_types<V: TypeVisitor>(&self, function_id: FunctionID, visitor: &mut V) {
        self.context.visit_types(TypeSite::Signature(function_id), visitor);

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types(body, visitor);
        self.expression_map.visit_types(body, visitor);
        self.cfg.visit_types(body, visitor);
    }

    /// Visits every type of the function `function_id` mutably: its
    /// signature, then its body.
    ///
    /// The definition function stores no signature types.
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, function_id: FunctionID, visitor: &mut V) {
        self.context.visit_types_mut(TypeSite::Signature(function_id), visitor);

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types_mut(body, visitor);
        self.expression_map.visit_types_mut(body, visitor);
        self.cfg.visit_types_mut(body, visitor);
    }

    /// Like [`Self::visit_types_mut`], but with a visitor that may await.
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        function_id: FunctionID,
        visitor: &mut V,
    ) {
        self.context.visit_types_mut_async(TypeSite::Signature(function_id), visitor).await;

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types_mut_async(body, visitor).await;
        self.expression_map.visit_types_mut_async(body, visitor).await;
        self.cfg.visit_types_mut_async(body, visitor).await;
    }
}

impl VisitTypeMut for IRContext {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types_mut(site, visitor),
            Self::Thunk(context) => context.visit_types_mut(site, visitor),
            Self::OperationHandler(context) => context.visit_types_mut(site, visitor),
        }
    }
}

impl VisitTypeMutAsync for IRContext {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types_mut_async(site, visitor).await,
            Self::Thunk(context) => context.visit_types_mut_async(site, visitor).await,
            Self::OperationHandler(context) => context.visit_types_mut_async(site, visitor).await,
        }
    }
}

impl VisitType for IRContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types(site, visitor),
            Self::Thunk(context) => context.visit_types(site, visitor),
            Self::OperationHandler(context) => context.visit_types(site, visitor),
        }
    }
}

impl VisitType for IRThunkContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(self.return_ty(), site);
        visitor.visit_type(self.effect(), site);
    }
}

impl VisitType for IROperationHandlerContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, parameter) in self.parameters() {
            parameter.visit_types(site, visitor);
        }
        visitor.visit_type(self.return_ty(), site);
        visitor.visit_type(self.effect(), site);
    }
}

impl VisitType for IRLambdaContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, parameter) in self.parameters() {
            parameter.visit_types(site, visitor);
        }
        visitor.visit_type(self.return_ty(), site);
        visitor.visit_type(self.effect(), site);
    }
}

impl VisitType for LambdaParameter {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(self.ty(), site);
    }
}

impl VisitType for OperationHandlerParameter {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(self.ty(), site);
    }
}

impl VisitType for Capture {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(self.binding_ty(), site);
        self.mode().visit_types(site, visitor);
    }
}

impl VisitType for CaptureMode {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Value(_) => {}
            Self::Reference { lifetime, .. } => visitor.visit_type(lifetime, site),
        }
    }
}

impl VisitType for CaptureMap {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, capture) in self.iter() {
            capture.visit_types(site, visitor);
        }
    }
}

#[cfg(test)]
mod tests;
