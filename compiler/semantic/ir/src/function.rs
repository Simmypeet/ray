use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_type::ty::Ty;

use crate::{
    address::Address,
    cfg::{BlockID, Cfg, Instruction, Reachables, Terminator},
    expression::{Expression, ExpressionID, ExpressionMap},
    lambda::{Capture, CaptureID, LambdaContext, LambdaParameter, LambdaParameterID},
    variable::{Variable, VariableID, VariableMap},
    visit::{TypeVisitor, VisitType},
};

pub type FunctionID = ID<Function>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct FunctionMap {
    functions: Arena<Function>,
    root: FunctionID,
}

impl FunctionMap {
    #[must_use]
    pub fn new() -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(Function::new());
        Self { functions, root }
    }

    #[must_use]
    pub const fn root_id(&self) -> FunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &Function {
        self.functions.get(self.root).expect("Root IR function should exist")
    }

    #[must_use]
    pub fn get_function(&self, id: FunctionID) -> &Function {
        self.functions.get(id).expect("IR function should exist")
    }

    fn get_function_mut(&mut self, id: FunctionID) -> &mut Function {
        self.functions.get_mut(id).expect("IR function should exist")
    }

    /// Iterates over all IR functions belonging to a source def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (FunctionID, &Function)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_lambda(&mut self, return_ty: Interned<Ty>) -> FunctionID {
        self.functions.insert(Function::new_lambda(return_ty))
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
        match self.get_function(function_id).context() {
            Context::Def => panic!("def functions should not contain captures"),
            Context::Lambda(context) => context.get_capture(capture_id),
        }
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
        expression: Expression,
    ) -> ExpressionID {
        self.get_function_mut(function_id).insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, function_id: FunctionID, variable: Variable) -> VariableID {
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

impl Default for FunctionMap {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum Context {
    Def,
    Lambda(LambdaContext),
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct Function {
    cfg: Cfg,
    variable_map: VariableMap,
    expression_map: ExpressionMap,
    context: Context,
}

impl Default for Function {
    fn default() -> Self { Self::new() }
}

impl Function {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: VariableMap::default(),
            expression_map: ExpressionMap::default(),
            context: Context::Def,
        }
    }

    #[must_use]
    pub fn new_lambda(return_ty: Interned<Ty>) -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: VariableMap::default(),
            expression_map: ExpressionMap::default(),
            context: Context::Lambda(LambdaContext::new(return_ty)),
        }
    }

    #[must_use]
    pub const fn context(&self) -> &Context { &self.context }

    #[must_use]
    pub fn insert_lambda_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        match &mut self.context {
            Context::Def => panic!("lambda parameters cannot be inserted into a def"),
            Context::Lambda(context) => context.insert_parameter(parameter),
        }
    }

    #[must_use]
    pub fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        match &mut self.context {
            Context::Def => panic!("captures cannot be inserted into a def"),
            Context::Lambda(context) => context.insert_capture(capture),
        }
    }

    #[must_use]
    pub fn get_expression(&self, id: ExpressionID) -> &Expression {
        self.expression_map.get_expression(id)
    }

    #[must_use]
    pub fn get_variable(&self, id: VariableID) -> &Variable { self.variable_map.get_variable(id) }

    /// Iterates over function-local storage and its IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn variables(&self) -> impl ExactSizeIterator<Item = (VariableID, &Variable)> {
        self.variable_map.variables()
    }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry_block() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: Expression) -> ExpressionID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
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

impl VisitType for Function {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        self.context.visit_types(visitor);
        self.variable_map.visit_types(visitor);
        self.expression_map.visit_types(visitor);
    }
}

impl VisitType for Context {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Lambda(context) => context.visit_types(visitor),
        }
    }
}

impl VisitType for LambdaContext {
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
