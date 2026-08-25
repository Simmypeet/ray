use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    expression::{Expression, ExpressionID},
    function::{Context as IrContext, Function as IrFunction},
    lambda::{Capture, CaptureID, LambdaParameter as IrLambdaParameter, LambdaParameterID},
    variable::{Variable, VariableID},
};
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::{Mutability, Ty};
use rayc_typed_ast::{
    lambda::LambdaParameterID as TypedLambdaParameterID,
    name_binding::Source,
    typed_function::{Context as TypedContext, FunctionID as TypedFunctionID},
    variable::VariableID as TypedVariableID,
};

use crate::context::LoweringContext;

pub(super) enum SourceLocation {
    Variable(VariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    Capture { id: CaptureID, span: RelativeSpan, pointee_ty: Interned<Ty>, mutability: Mutability },
}

pub(super) struct FunctionBuildState {
    function: IrFunction,
    current_block: BlockID,
    typed_function_id: TypedFunctionID,
    variables: FxHashMap<TypedVariableID, VariableID>,
    lambda_parameters: FxHashMap<TypedLambdaParameterID, LambdaParameterID>,
    captures: FxHashMap<Source, CaptureID>,
}

impl FunctionBuildState {
    pub(super) fn new(context: &LoweringContext<'_>) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let mut lambda_parameters = FxHashMap::default();
        let mut captures = FxHashMap::default();
        let function = match context.typed_function_context() {
            TypedContext::Def(_) => {
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
                IrFunction::new()
            }
            TypedContext::Lambda(lambda_context) => {
                assert_ne!(
                    typed_function_id,
                    context.root_typed_function_id(),
                    "root TypedAST function should not be a lambda"
                );
                let mut function = IrFunction::new_lambda();
                for (typed_id, parameter) in lambda_context.parameters() {
                    let ir_id = function.insert_lambda_parameter(IrLambdaParameter::new(
                        parameter.ty().clone(),
                        parameter.span(),
                    ));
                    assert!(lambda_parameters.insert(typed_id, ir_id).is_none());
                }
                for (_, requirement) in capture_plan.captures() {
                    let ir_id = function.insert_capture(Capture::new(
                        requirement.pointee_ty().clone(),
                        requirement.mutability(),
                        requirement.span(),
                    ));
                    assert!(captures.insert(requirement.source(), ir_id).is_none());
                }
                function
            }
        };
        let current_block = function.entry_block();
        Self {
            function,
            current_block,
            typed_function_id,
            variables: FxHashMap::default(),
            lambda_parameters,
            captures,
        }
    }

    pub(super) const fn typed_function_id(&self) -> TypedFunctionID { self.typed_function_id }

    pub(super) fn into_function(self) -> IrFunction { self.function }

    pub(super) fn emit_expression(&mut self, expression: Expression) -> ExpressionID {
        let expression_id = self.function.insert_expression(expression);
        self.function.push_expression(self.current_block, expression_id);
        expression_id
    }

    pub(super) fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.function.push_store(self.current_block, address, value);
    }

    pub(super) fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> VariableID {
        self.function.insert_variable(Variable::new(ty, span))
    }

    pub(super) fn register_source_variable(
        &mut self,
        context: &LoweringContext<'_>,
        typed_id: TypedVariableID,
    ) -> VariableID {
        let variable = context.variable(typed_id);
        let ir_id =
            self.function.insert_variable(Variable::new(variable.ty().clone(), variable.span()));
        self.variables.insert(typed_id, ir_id);
        ir_id
    }

    pub(super) fn terminate(&mut self, terminator: Terminator) {
        self.function.set_terminator(self.current_block, terminator);
    }

    pub(super) fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub(super) fn is_terminated(&self) -> bool {
        self.function.block_terminator(self.current_block).is_some()
    }

    pub(super) fn create_block(&mut self) -> BlockID { self.function.create_block() }

    pub(super) const fn select_block(&mut self, block: BlockID) { self.current_block = block; }

    pub(super) fn resolve_source(&self, source: Source) -> SourceLocation {
        if source.function_id() == self.typed_function_id {
            return match source {
                Source::Variable(id) => {
                    let variable_id = self
                        .variables
                        .get(&id.local_id())
                        .copied()
                        .expect("local source variable should be registered before use");
                    SourceLocation::Variable(variable_id)
                }
                Source::Parameter(id) => SourceLocation::Parameter(id.local_id()),
                Source::LambdaParameter(id) => {
                    let parameter_id = self
                        .lambda_parameters
                        .get(&id.local_id())
                        .copied()
                        .expect("lambda parameter should be registered before use");
                    SourceLocation::LambdaParameter(parameter_id)
                }
            };
        }

        let capture_id = self
            .captures
            .get(&source)
            .copied()
            .expect("non-local source should have an analyzed capture");
        let capture = match self.function.context() {
            IrContext::Def => panic!("def functions should not contain captures"),
            IrContext::Lambda(context) => context.get_capture(capture_id),
        };
        SourceLocation::Capture {
            id: capture_id,
            span: capture.span(),
            pointee_ty: capture.pointee_ty().clone(),
            mutability: capture.mutability(),
        }
    }
}
