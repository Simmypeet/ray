use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_source_file::SourceElement;
use rayc_syntax::expression::Call as CallSyn;
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, call::Call};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, MismatchedArgumentCount},
    tast_builder::TAstBuilder,
};

impl Bind<CallSyn> for TAstBuilder {
    async fn bind(&mut self, syn: CallSyn) -> TypedExprID {
        let mut args = Vec::new();

        if let Some(arg) = syn.arguments() {
            for arg in arg.expressions() {
                args.push(self.bind(arg).await);
            }
        }

        let Some(identifier) = syn.def_name() else {
            return self.push_error_expression_with_children(syn.span(), args);
        };

        let Some(function_id) = self.resolve_function_id(&identifier).await else {
            return self.push_error_expression_with_children(syn.span(), args);
        };

        let parameter_map = self.engine().get_parameter_map(function_id).await;

        if parameter_map.len() != args.len() {
            self.push_diagnostic(Diagnostic::MismatchedArgumentCount(
                MismatchedArgumentCount::builder()
                    .calling_symbol(function_id)
                    .expected(parameter_map.len())
                    .found(args.len())
                    .span(syn.span())
                    .build(),
            ));
        }

        for ((_, param), arg) in parameter_map.iter().zip(args.iter()) {
            self.push_function_call_constraint(param.ty(), *arg);
        }

        let return_type = self.engine().get_return_type(function_id).await;

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Call(Call::new(function_id, args)),
            syn.span(),
            return_type,
        ))
    }
}
