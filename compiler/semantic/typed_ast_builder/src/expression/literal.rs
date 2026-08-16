use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Boolean, Literal as LiteralSyntax};
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, literal::Literal};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<LiteralSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: LiteralSyntax) -> TypedExprID {
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
        };

        self.insert_expression(TypedExpr::new(TypedExprKind::Literal(literal), syn.span(), ty))
    }
}
