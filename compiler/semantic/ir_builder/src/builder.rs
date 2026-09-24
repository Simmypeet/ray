use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Address,
    ir_expr::IRExprID,
    ir_function::IRFunctionMap,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_memory::drop_resolution::resolve_drop_instance;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::ParameterID, struct_body::FieldID};
use rayc_solver::Solver;
use rayc_type::ty::{Mutability, Ty, TyKind, application::ClosureID};
use rayc_typed_ast::typed_function::TypedFunctionID;

use self::function_build_state::FunctionBuildState;
use crate::{context::LoweringContext, diagnostic::Diagnostic, statement::LoopTarget};

pub mod function_build_state;

pub(crate) struct Builder {
    engine: TrackedEngine,

    /// Resolves the dictionaries the IR selects, in the environment of the
    /// definition being lowered.
    solver: Solver,
    ir_functions: IRFunctionMap,
    building_function: FunctionBuildState,
    suspended_functions: Vec<FunctionBuildState>,
    diagnostics: Vec<Diagnostic>,
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
}

impl Builder {
    /// Resolves the `Drop` dictionary of each capture shared by the operation
    /// handler `handler`, in capture-layout order.
    ///
    /// The handlers only borrow their shared captures, so the function running
    /// the `run … with` drops them once the handled body returns. A borrowed
    /// capture is stored as a pointer, whose dictionary is a no-op. A capture
    /// without a usable dictionary is reported at the captured binding and
    /// gets an error dictionary.
    pub(crate) async fn handler_capture_drops(
        &mut self,
        context: &LoweringContext<'_>,
        handler: TypedFunctionID,
    ) -> Vec<Interned<Ty>> {
        let mut drops = Vec::new();
        for (_, requirement) in context.capture_plan(handler).captures() {
            let ty = requirement.storage_ty(&self.engine);
            let drop_instance = match resolve_drop_instance(&mut self.solver, ty).await {
                Ok(drop_instance) => drop_instance,
                Err(failures) => {
                    self.diagnostics.extend(
                        failures
                            .into_iter()
                            .map(|failure| failure.into_diagnostic(requirement.span()).into()),
                    );
                    Ty::new_error(TyKind::Instance, &self.engine)
                }
            };
            drops.push(drop_instance);
        }
        drops
    }
}
