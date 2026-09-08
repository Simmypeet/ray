use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row,
    instance_def::{InstanceDef, Key},
    instance_trait_ref::get_instance_trait_ref,
    parameter::get_parameter_map,
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    member::get_member_by_name,
    name::get_name,
    parent::get_parent_global,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    build::{Build, Output},
    register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct IncompatibleDefinition {
    name: Interned<str>,
    instance_def_span: RelativeSpan,
    trait_def_span: RelativeSpan,
}

impl Report for IncompatibleDefinition {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("instance method `{}` has an incompatible signature", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.instance_def_span).await)
                    .message("this signature does not match the trait method")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_def_span).await)
                    .message("the trait method is declared here")
                    .build(),
            ])
            .build()
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
    trait_def_id: GlobalSymbolID,
    instance_def_id: GlobalSymbolID,
) -> Option<Subst> {
    let trait_poly_vars = engine.get_poly_var_map(trait_def_id).await;
    let instance_poly_vars = engine.get_poly_var_map(instance_def_id).await;

    if trait_poly_vars.len() != instance_poly_vars.len() {
        return None;
    }

    let mut substitution =
        engine.build_subst_from_args(trait_ref.trait_id(), trait_ref.args()).await;

    // Invariant: corresponding trait and instance definitions discover their
    // local polymorphic variables in the same semantic order: first occurrence
    // in explicit parameter types, followed by given parameters in declaration
    // order. This makes positional pairing independent of variable names while
    // preserving alpha-equivalence between the two signatures.
    let local_substitution = trait_poly_vars
        .iter()
        .zip(instance_poly_vars.iter())
        .map(|((trait_id, _), (instance_id, _))| {
            (
                GlobalPolyVarID::new(trait_def_id, trait_id),
                Ty::new_poly_var(GlobalPolyVarID::new(instance_def_id, instance_id), engine),
            )
        })
        .collect();
    substitution.compose(&local_substitution, engine);

    for ((_, trait_poly_var), (_, instance_poly_var)) in
        trait_poly_vars.iter().zip(instance_poly_vars.iter())
    {
        if trait_poly_var.kind() != instance_poly_var.kind() {
            return None;
        }

        let trait_constraint = trait_poly_var.trait_ref().map(|trait_ref| {
            TraitRef::new(
                trait_ref.trait_id(),
                trait_ref.args().apply_subst_or_clone(&substitution, engine),
            )
        });

        // TODO: we should create and use dedicated type equivalence checking instead of
        // relying syntactic equality.
        if trait_constraint.as_ref() != instance_poly_var.trait_ref() {
            return None;
        }
    }

    Some(substitution)
}

async fn signatures_are_compatible(
    engine: &TrackedEngine,
    trait_def_id: GlobalSymbolID,
    instance_def_id: GlobalSymbolID,
    substitution: &Subst,
) -> bool {
    let trait_parameters = engine.get_parameter_map(trait_def_id).await;
    let instance_parameters = engine.get_parameter_map(instance_def_id).await;
    if trait_parameters.len() != instance_parameters.len() {
        return false;
    }

    for ((_, trait_parameter), (_, instance_parameter)) in
        trait_parameters.iter().zip(instance_parameters.iter())
    {
        if contains_error(trait_parameter.ty()) || contains_error(instance_parameter.ty()) {
            continue;
        }

        // TODO: we should create and use dedicated type equivalence checking instead of
        // relying syntactic equality.
        if trait_parameter.ty().apply_subst_or_clone(substitution, engine)
            != *instance_parameter.ty()
        {
            return false;
        }
    }

    let trait_return = engine.get_return_type(trait_def_id).await;
    let instance_return = engine.get_return_type(instance_def_id).await;

    // TODO: we should create and use dedicated type equivalence checking instead of
    // relying syntactic equality.
    if !contains_error(&trait_return)
        && !contains_error(&instance_return)
        && trait_return.apply_subst_or_clone(substitution, engine) != instance_return
    {
        return false;
    }

    // TODO: we should create and use dedicated type equivalence checking instead of
    // relying syntactic equality.
    let trait_effect = engine.get_effect_row(trait_def_id).await;
    let instance_effect = engine.get_effect_row(instance_def_id).await;
    contains_error(&trait_effect)
        || contains_error(&instance_effect)
        || trait_effect.apply_subst_or_clone(substitution, engine) == instance_effect
}

impl Build for Key {
    type Diagnostic = IncompatibleDefinition;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();

        let instance_id = engine
            .get_parent_global(symbol_id)
            .await
            .expect("an instance definition symbol should have a parent instance");
        let trait_ref = engine
            .get_instance_trait_ref(instance_id)
            .await
            .expect("an instance definition should only be built for a resolved instance");

        let name = engine.get_name(symbol_id).await;

        let trait_def_id = engine
            .get_member_by_name(trait_ref.trait_id(), &name)
            .await
            .expect("an instance definition should correspond to a trait definition");
        let trait_def_span = engine
            .get_span(trait_def_id)
            .await
            .expect("a trait definition symbol should have a source span");

        // An associated type with the same name cannot supply a method signature.
        let substitution = if engine.get_symbol_kind(trait_def_id).await == SymbolKind::TraitDef {
            poly_var_substitution(engine, &trait_ref, trait_def_id, symbol_id).await
        } else {
            None
        };

        let compatible = if let Some(substitution) = &substitution {
            signatures_are_compatible(engine, trait_def_id, symbol_id, substitution).await
        } else {
            false
        };

        if !compatible {
            let instance_def_span = engine
                .get_span(symbol_id)
                .await
                .expect("an instance definition symbol should have a source span");
            diagnostics.receive(IncompatibleDefinition { name, instance_def_span, trait_def_span });
        }

        let definition = InstanceDef::new(
            trait_def_id,
            symbol_id,
            substitution.unwrap_or_else(Subst::new_empty),
        );
        Output::new_with(engine.intern(definition), diagnostics.into_vec(), [], engine)
    }
}

register_build!(Key);
