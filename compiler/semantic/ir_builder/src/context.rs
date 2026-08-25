use qbice::storage::intern::Interned;
use rayc_tast_capture_analysis::{CaptureAnalysis, FunctionCapturePlan};
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    name_binding::{NameBindingID, Source},
    statement::Statement,
    typed_expr::{LvalueClassification, TypedExpr, TypedExprID},
    typed_function::{
        TypedContext as TypedFunctionContext, TypedFunction, TypedFunctionID, TypedFunctionMap,
    },
    typed_variable::{TypedVariable, TypedVariableID},
};

pub struct LoweringContext<'a> {
    typed_functions: &'a TypedFunctionMap,
    analysis: &'a CaptureAnalysis,
    typed_function_id: TypedFunctionID,
    typed_function: &'a TypedFunction,
}

impl<'a> LoweringContext<'a> {
    pub fn new(typed_functions: &'a TypedFunctionMap, analysis: &'a CaptureAnalysis) -> Self {
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

    pub const fn typed_function_context(&self) -> &TypedFunctionContext {
        self.typed_function.context()
    }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> {
        self.typed_function.statements()
    }

    pub fn expression(&self, id: TypedExprID) -> &TypedExpr {
        self.typed_function.get_expression(id)
    }

    pub fn classify_lvalue(&self, id: TypedExprID) -> LvalueClassification {
        self.typed_function.classify_lvalue(id)
    }

    pub fn expression_ty(&self, id: TypedExprID) -> &Interned<Ty> {
        self.typed_function.get_type_of_expr_id(id)
    }

    pub fn variable(&self, id: TypedVariableID) -> &TypedVariable {
        self.typed_function.get_variable(id)
    }

    pub fn capture_plan(&self, id: TypedFunctionID) -> &FunctionCapturePlan {
        self.analysis.plan(id)
    }

    pub fn name_binding_source(&self, id: NameBindingID) -> Source {
        *self.typed_functions.get_name_binding(id).source()
    }
}
