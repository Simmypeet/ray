//! Binds the construction of an enum value from one of its variants.

use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::path::Variant as ResolvedVariant;
use rayc_semantic_element::variant_payload_ty::get_variant_payload_ty;
use rayc_type::{subst::Substitutable, ty::Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, variant::Variant};

use crate::{
    diagnostic::{Diagnostic, MismatchedVariantPayload},
    tast_builder::TAstBuilder,
};

impl TAstBuilder {
    /// Builds the construction of `variant`, called with `arguments`, or not
    /// called at all when it is `None`: `Option.Some(value)` or
    /// `Option.None`.
    ///
    /// A variant that carries a payload takes exactly one argument, and one
    /// that does not is not called. Anything else is reported at `span`, the
    /// whole expression, which is then an error keeping the arguments.
    pub(super) async fn build_variant(
        &mut self,
        variant: &ResolvedVariant,
        arguments: Option<Vec<TypedExprID>>,
        span: RelativeSpan,
    ) -> TypedExprID {
        let enum_ = variant.parent();
        let subst = variant.substitution(self.engine()).await;
        let payload_ty = self
            .engine()
            .get_variant_payload_ty(variant.symbol_id())
            .await
            .map(|payload_ty| payload_ty.apply_subst_or_clone(&subst, self.engine()));

        // Check the arguments against the payload the variant declares.
        let payload = match (payload_ty, arguments) {
            (None, None) => None,
            (Some(payload_ty), Some(arguments)) if arguments.len() == 1 => {
                let argument = self.coerce(arguments[0], &payload_ty).await;
                self.push_variant_payload_constraint(&payload_ty, argument).await;
                Some(argument)
            }
            (payload_ty, arguments) => {
                self.push_diagnostic(Diagnostic::MismatchedVariantPayload(
                    MismatchedVariantPayload::builder()
                        .variant_id(variant.symbol_id())
                        .has_payload(payload_ty.is_some())
                        .maybe_found(arguments.as_ref().map(Vec::len))
                        .span(span)
                        .build(),
                ));

                return self
                    .push_error_expression_with_expression_children(
                        span,
                        arguments.into_iter().flatten(),
                    )
                    .await;
            }
        };

        // The value has the type of the enum, which carries the arguments the
        // variant is named with.
        let enum_ty = Ty::new_enum(enum_.symbol_id(), enum_.args().clone(), self.engine());
        self.insert_expression(Variant::new(variant.symbol_id(), payload), span, enum_ty).await
    }
}
