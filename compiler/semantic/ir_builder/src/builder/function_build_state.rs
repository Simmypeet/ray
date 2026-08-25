use std::mem;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    ir_expr::{ExpressionID, IRExpr, IRExprKind, load::Load},
    ir_function::{FunctionID as IrFunctionID, IRFunctionMap},
    ir_lambda::{Capture, CaptureID, LambdaParameter as IrLambdaParameter, LambdaParameterID},
    ir_variable::{IRVariable, IRVariableID},
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    name_binding::Source, typed_function::TypedFunctionID,
    typed_lambda::LambdaParameterID as TypedLambdaParameterID, typed_variable::TypedVariableID,
};

use super::Builder;
use crate::context::LoweringContext;

pub(super) struct FunctionBuildState {
    ir_function_id: IrFunctionID,
    current_block: BlockID,
    typed_function_id: TypedFunctionID,
    variables: FxHashMap<TypedVariableID, IRVariableID>,
    lambda_parameters: FxHashMap<TypedLambdaParameterID, LambdaParameterID>,
    captures: FxHashMap<Source, CaptureID>,
}

impl FunctionBuildState {
    fn new_def(context: &LoweringContext<'_>, ir_functions: &mut IRFunctionMap) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let _def_context = context.typed_function_context().assert_as_def_context();
        assert_eq!(
            typed_function_id,
            context.root_typed_function_id(),
            "only the root TypedAST function should be a def"
        );
        assert_eq!(
            capture_plan.captures().len(),
            0,
            "root TypedAST function should not capture a source"
        );
        let ir_function_id = ir_functions.root_id();
        let current_block = ir_functions.entry_block(ir_function_id);
        Self {
            ir_function_id,
            current_block,
            typed_function_id,
            variables: FxHashMap::default(),
            lambda_parameters: FxHashMap::default(),
            captures: FxHashMap::default(),
        }
    }

    fn new_lambda(
        context: &LoweringContext<'_>,
        ir_functions: &mut IRFunctionMap,
        return_ty: Interned<Ty>,
    ) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let lambda_context = context.typed_function_context().assert_as_lambda_context();
        assert_ne!(
            typed_function_id,
            context.root_typed_function_id(),
            "root TypedAST function should not be a lambda"
        );
        let mut lambda_parameters = FxHashMap::default();
        let mut captures = FxHashMap::default();
        let ir_function_id = ir_functions.insert_lambda(return_ty);
        for (typed_id, parameter) in lambda_context.parameters() {
            let ir_id = ir_functions.insert_lambda_parameter(
                ir_function_id,
                IrLambdaParameter::new(parameter.ty().clone(), parameter.span()),
            );
            assert!(lambda_parameters.insert(typed_id, ir_id).is_none());
        }
        for (_, requirement) in capture_plan.captures() {
            let ir_id = ir_functions.insert_capture(
                ir_function_id,
                Capture::new(
                    requirement.pointee_ty().clone(),
                    requirement.mutability(),
                    requirement.span(),
                ),
            );
            assert!(captures.insert(requirement.source(), ir_id).is_none());
        }
        let current_block = ir_functions.entry_block(ir_function_id);
        Self {
            ir_function_id,
            current_block,
            typed_function_id,
            variables: FxHashMap::default(),
            lambda_parameters,
            captures,
        }
    }
}

impl Builder {
    pub fn new(engine: TrackedEngine, context: &LoweringContext<'_>) -> Self {
        let mut ir_functions = IRFunctionMap::new();
        let building_function = FunctionBuildState::new_def(context, &mut ir_functions);

        Self { engine, ir_functions, building_function, suspended_functions: Vec::new() }
    }

    pub fn lower(mut self, context: &LoweringContext<'_>) -> IRFunctionMap {
        self.lower_current_function(context);
        assert!(
            self.suspended_functions.is_empty(),
            "all suspended IR functions should be restored before finishing lowering"
        );
        assert_eq!(
            self.building_function.typed_function_id,
            context.root_typed_function_id(),
            "root IR function should be active when lowering finishes"
        );
        self.ir_functions
    }

    pub fn lower_lambda_function(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
        return_ty: Interned<Ty>,
    ) -> IrFunctionID {
        let lambda_context = context.for_function(typed_function_id);
        self.start_lambda(&lambda_context, return_ty);
        self.lower_current_function(&lambda_context);
        self.finish_lambda()
    }

    fn lower_current_function(&mut self, context: &LoweringContext<'_>) {
        self.lower_statements(context);
    }

    fn start_lambda(&mut self, context: &LoweringContext<'_>, return_ty: Interned<Ty>) {
        let lambda = FunctionBuildState::new_lambda(context, &mut self.ir_functions, return_ty);
        let enclosing = mem::replace(&mut self.building_function, lambda);
        self.suspended_functions.push(enclosing);
    }

    fn finish_lambda(&mut self) -> IrFunctionID {
        let enclosing = self
            .suspended_functions
            .pop()
            .expect("a lambda should suspend its enclosing IR function");
        let lambda = mem::replace(&mut self.building_function, enclosing);
        lambda.ir_function_id
    }

    pub fn emit_expression(&mut self, expression: IRExpr) -> ExpressionID {
        let function_id = self.building_function.ir_function_id;
        let expression_id = self.ir_functions.insert_expression(function_id, expression);
        self.ir_functions.push_expression(
            function_id,
            self.building_function.current_block,
            expression_id,
        );
        expression_id
    }

    pub fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.ir_functions.push_store(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            address,
            value,
        );
    }

    pub fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> IRVariableID {
        self.ir_functions
            .insert_variable(self.building_function.ir_function_id, IRVariable::new(ty, span))
    }

    pub fn register_source_variable(
        &mut self,
        context: &LoweringContext<'_>,
        typed_id: TypedVariableID,
    ) -> IRVariableID {
        let variable = context.variable(typed_id);
        let ir_id = self.ir_functions.insert_variable(
            self.building_function.ir_function_id,
            IRVariable::new(variable.ty().clone(), variable.span()),
        );
        self.building_function.variables.insert(typed_id, ir_id);
        ir_id
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.ir_functions.set_terminator(
            self.building_function.ir_function_id,
            self.building_function.current_block,
            terminator,
        );
    }

    pub fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.building_function.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub fn is_terminated(&self) -> bool {
        self.ir_functions
            .block_terminator(
                self.building_function.ir_function_id,
                self.building_function.current_block,
            )
            .is_some()
    }

    pub fn create_block(&mut self) -> BlockID {
        self.ir_functions.create_block(self.building_function.ir_function_id)
    }

    pub const fn select_block(&mut self, block: BlockID) {
        self.building_function.current_block = block;
    }

    pub fn source_address(&mut self, source: Source) -> Address {
        if source.function_id() == self.building_function.typed_function_id {
            return match source {
                Source::Variable(id) => {
                    let variable_id = self
                        .building_function
                        .variables
                        .get(&id.local_id())
                        .copied()
                        .expect("local source variable should be registered before use");
                    self.variable_address(variable_id)
                }
                Source::Parameter(id) => self.parameter_address(id.local_id()),
                Source::LambdaParameter(id) => {
                    let parameter_id = self
                        .building_function
                        .lambda_parameters
                        .get(&id.local_id())
                        .copied()
                        .expect("lambda parameter should be registered before use");
                    self.lambda_parameter_address(parameter_id)
                }
            };
        }

        let capture_id = self
            .building_function
            .captures
            .get(&source)
            .copied()
            .expect("non-local source should have an analyzed capture");
        let (span, captured_ty, mutability) = {
            let capture =
                self.ir_functions.get_capture(self.building_function.ir_function_id, capture_id);
            (capture.span(), capture.pointee_ty().clone(), capture.mutability())
        };
        let ty = self.pointer_ty(captured_ty, mutability);
        let pointer = self.emit_expression(IRExpr::new(
            IRExprKind::Load(Load::new(self.capture_address(capture_id))),
            span,
            ty,
        ));
        self.dereference_address(pointer)
    }
}
