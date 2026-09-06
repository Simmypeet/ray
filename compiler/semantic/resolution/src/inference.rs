//! Inference generation with the source location of the inferred argument.

use rayc_lexical::tree::RelativeSpan;
use rayc_type::{
    trait_ref::TraitRef,
    ty::{InferenceConstraint, TyKind, inference::Inference},
};

/// Generates inference variables while resolving source syntax.
pub trait GenInferWithSpan: Send + Sync {
    /// Generates an ordinary inference variable at the supplied source span.
    fn gen_infer(
        &mut self,
        kind: TyKind,
        constraint: InferenceConstraint,
        span: RelativeSpan,
    ) -> Inference;

    /// Generates a dictionary inference for an omitted given argument.
    fn gen_instance_infer(
        &mut self,
        expected_trait_ref: &TraitRef,
        span: RelativeSpan,
    ) -> Inference;

    /// Generates an unconstrained effect-row inference at the supplied span.
    fn gen_effect_row_infer(&mut self, span: RelativeSpan) -> Inference {
        self.gen_infer(TyKind::EffectRow, InferenceConstraint::Any, span)
    }
}
