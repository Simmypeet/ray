use rayc_source_file::SourceElement;
use rayc_syntax::Identifier as IdentifierSyn;
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, identifier::Identifier};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, UnboundName},
    tast_builder::TAstBuilder,
};

impl Bind<IdentifierSyn> for TAstBuilder {
    async fn bind(&mut self, syn: IdentifierSyn) -> TypedExprID {
        let Some(name_binding_id) = self.lookup_name_binding(&syn.kind.0) else {
            self.push_diagnostic(Diagnostic::UnboundName(
                UnboundName::builder().name(syn.kind.0.clone()).span(syn.span).build(),
            ));

            return self.push_error_expression(syn.span());
        };

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Identifier(Identifier::new(name_binding_id)),
            syn.span,
            self.type_of_name_binding(name_binding_id),
            self.empty_effect(),
        ))
    }
}
