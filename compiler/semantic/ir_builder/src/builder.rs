use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Address,
    ir_expr::IRExprID,
    ir_function::IRFunctionMap,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_variable::IRVariableID,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::{Mutability, Ty};

use self::function_build_state::FunctionBuildState;

mod function_build_state;

pub struct Builder {
    engine: TrackedEngine,
    ir_functions: IRFunctionMap,
    building_function: FunctionBuildState,
    suspended_functions: Vec<FunctionBuildState>,
}

impl Builder {
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
