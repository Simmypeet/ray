use qbice::storage::intern::Interned;
use rayc_type::ty::{Ty, application::ClosureID};
use rayc_typed_ast::{
    capture_plan::{CapturePlan, FunctionCapturePlan},
    name_binding::{NameBindingID, Source},
    statement::Statement,
    typed_expr::{TypedExpr, TypedExprID},
    typed_function::{
        TypedContext as TypedFunctionContext, TypedFunction, TypedFunctionID, TypedFunctionMap,
    },
    typed_variable::{TypedVariable, TypedVariableID},
};

pub struct LoweringContext<'a> {
    typed_functions: &'a TypedFunctionMap,
    analysis: &'a CapturePlan,
    typed_function_id: TypedFunctionID,
    typed_function: &'a TypedFunction,
}

impl<'a> LoweringContext<'a> {
    pub fn new(typed_functions: &'a TypedFunctionMap, analysis: &'a CapturePlan) -> Self {
        let typed_function_id = typed_functions.root_id();
        let typed_function = typed_functions.root();
        Self { typed_functions, analysis, typed_function_id, typed_function }
    }

    pub fn for_function(&self, typed_function_id: TypedFunctionID) -> Self {
        Self {
            typed_functions: self.typed_functions,
            analysis: self.analysis,
            typed_function_id,
            typed_function: self.typed_functions.get_function(typed_function_id),
        }
    }

    pub const fn root_typed_function_id(&self) -> TypedFunctionID { self.typed_functions.root_id() }

    pub const fn typed_function_id(&self) -> TypedFunctionID { self.typed_function_id }

    pub fn closure_function(&self, closure_id: ClosureID) -> Option<TypedFunctionID> {
        self.typed_functions.closure_function(closure_id)
    }

    pub const fn typed_function_context(&self) -> &TypedFunctionContext {
        self.typed_function.context()
    }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> {
        self.typed_function.statements()
    }

    pub fn expression(&self, id: TypedExprID) -> &TypedExpr {
        self.typed_function.get_expression(id)
    }

    pub fn function_effect(&self) -> &Interned<Ty> {
        self.typed_functions.effect_of(self.typed_function_id)
    }

    pub fn variable(&self, id: TypedVariableID) -> &TypedVariable {
        self.typed_function.get_variable(id)
    }

    pub fn capture_plan(&self, id: TypedFunctionID) -> &FunctionCapturePlan {
        self.analysis.plan(id)
    }

    pub fn shares_capture_plan(&self, first: TypedFunctionID, second: TypedFunctionID) -> bool {
        self.analysis.shares_plan(first, second)
    }

    pub fn name_binding_source(&self, id: NameBindingID) -> Source {
        *self.typed_functions.get_name_binding(id).source()
    }
}
