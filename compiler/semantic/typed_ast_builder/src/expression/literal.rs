use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Boolean, Literal as LiteralSyntax};
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, literal::Literal};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, EmbeddedNulString},
    tast_builder::TAstBuilder,
};

impl Bind<LiteralSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: LiteralSyntax) -> TypedExprID {
        if let LiteralSyntax::String(token) = &syn
            && token.kind.0.as_bytes().contains(&0)
        {
            self.push_diagnostic(Diagnostic::EmbeddedNulString(
                EmbeddedNulString::builder().span(syn.span()).build(),
            ));
        }
        let (literal, ty) = match &syn {
            LiteralSyntax::Boolean(boolean) => {
                let value = match boolean {
                    Boolean::True(_) => true,
                    Boolean::False(_) => false,
                };

                (Literal::Bool(value), Ty::new_primitive(Primitive::Bool, self.engine()))
            }

            LiteralSyntax::Numeric(token) => (
                Literal::Numeric(
                    // TODO: properly handle overflow
                    token.kind.0.parse::<u128>().expect("should've been a valid numeric literal"),
                ),
                self.new_numeric_type_inference(),
            ),
            LiteralSyntax::String(token) => (
                Literal::String(token.kind.0.clone()),
                Ty::new_primitive(Primitive::CStr, self.engine()),
            ),
        };

        self.insert_expression(TypedExprKind::Literal(literal), syn.span(), ty).await
    }
}
