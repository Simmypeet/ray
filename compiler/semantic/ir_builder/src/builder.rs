use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Address,
    ir_expr::IRExprID,
    ir_function::IRFunctionMap,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::{Mutability, Ty, application::ClosureID};

use self::function_build_state::FunctionBuildState;
use crate::diagnostic::NotAllPathsReturnValue;

mod function_build_state;

pub struct Builder {
    engine: TrackedEngine,
    ir_functions: IRFunctionMap,
    building_function: FunctionBuildState,
    suspended_functions: Vec<FunctionBuildState>,
    diagnostics: Vec<NotAllPathsReturnValue>,
}

impl Builder {
    pub fn register_closure(
        &mut self,
        closure_id: ClosureID,
        function_id: rayc_ir::ir_function::FunctionID,
    ) {
        self.ir_functions.register_closure(closure_id, function_id);
    }

    pub fn pointer_ty(&self, pointee_ty: Interned<Ty>, mutability: Mutability) -> Interned<Ty> {
        Ty::new_pointer(pointee_ty, mutability, &self.engine)
    }

    pub fn error_address(&self) -> Address { Address::new_error(&self.engine) }

    pub fn variable_address(&self, id: IRVariableID) -> Address {
        Address::new_variable(id, &self.engine)
    }

    pub fn parameter_address(&self, id: ParameterID) -> Address {
        Address::new_parameter(id, &self.engine)
    }

    pub fn lambda_parameter_address(&self, id: LambdaParameterID) -> Address {
        Address::new_lambda_parameter(id, &self.engine)
    }

    pub fn operation_handler_parameter_address(&self, id: OperationHandlerParameterID) -> Address {
        Address::new_operation_handler_parameter(id, &self.engine)
    }

    pub fn capture_address(&self, id: CaptureID) -> Address {
        Address::new_capture(id, &self.engine)
    }

    pub fn dereference_address(&self, value: IRExprID) -> Address {
        Address::new_deref(value, &self.engine)
    }

    pub fn project_tuple(&self, address: &mut Address, index: usize) {
        address.add_tuple_index(index, &self.engine);
    }
}
