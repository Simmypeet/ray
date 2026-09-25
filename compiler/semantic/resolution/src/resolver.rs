//! Private resolution state and primitive resolution capabilities.

use std::fmt;

use bon::Builder;
use qbice::storage::intern::Interned;
use rayc_handler::Handler;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID, get_target_root_module_id,
    member::{get_member_by_name, try_get_members},
    parent::get_closest_module_id,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarMap, PolyVarStack, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{
        InferenceConstraint, Mutability, Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel,
    },
};

use crate::{
    Diagnostic, DuplicateGivenArgument, ExpectedEffect, ExpectedInstance, ExpectedMarker,
    ExpectedTrait, ExplicitTypeArgumentsNotAllowed, GenInferWithSpan, GivenArgumentNotFound,
    MissingGivenArgument, PathSegmentNotFound, PositionalGivenArgumentAfterNamed,
    TypeArgumentArityMismatch, TypeInferenceNotAllowed, TypeKindMismatch,
    lifetime::LifetimeElision,
};

/// Resolves syntax relative to a symbol and its polymorphic environment.
#[derive(Builder)]
pub struct Resolver<'a> {
    engine: &'a TrackedEngine,
    poly_var_stack: Option<&'a PolyVarStack>,

    building_poly_var_map: Option<&'a PolyVarMap>,

    site: GlobalSymbolID,
    handler: &'a dyn Handler<Diagnostic>,
    obligation_handler: &'a dyn Handler<crate::Obligation>,
    infer_gen: Option<&'a mut dyn GenInferWithSpan>,

    #[builder(default)]
    lifetime_elision: LifetimeElision,
}

impl fmt::Debug for Resolver<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resolver")
            .field("poly_vars", &self.poly_var_stack)
            .field("building_poly_va_map", &self.poly_var_stack)
            .field("site", &self.site)
            .field("has_infer_gen", &self.infer_gen.is_some())
            .field("lifetime_elision", &self.lifetime_elision)
            .finish_non_exhaustive()
    }
}

impl Resolver<'_> {
    pub(crate) async fn self_instance(&self) -> Option<rayc_type::ty::self_instance::SelfInstance> {
        use rayc_symbol::parent::get_parent_global;
        let parent = self.engine.get_parent_global(self.site).await?;
        (self.engine.get_symbol_kind(parent).await == SymbolKind::Trait)
            .then_some(rayc_type::ty::self_instance::SelfInstance::new(parent))
    }

    pub(crate) fn report_invalid_this_path(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::InvalidThisPath(crate::InvalidThisPath::new(span)));
    }

    pub(crate) fn report_named_trait_type_projection(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::NamedTraitTypeProjection(
            crate::NamedTraitTypeProjection::new(span),
        ));
    }

    pub(crate) fn report_missing_trait_type_declaration(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::MissingTraitTypeDeclaration(
            crate::MissingTraitTypeDeclaration::new(span),
        ));
    }

    pub(crate) fn report_expected_value_type(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::ExpectedValueType(crate::ExpectedValueType::new(span)));
    }

    pub fn report_unsupported_callable_type(&self, span: RelativeSpan) {
        self.handler
            .receive(Diagnostic::UnsupportedCallableType(crate::UnsupportedCallableType { span }));
    }

    pub(crate) fn report_too_many_given_arguments(&self, span: RelativeSpan, expected: usize) {
        self.handler.receive(Diagnostic::TooManyGivenArguments(crate::TooManyGivenArguments {
            span,
            expected,
        }));
    }

    pub(crate) const fn engine(&self) -> &TrackedEngine { self.engine }

    pub(crate) const fn site(&self) -> GlobalSymbolID { self.site }

    pub(crate) const fn lifetime_elision(&self) -> &LifetimeElision { &self.lifetime_elision }

    /// Replaces how this resolver treats elided lifetimes, returning the
    /// previous treatment so that it can be restored.
    pub const fn replace_lifetime_elision(&mut self, elision: LifetimeElision) -> LifetimeElision {
        std::mem::replace(&mut self.lifetime_elision, elision)
    }

    /// Returns whether this resolver may create inference variables, which is
    /// only the case inside a function body.
    pub(crate) const fn infers(&self) -> bool { self.infer_gen.is_some() }

    pub(crate) fn report(&self, diagnostic: Diagnostic) { self.handler.receive(diagnostic); }

    pub(crate) fn require_instance_trait_ref(
        &self,
        instance: Interned<Ty>,
        expected: TraitRef,
        span: RelativeSpan,
    ) {
        self.obligation_handler.receive(crate::Obligation::TraitRefCheck(
            crate::TraitRefCheck::new(
                rayc_type::constraint::instance_trait_ref::InstanceTraitRef::new(
                    instance, expected,
                ),
                span,
            ),
        ));
    }

    pub(crate) fn require_wf_check(&self, check: crate::WfCheck) {
        self.obligation_handler.receive(crate::Obligation::WfCheck(check));
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

    pub(crate) fn new_instance_type(&self, symbol_id: GlobalSymbolID, args: Args) -> Interned<Ty> {
        Ty::new_instance(symbol_id, args, self.engine)
    }

    pub(crate) fn new_struct_type(&self, symbol_id: GlobalSymbolID, args: Args) -> Interned<Ty> {
        Ty::new_struct(symbol_id, args, self.engine)
    }

    pub(crate) fn new_poly_var_type_from_id(&self, id: GlobalPolyVarID) -> Interned<Ty> {
        Ty::new_poly_var(id, self.engine)
    }

    pub(crate) fn search_poly_var(&self, name: &str) -> Option<GlobalPolyVarID> {
        self.building_poly_var_map
            .and_then(|x| x.find_by_name(name).map(|x| GlobalPolyVarID::new(self.site, x)))
            .or_else(|| self.poly_var_stack.and_then(|x| x.find_by_name(name)))
    }

    pub(crate) async fn poly_var_trait_ref(&self, id: GlobalPolyVarID) -> Option<TraitRef> {
        if let Some(building_poly_var_map) = self.building_poly_var_map
            && id.parent_id() == self.site
        {
            return building_poly_var_map.trait_ref_of(id.id()).cloned();
        }

        let poly_var_map = self.engine.get_poly_var_map(id.parent_id()).await;
        poly_var_map.trait_ref_of(id.id()).cloned()
    }

    pub(crate) fn new_inference_type(
        &mut self,
        kind: TyKind,
        span: RelativeSpan,
    ) -> Option<Interned<Ty>> {
        let infer_gen = self.infer_gen.as_deref_mut()?;
        Some(self.engine.intern(Ty::Inference(infer_gen.gen_infer(
            kind,
            InferenceConstraint::Any,
            span,
        ))))
    }

    pub(crate) fn new_instance_inference_type(
        &mut self,
        expected_trait_ref: &TraitRef,
        span: RelativeSpan,
    ) -> Option<Interned<Ty>> {
        let infer_gen = self.infer_gen.as_deref_mut()?;
        Some(
            self.engine
                .intern(Ty::Inference(infer_gen.gen_instance_infer(expected_trait_ref, span))),
        )
    }

    pub(crate) fn apply_subst_to_trait_ref(&self, trait_ref: &TraitRef, subst: &Subst) -> TraitRef {
        trait_ref.apply_subst_or_clone(subst, self.engine)
    }

    pub(crate) fn compose_subst(&self, subst: &mut Subst, new_subst: &Subst) {
        subst.compose(new_subst, self.engine);
    }

    pub(crate) async fn type_kind(&self, ty: &Interned<Ty>) -> TyKind {
        if let Ty::PolyVar(id) = &**ty
            && let Some(kind) = self.poly_var_stack.and_then(|x| x.kind_of(*id))
        {
            return kind;
        }

        if let Ty::PolyVar(id) = &**ty
            && let Some(building_poly_var_map) = self.building_poly_var_map
            && id.parent_id() == self.site
        {
            return building_poly_var_map.kind_of(id.id());
        }

        ty.kind_of(self.engine).await
    }

    pub(crate) async fn symbol_kind(&self, symbol_id: GlobalSymbolID) -> SymbolKind {
        self.engine.get_symbol_kind(symbol_id).await
    }

    pub(crate) async fn argument_parameters(
        &self,
        symbol_id: GlobalSymbolID,
    ) -> Option<Interned<PolyVarMap>> {
        if symbol_id == self.site
            && let Some(parameters) = self.building_poly_var_map
        {
            return Some(self.engine.intern(parameters.clone()));
        }
        let symbol_kind = self.engine.get_symbol_kind(symbol_id).await;
        if symbol_kind.has_poly_var_map() {
            Some(self.engine.get_poly_var_map(symbol_id).await)
        } else {
            None
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
            if let Some(local) = self.engine.get_member_by_name(closest_module_id, name).await {
                return Some(local);
            }
            // Only explicitly linked roots are visible, after local names.
            let targets = self.engine.query(&rayc_target::MapKey).await;
            let target_id = *targets.get(name)?;
            let linked =
                self.engine.query(&rayc_target::LinkKey { target_id: self.site.target_id }).await;
            if !linked.contains(&target_id) {
                return None;
            }
            Some(target_id.make_global(self.engine.get_target_root_module_id(target_id).await))
        }
    }

    pub(crate) fn report_expected_effect(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedEffect(ExpectedEffect::new(span, actual)));
    }

    pub(crate) fn report_expected_trait(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedTrait(ExpectedTrait::new(span, actual)));
    }

    pub(crate) fn report_expected_marker(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedMarker(ExpectedMarker::new(span, actual)));
    }

    pub(crate) fn report_expected_instance(&self, span: RelativeSpan, actual: SymbolKind) {
        self.handler.receive(Diagnostic::ExpectedInstance(ExpectedInstance::new(span, actual)));
    }

    pub(crate) fn report_positional_given_argument_after_named(&self, span: RelativeSpan) {
        self.handler.receive(Diagnostic::PositionalGivenArgumentAfterNamed(
            PositionalGivenArgumentAfterNamed { span },
        ));
    }

    pub(crate) fn report_given_argument_not_found(&self, name: Interned<str>, span: RelativeSpan) {
        self.handler
            .receive(Diagnostic::GivenArgumentNotFound(GivenArgumentNotFound { name, span }));
    }

    pub(crate) fn report_missing_given_argument(&self, name: Interned<str>, span: RelativeSpan) {
        self.handler.receive(Diagnostic::MissingGivenArgument(MissingGivenArgument { name, span }));
    }

    pub(crate) fn report_duplicate_given_argument(
        &self,
        name: Interned<str>,
        original_span: RelativeSpan,
        duplicate_span: RelativeSpan,
    ) {
        self.handler.receive(Diagnostic::DuplicateGivenArgument(DuplicateGivenArgument {
            name,
            original_span,
            duplicate_span,
        }));
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
