use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_arena::{Arena, ID};

use crate::{
    address::Address,
    cfg::{BlockID, Cfg, Instruction, Reachables, Terminator},
    expression::{Expression, ExpressionID, ExpressionMap},
    lambda::{Capture, CaptureID, LambdaContext, LambdaParameter, LambdaParameterID},
    variable::{Variable, VariableID, VariableMap},
};

pub type FunctionID = ID<Function>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct FunctionMap {
    functions: Arena<Function>,
    root: FunctionID,
}

impl FunctionMap {
    #[must_use]
    pub fn new(root: Function) -> Self {
        match root.context() {
            Context::Def => {}
            Context::Lambda(_) => panic!("Root IR function should be a def"),
        }

        let mut functions = Arena::new();
        let root = functions.insert(root);
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

    /// Iterates over all IR functions belonging to a source def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (FunctionID, &Function)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_lambda(&mut self, function: Function) -> FunctionID {
        match function.context() {
            Context::Def => panic!("Only the root IR function should be a def"),
            Context::Lambda(_) => self.functions.insert(function),
        }
    }
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
    pub fn new_lambda() -> Self {
        Self {
            cfg: Cfg::default(),
            variable_map: VariableMap::default(),
            expression_map: ExpressionMap::default(),
            context: Context::Lambda(LambdaContext::new()),
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
