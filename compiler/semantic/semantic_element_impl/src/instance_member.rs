use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row, instance_trait_ref::get_instance_trait_ref,
    parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::get_member_by_name,
    name::{get_name, get_qualified_name},
    parent::get_parent_global,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{get_effect_row_syntax, get_return_type_syntax},
};
use rayc_type::{
    instance_member::{InstanceMember, Key},
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, TyKind, application::View as ApplicationView},
};

use crate::{
    build::{Build, Output},
    register_build,
};

/// The particular contract violated by an instance member.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Mismatch {
    ParameterCount { expected: usize, actual: usize },
    ParameterType { index: usize, expected: Interned<Ty>, actual: Interned<Ty> },
    ReturnType { expected: Interned<Ty>, actual: Interned<Ty> },
    EffectRow { expected: Interned<Ty>, actual: Interned<Ty> },
    PolyVarCount { expected: usize, actual: usize },
    PolyVarKind { index: usize, expected: TyKind, actual: TyKind },
    InstanceParameterTraitRef { index: usize, expected: TraitRef, actual: TraitRef },
    MemberKind { expected: SymbolKind, actual: SymbolKind },
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Diagnostic {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_span: RelativeSpan,
    mismatch: Mismatch,
}

const fn kind_name(kind: TyKind) -> &'static str {
    match kind {
        TyKind::Star => "type",
        TyKind::EffectRow => "effect row",
        TyKind::Instance => "instance",
    }
}

async fn display_trait_ref(reference: &TraitRef, engine: &TrackedEngine) -> String {
    let name = engine.get_qualified_name(reference.trait_id()).await;
    let mut args = Vec::new();
    for argument in reference.args().iter() {
        args.push(argument.display(engine).await.to_string());
    }
    format!("{name}[{}]", args.join(", "))
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let (problem, detail) = match &self.mismatch {
            Mismatch::ParameterCount { expected, actual } => (
                "parameter count mismatch".to_owned(),
                format!("expected {expected} parameters, found {actual}"),
            ),
            Mismatch::ParameterType { index, expected, actual } => (
                format!("parameter {} type mismatch", index + 1),
                format!(
                    "expected `{}`, found `{}`",
                    expected.display(engine).await,
                    actual.display(engine).await
                ),
            ),
            Mismatch::ReturnType { expected, actual } => (
                "return type mismatch".to_owned(),
                format!(
                    "expected `{}`, found `{}`",
                    expected.display(engine).await,
                    actual.display(engine).await
                ),
            ),
            Mismatch::EffectRow { expected, actual } => (
                "effect row mismatch".to_owned(),
                format!(
                    "expected `{}`, found `{}`",
                    expected.display(engine).await,
                    actual.display(engine).await
                ),
            ),
            Mismatch::PolyVarCount { expected, actual } => (
                "polymorphic variable count mismatch".to_owned(),
                format!("expected {expected} polymorphic variables, found {actual}"),
            ),
            Mismatch::PolyVarKind { index, expected, actual } => (
                format!("polymorphic variable {} kind mismatch", index + 1),
                format!("expected {}, found {}", kind_name(*expected), kind_name(*actual)),
            ),
            Mismatch::InstanceParameterTraitRef { index, expected, actual } => (
                format!(
                    "instance parameter at polymorphic position {} trait reference mismatch",
                    index + 1
                ),
                format!(
                    "expected `{}`, found `{}`",
                    display_trait_ref(expected, engine).await,
                    display_trait_ref(actual, engine).await
                ),
            ),
            Mismatch::MemberKind { expected, actual } => (
                "member kind mismatch".to_owned(),
                format!("expected an implementation of {}, found {}", expected.str(), actual.str()),
            ),
        };
        Rendered::builder()
            .message(format!("instance member `{}`: {problem}", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.instance_span).await)
                    .message(detail)
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_span).await)
                    .message("the corresponding trait declaration is here")
                    .build(),
            ])
            .build()
    }
}

struct Compatibility<'a> {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_span: RelativeSpan,
    diagnostics: &'a Storage<Diagnostic>,
}

impl Compatibility<'_> {
    fn report(&self, mismatch: Mismatch) {
        self.report_at(mismatch, self.trait_span, self.instance_span);
    }

    fn report_at(&self, mismatch: Mismatch, trait_span: RelativeSpan, instance_span: RelativeSpan) {
        self.diagnostics.receive(Diagnostic {
            name: self.name.clone(),
            trait_span,
            instance_span,
            mismatch,
        });
    }
}

fn contains_error(ty: &Ty) -> bool {
    ty.recursive_iter().any(|ty| {
        matches!(
            ty,
            Ty::Application(application) if matches!(application.view(), ApplicationView::Error)
        )
    })
}

async fn poly_var_substitution(
    engine: &TrackedEngine,
    trait_ref: &TraitRef,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    compatibility: &Compatibility<'_>,
) -> Option<Subst> {
    let trait_poly_vars = engine.get_poly_var_map(trait_member_id).await;
    let instance_poly_vars = engine.get_poly_var_map(instance_member_id).await;

    if trait_poly_vars.len() != instance_poly_vars.len() {
        compatibility.report(Mismatch::PolyVarCount {
            expected: trait_poly_vars.len(),
            actual: instance_poly_vars.len(),
        });
        return None;
    }

    let mut substitution =
        engine.build_subst_from_args(trait_ref.trait_id(), trait_ref.args()).await;

    // Invariant: corresponding trait and instance members discover their
    // local polymorphic variables in the same semantic order: first occurrence
    // in method parameter types, or declaration order for associated types,
    // followed by given parameters in declaration
    // order. This makes positional pairing independent of variable names while
    // preserving alpha-equivalence between the two signatures.
    let local_substitution = trait_poly_vars
        .iter()
        .zip(instance_poly_vars.iter())
        .map(|((trait_id, _), (instance_id, _))| {
            (
                GlobalPolyVarID::new(trait_member_id, trait_id),
                Ty::new_poly_var(GlobalPolyVarID::new(instance_member_id, instance_id), engine),
            )
        })
        .collect();
    substitution.compose(&local_substitution, engine);

    let mut compatible = true;
    for (index, ((_, trait_poly_var), (_, instance_poly_var))) in
        trait_poly_vars.iter().zip(instance_poly_vars.iter()).enumerate()
    {
        if trait_poly_var.kind() != instance_poly_var.kind() {
            compatibility.report_at(
                Mismatch::PolyVarKind {
                    index,
                    expected: trait_poly_var.kind(),
                    actual: instance_poly_var.kind(),
                },
                trait_poly_var.span(),
                instance_poly_var.span(),
            );
            compatible = false;
            continue;
        }

        let trait_constraint = trait_poly_var.trait_ref().map(|trait_ref| {
            TraitRef::new(
                trait_ref.trait_id(),
                trait_ref.args().apply_subst_or_clone(&substitution, engine),
            )
        });

        // TODO: we should create and use dedicated type equivalence checking instead of
        // relying syntactic equality.
        if let (Some(expected), Some(actual)) = (trait_constraint, instance_poly_var.trait_ref())
            && !expected.contains_error()
            && !actual.contains_error()
            && expected != *actual
        {
            compatibility.report_at(
                Mismatch::InstanceParameterTraitRef { index, expected, actual: actual.clone() },
                trait_poly_var.span(),
                instance_poly_var.span(),
            );
            compatible = false;
        }
    }

    compatible.then_some(substitution)
}

async fn check_method_signature(
    engine: &TrackedEngine,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    substitution: &Subst,
    compatibility: &Compatibility<'_>,
) {
    let trait_parameters = engine.get_parameter_map(trait_member_id).await;
    let instance_parameters = engine.get_parameter_map(instance_member_id).await;
    if trait_parameters.len() != instance_parameters.len() {
        compatibility.report(Mismatch::ParameterCount {
            expected: trait_parameters.len(),
            actual: instance_parameters.len(),
        });
    }

    for (index, ((_, trait_parameter), (_, instance_parameter))) in
        trait_parameters.iter().zip(instance_parameters.iter()).enumerate()
    {
        if contains_error(trait_parameter.ty()) || contains_error(instance_parameter.ty()) {
            continue;
        }

        // TODO: we should create and use dedicated type equivalence checking instead of
        // relying syntactic equality.
        let expected = trait_parameter.ty().apply_subst_or_clone(substitution, engine);
        if expected != *instance_parameter.ty() {
            compatibility.report_at(
                Mismatch::ParameterType {
                    index,
                    expected,
                    actual: instance_parameter.ty().clone(),
                },
                trait_parameter.span().unwrap_or(compatibility.trait_span),
                instance_parameter.span().unwrap_or(compatibility.instance_span),
            );
        }
    }

    let trait_return = engine.get_return_type(trait_member_id).await;
    let instance_return = engine.get_return_type(instance_member_id).await;

    // TODO: we should create and use dedicated type equivalence checking instead of
    // relying syntactic equality.
    if !contains_error(&trait_return)
        && !contains_error(&instance_return)
        && trait_return.apply_subst_or_clone(substitution, engine) != instance_return
    {
        compatibility.report_at(
            Mismatch::ReturnType {
                expected: trait_return.apply_subst_or_clone(substitution, engine),
                actual: instance_return,
            },
            engine
                .get_return_type_syntax(trait_member_id)
                .await
                .map_or(compatibility.trait_span, |syntax| syntax.span()),
            engine
                .get_return_type_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span, |syntax| syntax.span()),
        );
    }

    // TODO: we should create and use dedicated type equivalence checking instead of
    // relying syntactic equality.
    let trait_effect = engine.get_effect_row(trait_member_id).await;
    let instance_effect = engine.get_effect_row(instance_member_id).await;
    if !contains_error(&trait_effect)
        && !contains_error(&instance_effect)
        && trait_effect.apply_subst_or_clone(substitution, engine) != instance_effect
    {
        compatibility.report_at(
            Mismatch::EffectRow {
                expected: trait_effect.apply_subst_or_clone(substitution, engine),
                actual: instance_effect,
            },
            engine
                .get_effect_row_syntax(trait_member_id)
                .await
                .map_or(compatibility.trait_span, |syntax| syntax.span()),
            engine
                .get_effect_row_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span, |syntax| syntax.span()),
        );
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();

        let instance_id = engine
            .get_parent_global(symbol_id)
            .await
            .expect("an instance member symbol should have a parent instance");
        let trait_ref = engine
            .get_instance_trait_ref(instance_id)
            .await
            .expect("an instance member should only be built for a resolved instance");

        let name = engine.get_name(symbol_id).await;

        let trait_member_id = engine
            .get_member_by_name(trait_ref.trait_id(), &name)
            .await
            .expect("an instance member should correspond to a trait member");
        let trait_member_span = engine
            .get_span(trait_member_id)
            .await
            .expect("a trait member symbol should have a source span");

        // Only corresponding member kinds share a polymorphic-variable mapping.
        let instance_kind = engine.get_symbol_kind(symbol_id).await;
        let trait_kind = engine.get_symbol_kind(trait_member_id).await;
        let matching_kinds = matches!(
            (trait_kind, instance_kind),
            (SymbolKind::TraitDef, SymbolKind::InstanceDef)
                | (SymbolKind::TraitType, SymbolKind::InstanceType)
        );
        let compatibility = Compatibility {
            name,
            trait_span: trait_member_span,
            instance_span: engine
                .get_span(symbol_id)
                .await
                .expect("an instance member should have a span"),
            diagnostics: &diagnostics,
        };
        let substitution = if matching_kinds {
            poly_var_substitution(engine, &trait_ref, trait_member_id, symbol_id, &compatibility)
                .await
        } else {
            compatibility
                .report(Mismatch::MemberKind { expected: trait_kind, actual: instance_kind });
            None
        };

        // Method checks depend on a valid correspondence between polymorphic variables.
        if instance_kind == SymbolKind::InstanceDef
            && let Some(substitution) = &substitution
        {
            check_method_signature(
                engine,
                trait_member_id,
                symbol_id,
                substitution,
                &compatibility,
            )
            .await;
        }

        let definition = InstanceMember::new(
            trait_member_id,
            symbol_id,
            substitution.unwrap_or_else(Subst::new_empty),
        );
        Output::new_with(engine.intern(definition), diagnostics.into_vec(), [], engine)
    }
}

register_build!(Key);
