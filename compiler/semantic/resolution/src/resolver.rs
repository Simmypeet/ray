//! Private resolution state and primitive resolution capabilities.

use std::fmt;

use qbice::storage::intern::Interned;
use rayc_handler::Handler;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::{get_member_by_name, try_get_members},
    parent::get_closest_module_id,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    poly_var::{PolyVarStack, get_poly_var_map},
    ty::{
        InferenceConstraint, Mutability, Primitive, Ty, TyKind, args::Args,
        effect_row::EffectLabel, inference::GenInfer,
    },
};

use crate::{
    Diagnostic, ExpectedEffect, ExpectedTrait, ExplicitTypeArgumentsNotAllowed,
    PathSegmentNotFound, PolyVarNotFound, TypeArgumentArityMismatch, TypeInferenceNotAllowed,
    TypeKindMismatch,
};

/// Resolves syntax relative to a symbol and its polymorphic environment.
pub struct Resolver<'a> {
    engine: &'a TrackedEngine,
    poly_vars: &'a PolyVarStack,
    site: GlobalSymbolID,
    handler: &'a dyn Handler<Diagnostic>,
    infer_gen: Option<&'a mut dyn GenInfer>,
}

impl fmt::Debug for Resolver<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resolver")
            .field("poly_vars", &self.poly_vars)
            .field("site", &self.site)
            .field("has_infer_gen", &self.infer_gen.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> Resolver<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a TrackedEngine,
        poly_vars: &'a PolyVarStack,
        site: GlobalSymbolID,
        handler: &'a dyn Handler<Diagnostic>,
        infer_gen: Option<&'a mut dyn GenInfer>,
    ) -> Self {
        Self { engine, poly_vars, site, handler, infer_gen }
    }

    pub(crate) fn new_primitive_type(&self, primitive: Primitive) -> Interned<Ty> {
        Ty::new_primitive(primitive, self.engine)
    }

    pub(crate) fn new_error_type(&self, kind: TyKind) -> Interned<Ty> {
        Ty::new_error(kind, self.engine)
    }

    pub(crate) fn new_pointer_type(
        &self,
        pointee: Interned<Ty>,
        mutability: Mutability,
    ) -> Interned<Ty> {
        Ty::new_pointer(pointee, mutability, self.engine)
    }

    pub(crate) fn new_tuple_type(&self, elements: Vec<Interned<Ty>>) -> Interned<Ty> {
        Ty::new_tuple(self.engine.intern_unsized(elements), self.engine)
    }

    pub(crate) fn new_unit_type(&self) -> Interned<Ty> { Ty::new_unit(self.engine) }

    pub(crate) fn new_lambda_type(
        &self,
        parameters: Vec<Interned<Ty>>,
        return_type: Interned<Ty>,
        effect_row: Interned<Ty>,
    ) -> Interned<Ty> {
        Ty::new_lambda(parameters, return_type, effect_row, self.engine)
    }

    pub(crate) fn new_effect_row_type(
        &self,
        labels: Vec<Interned<EffectLabel>>,
        tail: Option<Interned<Ty>>,
    ) -> Interned<Ty> {
        Ty::new_effect_row(labels, tail, self.engine)
    }

    pub(crate) fn new_effect_label(
        &self,
        effect_id: GlobalSymbolID,
        args: Args,
    ) -> Interned<EffectLabel> {
        self.engine.intern(EffectLabel::new(effect_id, args))
    }

    pub(crate) fn new_args(&self, args: impl IntoIterator<Item = Interned<Ty>>) -> Args {
        Args::new(args, self.engine)
    }

    pub(crate) fn new_poly_var_type(
        &self,
        identifier: &rayc_syntax::Identifier,
        error_kind: TyKind,
    ) -> Interned<Ty> {
        let Some(id) = self.poly_vars.find_by_name(&identifier.kind.0) else {
            self.handler.receive(Diagnostic::PolyVarNotFound(PolyVarNotFound::new(
                identifier.kind.0.clone(),
                identifier.span(),
            )));
            return self.new_error_type(error_kind);
        };
        Ty::new_poly_var(id, self.engine)
    }

    pub(crate) async fn new_checked_poly_var_type(
        &self,
        identifier: &rayc_syntax::Identifier,
        expected: TyKind,
    ) -> Interned<Ty> {
        let ty = self.new_poly_var_type(identifier, expected);
        let actual = self.type_kind(&ty).await;
        if actual != expected {
            self.report_type_kind_mismatch(identifier.span(), expected, actual);
            return self.new_error_type(expected);
        }
        ty
    }

    pub(crate) fn new_inference_type(&mut self, kind: TyKind) -> Option<Interned<Ty>> {
        let infer_gen = self.infer_gen.as_deref_mut()?;
        Some(self.engine.intern(Ty::Inference(infer_gen.gen_infer(kind, InferenceConstraint::Any))))
    }

    pub(crate) async fn type_kind(&self, ty: &Interned<Ty>) -> TyKind {
        ty.kind_of(self.engine).await
    }

    pub(crate) async fn symbol_kind(&self, symbol_id: GlobalSymbolID) -> SymbolKind {
        self.engine.get_symbol_kind(symbol_id).await
    }

    pub(crate) async fn poly_var_kinds(&self, symbol_id: GlobalSymbolID) -> Vec<TyKind> {
        let symbol_kind = self.engine.get_symbol_kind(symbol_id).await;
        if symbol_kind.has_poly_var_map() {
            self.engine
                .get_poly_var_map(symbol_id)
                .await
                .iter()
                .map(|(_, poly_var)| poly_var.kind())
                .collect()
        } else {
            Vec::new()
        }
    }

    pub(crate) async fn find_path_symbol(
        &self,
        previous: Option<GlobalSymbolID>,
        name: &str,
    ) -> Option<GlobalSymbolID> {
        if let Some(previous) = previous {
            self.engine.try_get_members(previous).await.and_then(|members| {
                members.get_by_name(name).map(|member_id| previous.target_id.make_global(member_id))
            })
        } else {
            let closest_module_id = self.engine.get_closest_module_id(self.site).await;
            let closest_module_id = self.site.target_id.make_global(closest_module_id);
            self.engine.get_member_by_name(closest_module_id, name).await
        }
    }

    pub(crate) fn report_expected_effect(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedEffect(ExpectedEffect::new(span, actual)));
    }

    pub(crate) fn report_expected_trait(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedTrait(ExpectedTrait::new(span, actual)));
    }

    pub(crate) fn report_path_segment_not_found(&self, identifier: rayc_syntax::Identifier) {
        self.handler.receive(Diagnostic::PathSegmentNotFound(PathSegmentNotFound::new(
            identifier.kind.0,
            identifier.span,
        )));
    }

    pub(crate) fn report_type_inference_not_allowed(
        &self,
        identifier: &rayc_syntax::Identifier,
        expected: usize,
    ) {
        self.handler.receive(Diagnostic::TypeInferenceNotAllowed(TypeInferenceNotAllowed::new(
            identifier.kind.0.clone(),
            identifier.span(),
            expected,
        )));
    }

    pub(crate) fn report_explicit_type_arguments_not_allowed(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::ExplicitTypeArgumentsNotAllowed(
            ExplicitTypeArgumentsNotAllowed::new(span),
        ));
    }

    pub(crate) fn report_type_argument_arity_mismatch(
        &self,
        span: RelativeSpan,
        expected: usize,
        actual: usize,
    ) {
        self.handler.receive(Diagnostic::TypeArgumentArityMismatch(
            TypeArgumentArityMismatch::new(span, expected, actual),
        ));
    }

    pub(crate) fn report_type_kind_mismatch(
        &self,
        span: RelativeSpan,
        expected: TyKind,
        actual: TyKind,
    ) {
        self.handler
            .receive(Diagnostic::TypeKindMismatch(TypeKindMismatch::new(span, expected, actual)));
    }
}
