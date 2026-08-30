use rayc_source_file::SourceElement;
use rayc_syntax::expression::RunWith as RunWithSyntax;
use rayc_type::ty::Ty;
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, errored::Errored, run_with::RunWith};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, EffectHandlerNotSupported},
    tast_builder::TAstBuilder,
};

impl Bind<RunWithSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: RunWithSyntax) -> TypedExprID {
        let unit = Ty::new_unit(self.engine());
        let Some(effect) = syn.effect() else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit);
        };
        let Ok(effect) = self.resolve_effect_path(&effect).await else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit);
        };

        self.push_diagnostic(Diagnostic::EffectHandlerNotSupported(
            EffectHandlerNotSupported::builder().span(syn.span()).build(),
        ));

        self.insert_expression(
            TypedExprKind::RunWith(RunWith::new(effect.symbol_id())),
            syn.span(),
            unit,
        )
    }
}
