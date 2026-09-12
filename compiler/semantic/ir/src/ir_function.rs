use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_type::ty::Ty;

use crate::{
    address::Address,
    cfg::{BlockID, Cfg, Instruction, Reachables, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExpressionMap},
    ir_lambda::{
        Capture, CaptureID, IRLambdaContext, IRThunkContext, LambdaParameter, LambdaParameterID,
    },
    ir_operation_handler::{
        IROperationHandlerContext, OperationHandlerParameter, OperationHandlerParameterID,
    },
    ir_variable::{IRVariable, IRVariableID, IRVariableMap},
    visit::{ExprVisitor, TypeVisitor, VisitExpr, VisitType},
};

pub type FunctionID = ID<IRFunction>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunctionMap {
    functions: Arena<IRFunction>,
    root: FunctionID,
}

impl IRFunctionMap {
    #[must_use]
    pub fn new(root_effect: Interned<Ty>) -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(IRFunction::new(root_effect));
        Self { functions, root }
    }

    pub fn fill_return_on_unterminated_blocks(&mut self, function_id: FunctionID) {
        self.get_function_mut(function_id).cfg.fill_return_on_unterminated_blocks();
    }

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
    pub fn insert_lambda(&mut self, return_ty: Interned<Ty>, effect: Interned<Ty>) -> FunctionID {
        self.functions.insert(IRFunction::new_lambda(return_ty, effect))
    }

    #[must_use]
    pub fn insert_thunk(&mut self, return_ty: Interned<Ty>, effect: Interned<Ty>) -> FunctionID {
        self.functions.insert(IRFunction::new_thunk(return_ty, effect))
    }

    #[must_use]
    pub fn insert_operation_handler(
        &mut self,
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> FunctionID {
        self.functions.insert(IRFunction::new_operation_handler(operation, return_ty, effect))
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
    pub fn insert_capture(&mut self, function_id: FunctionID, capture: Capture) -> CaptureID {
        self.get_function_mut(function_id).insert_capture(capture)
    }

    #[must_use]
    pub fn get_capture(&self, function_id: FunctionID, capture_id: CaptureID) -> &Capture {
        self.get_function(function_id).context().get_capture(capture_id)
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
    pub fn insert_variable(
        &mut self,
        function_id: FunctionID,
        variable: IRVariable,
    ) -> IRVariableID {
        self.get_function_mut(function_id).insert_variable(variable)
    }

    pub fn push_expression(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        expression: IRExprID,
    ) {
        self.get_function_mut(function_id).push_expression(block_id, expression);
    }

    pub fn push_store(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        address: Address,
        value: IRExprID,
    ) {
        self.get_function_mut(function_id).push_store(block_id, address, value);
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

impl VisitType for IRFunctionMap {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, function) in self.functions() {
            function.visit_types(visitor);
        }
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

    fn get_capture(&self, id: CaptureID) -> &Capture {
        match self {
            Self::Def => panic!("a def context does not have captures"),
            Self::Lambda(context) => context.get_capture(id),
            Self::Thunk(context) => context.get_capture(id),
            Self::OperationHandler(context) => context.get_capture(id),
        }
    }

    fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        match self {
            Self::Def => panic!("a def context cannot have captures"),
            Self::Lambda(context) => context.insert_capture(capture),
            Self::Thunk(context) => context.insert_capture(capture),
            Self::OperationHandler(context) => context.insert_capture(capture),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunction {
    cfg: Cfg,
    variable_map: IRVariableMap,
    expression_map: IRExpressionMap,
    context: IRContext,
    effect: Interned<Ty>,
}

impl IRFunction {
    #[must_use]
    pub fn new(effect: Interned<Ty>) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Def,
            effect,
        }
    }

    #[must_use]
    pub fn new_lambda(return_ty: Interned<Ty>, effect: Interned<Ty>) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Lambda(IRLambdaContext::new(return_ty)),
            effect,
        }
    }

    #[must_use]
    pub fn new_thunk(return_ty: Interned<Ty>, effect: Interned<Ty>) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Thunk(IRThunkContext::new(return_ty)),
            effect,
        }
    }

    #[must_use]
    pub fn new_operation_handler(
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::OperationHandler(IROperationHandlerContext::new(
                operation, return_ty,
            )),
            effect,
        }
    }

    #[must_use]
    pub const fn context(&self) -> &IRContext { &self.context }

    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }

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
    pub fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        self.context.insert_capture(capture)
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
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry_block() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: IRExpr) -> IRExprID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: IRVariable) -> IRVariableID {
        self.variable_map.insert_variable(variable)
    }

    pub fn push_expression(&mut self, block_id: BlockID, expression: IRExprID) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_store(&mut self, block_id: BlockID, address: Address, value: IRExprID) {
        self.cfg.push_store(block_id, address, value);
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_instructions(&self, block_id: BlockID) -> &[Instruction] {
        self.cfg.instructions(block_id)
    }

    #[must_use]
    pub fn block_terminator(&self, block_id: BlockID) -> Option<&Terminator> {
        self.cfg.terminator(block_id)
    }

    /// Iterates over blocks that do not have a terminator.
    pub fn unterminated_blocks(&self) -> impl Iterator<Item = BlockID> + '_ {
        self.cfg.unterminated_blocks()
    }

    #[must_use]
    pub fn reachables(&self) -> Reachables { self.cfg.reachables() }
}

impl VisitType for IRFunction {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        self.context.visit_types(visitor);
        visitor.visit_type(&self.effect);
        self.variable_map.visit_types(visitor);
        self.expression_map.visit_types(visitor);
    }
}

impl VisitType for IRContext {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types(visitor),
            Self::Thunk(context) => context.visit_types(visitor),
            Self::OperationHandler(context) => context.visit_types(visitor),
        }
    }
}

impl VisitType for IRThunkContext {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        visitor.visit_type(self.return_ty());
        for (_, capture) in self.captures() {
            capture.visit_types(visitor);
        }
    }
}

impl VisitType for IROperationHandlerContext {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, parameter) in self.parameters() {
            parameter.visit_types(visitor);
        }
        visitor.visit_type(self.return_ty());
        for (_, capture) in self.captures() {
            capture.visit_types(visitor);
        }
    }
}

impl VisitType for IRLambdaContext {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, parameter) in self.parameters() {
            parameter.visit_types(visitor);
        }
        visitor.visit_type(self.return_ty());
        for (_, capture) in self.captures() {
            capture.visit_types(visitor);
        }
    }
}

impl VisitType for LambdaParameter {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) { visitor.visit_type(self.ty()); }
}

impl VisitType for OperationHandlerParameter {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) { visitor.visit_type(self.ty()); }
}

impl VisitType for Capture {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        visitor.visit_type(self.binding_ty());
    }
}
