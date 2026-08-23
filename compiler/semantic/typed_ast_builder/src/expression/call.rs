use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, MemberID};
use rayc_syntax::expression::Call as CallSyn;
use rayc_type::{
    poly_var::{PolyVarMap, get_poly_var_map},
    subst::{Subst, Substitutable},
};
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, call::Call};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, MismatchedArgumentCount},
    tast_builder::TAstBuilder,
};

impl TAstBuilder {
    fn instantiate_poly_vars(
        &mut self,
        function_id: GlobalSymbolID,
        poly_var_map: &PolyVarMap,
    ) -> Subst {
        poly_var_map
            .iter()
            .map(|(id, poly_var)| {
                (MemberID::new(function_id, id), self.new_type_inference_with_kind(poly_var.kind()))
            })
            .collect()
    }
}

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

        let poly_var_map = self.engine().get_poly_var_map(function_id).await;
        let call_subst = self.instantiate_poly_vars(function_id, &poly_var_map);

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
            let parameter_ty = param.ty().apply_subst_or_clone(&call_subst, self.engine());
            self.push_function_call_constraint(&parameter_ty, *arg);
        }

        let return_type = self.engine().get_return_type(function_id).await;
        let return_type = return_type.apply_subst_or_clone(&call_subst, self.engine());

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Call(Call::new(function_id, args, call_subst)),
            syn.span(),
            return_type,
        ))
    }
}
