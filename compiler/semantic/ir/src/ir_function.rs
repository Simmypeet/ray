use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_type::ty::Ty;

use crate::{
    address::Address,
    cfg::{BlockID, Cfg, Instruction, Reachables, Terminator},
    ir_expr::{ExpressionID, IRExpr, IRExpressionMap},
    ir_lambda::{Capture, CaptureID, IRLambdaContext, LambdaParameter, LambdaParameterID},
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
    pub fn new() -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(IRFunction::new());
        Self { functions, root }
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
    pub fn insert_lambda(&mut self, return_ty: Interned<Ty>) -> FunctionID {
        self.functions.insert(IRFunction::new_lambda(return_ty))
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
    pub fn insert_capture(&mut self, function_id: FunctionID, capture: Capture) -> CaptureID {
        self.get_function_mut(function_id).insert_capture(capture)
    }

    #[must_use]
    pub fn get_capture(&self, function_id: FunctionID, capture_id: CaptureID) -> &Capture {
        self.get_function(function_id).context().assert_as_lambda_context().get_capture(capture_id)
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
    pub fn insert_expression(
        &mut self,
        function_id: FunctionID,
        expression: IRExpr,
    ) -> ExpressionID {
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
        expression: ExpressionID,
    ) {
        self.get_function_mut(function_id).push_expression(block_id, expression);
    }

    pub fn push_store(
        &mut self,
        function_id: FunctionID,
        block_id: BlockID,
        address: Address,
        value: ExpressionID,
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

impl Default for IRFunctionMap {
    fn default() -> Self { Self::new() }
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
}

impl IRContext {
    #[track_caller]
    pub fn assert_as_def_context(&self) {
        match self {
            Self::Def => {}
            Self::Lambda(_) => panic!("expected a def context, found a lambda context"),
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_lambda_context(&self) -> &IRLambdaContext {
        match self {
            Self::Def => panic!("expected a lambda context, found a def context"),
            Self::Lambda(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_lambda_context_mut(&mut self) -> &mut IRLambdaContext {
        match self {
            Self::Def => panic!("expected a lambda context, found a def context"),
            Self::Lambda(context) => context,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunction {
    cfg: Cfg,
    variable_map: IRVariableMap,
    expression_map: IRExpressionMap,
    context: IRContext,
}

impl Default for IRFunction {
    fn default() -> Self { Self::new() }
}

impl IRFunction {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Def,
        }
    }

    #[must_use]
    pub fn new_lambda(return_ty: Interned<Ty>) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Lambda(IRLambdaContext::new(return_ty)),
        }
    }

    #[must_use]
    pub const fn context(&self) -> &IRContext { &self.context }

    #[must_use]
    pub fn insert_lambda_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        self.context.assert_as_lambda_context_mut().insert_parameter(parameter)
    }

    #[must_use]
    pub fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        self.context.assert_as_lambda_context_mut().insert_capture(capture)
    }

    #[must_use]
    pub fn get_expression(&self, id: ExpressionID) -> &IRExpr {
        self.expression_map.get_expression(id)
    }

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
    pub fn insert_expression(&mut self, expression: IRExpr) -> ExpressionID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: IRVariable) -> IRVariableID {
        self.variable_map.insert_variable(variable)
    }

    pub fn push_expression(&mut self, block_id: BlockID, expression: ExpressionID) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_store(&mut self, block_id: BlockID, address: Address, value: ExpressionID) {
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

    #[must_use]
    pub fn reachables(&self) -> Reachables { self.cfg.reachables() }
}

impl VisitType for IRFunction {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        self.context.visit_types(visitor);
        self.variable_map.visit_types(visitor);
        self.expression_map.visit_types(visitor);
    }
}

impl VisitType for IRContext {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types(visitor),
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

impl VisitType for Capture {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        visitor.visit_type(self.pointee_ty());
    }
}
