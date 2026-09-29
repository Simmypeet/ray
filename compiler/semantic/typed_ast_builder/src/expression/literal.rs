use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Boolean, Literal as LiteralSyntax, NumericLiteral, NumericSuffix};
use rayc_type::ty::{Integer, Primitive, Ty};
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

            LiteralSyntax::Numeric(numeric) => {
                return self.bind_numeric_literal(numeric, syn.span()).await;
            }
            LiteralSyntax::String(token) => (
                Literal::String(token.kind.0.clone()),
                Ty::new_primitive(Primitive::CStr, self.engine()),
            ),
        };

        self.insert_expression(TypedExprKind::Literal(literal), syn.span(), ty).await
    }
}

impl TAstBuilder {
    /// Binds a numeric literal. A suffix such as `i8` in `23i8` fixes the
    /// literal's type; otherwise the type is inferred, defaulting to `int32`.
    async fn bind_numeric_literal(
        &mut self,
        syn: &NumericLiteral,
        span: RelativeSpan,
    ) -> TypedExprID {
        let Some(numeric) = syn.numeric() else {
            return self.push_error_expression(span).await;
        };
        let digits = numeric.kind.0;

        // The lexer only produces ASCII digits, so parsing fails only when the
        // value exceeds `u128`. Such a literal fits in no type, which the
        // deferred range check reports.
        let value = digits.parse::<u128>().ok();

        let ty = match syn.suffix() {
            Some(suffix) => {
                Ty::new_primitive(Primitive::Integer(suffix_integer(&suffix)), self.engine())
            }
            None => self.new_numeric_type_inference(),
        };

        // An out-of-range literal is always reported, so the placeholder value
        // of a literal exceeding `u128` never reaches code generation.
        let id = self
            .insert_expression(
                TypedExprKind::Literal(Literal::Numeric(value.unwrap_or(u128::MAX))),
                span,
                ty,
            )
            .await;

        // The literal's type may still be an inference variable here, so its
        // range is checked once every type has been inferred.
        self.require_numeric_literal_range(id, digits, value);

        id
    }
}

/// Returns the integer type that a numeric literal suffix fixes.
const fn suffix_integer(suffix: &NumericSuffix) -> Integer {
    match suffix {
        NumericSuffix::I8(_) => Integer::Int8,
        NumericSuffix::I16(_) => Integer::Int16,
        NumericSuffix::I32(_) => Integer::Int32,
        NumericSuffix::I64(_) => Integer::Int64,
        NumericSuffix::Isize(_) => Integer::Isize,
        NumericSuffix::U8(_) => Integer::Uint8,
        NumericSuffix::U16(_) => Integer::Uint16,
        NumericSuffix::U32(_) => Integer::Uint32,
        NumericSuffix::U64(_) => Integer::Uint64,
        NumericSuffix::Usize(_) => Integer::Usize,
    }
}
