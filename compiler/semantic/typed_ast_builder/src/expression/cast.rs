use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::Cast as CastSyntax;
use rayc_type::ty::{InferenceConstraint, Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, cast::Cast};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, InvalidCastTarget},
    tast_builder::TAstBuilder,
};

impl Bind<CastSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: CastSyntax) -> TypedExprID {
        let Some(prefix) = syn.prefix() else {
            return self.push_error_expression(syn.span()).await;
        };

        // `x as int64 as float32` casts `x as int64` to `float32`.
        let mut operand = self.bind(prefix).await;
        for cast in syn.casts() {
            let span = self.span_of_expression(operand).join(&cast.span());
            let Some(target) = cast.ty() else {
                operand =
                    self.push_error_expression_with_expression_children(span, [operand]).await;
                continue;
            };

            let target = self.resolve_local_type_annotation(&target).await;
            operand = self.build_cast(operand, target, span).await;
        }

        operand
    }
}

impl TAstBuilder {
    /// Converts the operand to the `target` type. Both the operand and the
    /// target must have numeric types.
    async fn build_cast(
        &mut self,
        operand: TypedExprID,
        target: Interned<Ty>,
        span: RelativeSpan,
    ) -> TypedExprID {
        // An erroneous target has already been reported by its resolution.
        let is_numeric_target = target
            .as_primitive()
            .is_some_and(|primitive| primitive.satisfies_constraint(InferenceConstraint::Numeric));
        if !is_numeric_target {
            if !target.contains_error() {
                self.push_diagnostic(Diagnostic::InvalidCastTarget(
                    InvalidCastTarget::builder().target(target).span(span).build(),
                ));
            }
            return self.push_error_expression_with_expression_children(span, [operand]).await;
        }

        let operand_ty = self.new_numeric_type_inference();
        self.push_cast_operand_constraint(&operand_ty, operand).await;

        self.insert_expression(TypedExprKind::Cast(Cast::new(operand)), span, target).await
    }
}
