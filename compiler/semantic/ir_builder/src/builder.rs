use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    expression::{Expression, ExpressionID},
    function::Function as IrFunction,
    variable::{Variable, VariableID},
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::{Function as TypedFunction, FunctionID as TypedFunctionID},
    variable::VariableID as TypedVariableID,
};

pub struct Builder {
    engine: TrackedEngine,
    function: IrFunction,
    current_block: BlockID,
    typed_function_id: TypedFunctionID,
    variables: FxHashMap<TypedVariableID, VariableID>,
}

impl Builder {
    pub fn new(engine: TrackedEngine, typed_function_id: TypedFunctionID) -> Self {
        let function = IrFunction::new();
        let current_block = function.entry_block();
        Self { engine, function, current_block, typed_function_id, variables: FxHashMap::default() }
    }

    pub fn lower(mut self, typed_function: &TypedFunction) -> IrFunction {
        self.lower_statements(typed_function);
        self.function
    }

    pub fn emit_expression(&mut self, expression: Expression) -> ExpressionID {
        let expression_id = self.function.insert_expression(expression);
        self.function.push_expression(self.current_block, expression_id);
        expression_id
    }

    pub fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.function.push_store(self.current_block, address, value);
    }

    pub fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> VariableID {
        self.function.insert_variable(Variable::new(ty, span))
    }

    pub fn register_source_variable(
        &mut self,
        typed_function: &TypedFunction,
        typed_id: TypedVariableID,
    ) -> VariableID {
        let variable = typed_function.get_variable(typed_id);
        let ir_id =
            self.function.insert_variable(Variable::new(variable.ty().clone(), variable.span()));
        self.variables.insert(typed_id, ir_id);
        ir_id
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.function.set_terminator(self.current_block, terminator);
    }

    pub fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub fn is_terminated(&self) -> bool {
        self.function.block_terminator(self.current_block).is_some()
    }

    pub fn create_block(&mut self) -> BlockID { self.function.create_block() }

    pub const fn select_block(&mut self, block: BlockID) { self.current_block = block; }

    pub fn source_variable(&self, id: TypedVariableID) -> Option<VariableID> {
        self.variables.get(&id).copied()
    }

    pub const fn typed_function_id(&self) -> TypedFunctionID { self.typed_function_id }

    pub fn error_address(&self) -> Address { Address::new_error(&self.engine) }

    pub fn variable_address(&self, id: VariableID) -> Address {
        Address::new_variable(id, &self.engine)
    }

    pub fn parameter_address(&self, id: ParameterID) -> Address {
        Address::new_parameter(id, &self.engine)
    }

    pub fn dereference_address(&self, value: ExpressionID) -> Address {
        Address::new_deref(value, &self.engine)
    }

    pub fn project_tuple(&self, address: &mut Address, index: usize) {
        address.add_tuple_index(index, &self.engine);
    }
}
