use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Address,
    ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load},
    ir_function::IRFunctionMap,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::ParameterID, struct_body::FieldID};
use rayc_type::ty::{Mutability, Ty, application::ClosureID};
use rayc_typed_ast::typed_expr::TypedExprID;

use self::function_build_state::FunctionBuildState;
use crate::{context::LoweringContext, diagnostic::NotAllPathsReturnValue, statement::LoopTarget};

mod function_build_state;

pub struct Builder {
    engine: TrackedEngine,
    ir_functions: IRFunctionMap,
    building_function: FunctionBuildState,
    suspended_functions: Vec<FunctionBuildState>,
    diagnostics: Vec<NotAllPathsReturnValue>,
}

impl Builder {
    pub(crate) fn push_loop_target(&mut self, target: LoopTarget) {
        self.building_function.push_loop_target(target);
    }

    pub(crate) fn pop_loop_target(&mut self) { self.building_function.pop_loop_target(); }

    pub(crate) fn current_loop_target(&self) -> Option<LoopTarget> {
        self.building_function.current_loop_target()
    }

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

    pub fn project_field(&self, address: &mut Address, field_id: FieldID) {
        address.add_field(field_id, &self.engine);
    }

    pub fn lower_address_and_load(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> IRExprID {
        let address = self.lower_address_by_id(context, expression_id);
        let typed_expression = context.expression(expression_id);
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();

        self.emit_expression(IRExpr::new(IRExprKind::Load(Load::new(address)), span, ty))
    }
}
