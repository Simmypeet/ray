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
    function::Function as TypedFunction, variable::VariableID as TypedVariableID,
};

pub(crate) struct Builder {
    engine: TrackedEngine,
    function: IrFunction,
    current_block: BlockID,
    variables: FxHashMap<TypedVariableID, VariableID>,
}

impl Builder {
    pub(crate) fn new(engine: TrackedEngine) -> Self {
        let function = IrFunction::new();
        let current_block = function.entry_block();
        Self { engine, function, current_block, variables: FxHashMap::default() }
    }

    pub(crate) fn lower(mut self, typed_function: &TypedFunction) -> IrFunction {
        self.lower_statements(typed_function);
        self.function.validate().expect("lowering should produce structurally valid IR");
        self.function
    }

    pub(crate) fn emit_expression(&mut self, expression: Expression) -> ExpressionID {
        let expression_id = self.function.insert_expression(expression);
        self.function.push_expression(self.current_block, expression_id);
        expression_id
    }

    pub(crate) fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.function.push_store(self.current_block, address, value);
    }

    pub(crate) fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> VariableID {
        self.function.insert_variable(Variable::new(ty, span))
    }

    pub(crate) fn register_source_variable(
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

    pub(crate) fn terminate(&mut self, terminator: Terminator) {
        self.function.set_terminator(self.current_block, terminator);
    }

    pub(crate) fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub(crate) fn is_terminated(&self) -> bool {
        self.function.block_terminator(self.current_block).is_some()
    }

    pub(crate) fn create_block(&mut self) -> BlockID { self.function.create_block() }

    pub(crate) const fn select_block(&mut self, block: BlockID) { self.current_block = block; }

    pub(crate) fn source_variable(&self, id: TypedVariableID) -> Option<VariableID> {
        self.variables.get(&id).copied()
    }

    pub(crate) fn error_address(&self) -> Address { Address::new_error(&self.engine) }

    pub(crate) fn variable_address(&self, id: VariableID) -> Address {
        Address::new_variable(id, &self.engine)
    }

    pub(crate) fn parameter_address(&self, id: ParameterID) -> Address {
        Address::new_parameter(id, &self.engine)
    }

    pub(crate) fn dereference_address(&self, value: ExpressionID) -> Address {
        Address::new_deref(value, &self.engine)
    }

    pub(crate) fn project_tuple(&self, address: &mut Address, index: usize) {
        address.add_tuple_index(index, &self.engine);
    }
}
