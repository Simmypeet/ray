//! Checks an instance member's given requirements, where clause and
//! signature against its trait member.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_handler::Storage;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_solver::Solver;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    name::get_name,
    parent::get_parent_global,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{get_effect_row_syntax, get_return_type_syntax, get_where_clause_syntax},
};
use rayc_type::{
    instance_member::{InstanceMember, Key},
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView},
    where_clause::get_where_clause,
};

use super::diagnostic::{Compatibility, Diagnostic, Mismatch};

async fn check_where_clause(
    engine: &TrackedEngine,
    member: &InstanceMember,
    instance_id: GlobalSymbolID,
    compatibility: &Compatibility<'_>,
) -> bool {
    use rayc_solver::givens::get_givens;

    use crate::build::DiagnosticKey;

    let trait_member_id = member.trait_member_id();
    let instance_member_id = member.instance_member_id();

    // Invalid clauses already emit their own resolution diagnostics. Their
    // recovery predicates must not cause additional conformance errors.
    if !engine
        .query(&DiagnosticKey::new(rayc_type::where_clause::DeclaredKey {
            symbol_id: trait_member_id,
        }))
        .await
        .is_empty()
        || !engine
            .query(&DiagnosticKey::new(rayc_type::where_clause::DeclaredKey {
                symbol_id: instance_member_id,
            }))
            .await
            .is_empty()
    {
        return false;
    }

    let trait_clause = engine.get_where_clause(trait_member_id).await;
    let instance_clause = engine.get_where_clause(instance_member_id).await;
    let substed_trait_predicates = trait_clause
        .declared()
        .map(|predicate| {
            predicate.kind().apply_subst_or_clone(member.poly_var_substitution(), engine)
        })
        .collect::<Vec<_>>();

    // Both directions receive the same ambient contract: the enclosing
    // instance assumptions and the enclosing trait assumptions translated to
    // the instance's variables. Member-local predicates are added only to the
    // direction in which they act as givens.
    let mut ambient = engine.get_givens(instance_id).await.to_vec();
    let trait_id = engine.get_parent_global(trait_member_id).await.expect("trait member parent");

    // Technically, we don't need to extend the ambient givens with the trait's
    // givens. This is because the instance's where clause must already entail the
    // trait's where clause, in other words, the trait's givens are already covered
    // by the instance's givens. However, we do this since users might write
    // malformed instances that don't entail the trait's where clause.
    ambient.extend(
        engine.get_givens(trait_id).await.iter().map(|predicate| {
            predicate.apply_subst_or_clone(member.poly_var_substitution(), engine)
        }),
    );

    // Each direction assumes its member's whole where clause, implied bounds
    // included, while only the declared predicates are compared.
    let mut trait_givens = ambient.clone();
    trait_givens.extend(trait_clause.predicates().map(|predicate| {
        predicate.kind().apply_subst_or_clone(member.poly_var_substitution(), engine)
    }));
    let mut trait_solver =
        Solver::with_givens(engine.clone(), instance_member_id, trait_givens).await;

    let mut instance_givens = ambient;
    instance_givens.extend(instance_clause.predicates().map(|predicate| predicate.kind().clone()));
    let mut instance_solver =
        Solver::with_givens(engine.clone(), instance_member_id, instance_givens).await;

    let mut compatible = true;
    for predicate in instance_clause.declared() {
        if !trait_solver.entails_predicate(predicate.kind()).await {
            compatibility.report_at(
                Mismatch::ExtraneousWhereClausePredicate { actual: predicate.kind().clone() },
                compatibility.trait_span(),
                predicate.span(),
            );
            compatible = false;
        }
    }

    let instance_clause_span = engine
        .get_where_clause_syntax(instance_member_id)
        .await
        .map_or(compatibility.instance_span(), |syntax| syntax.span());

    for (predicate, expected) in trait_clause.declared().zip(substed_trait_predicates) {
        if !instance_solver.entails_predicate(&expected).await {
            compatibility.report_at(
                Mismatch::MissingWhereClausePredicate { expected },
                predicate.span(),
                instance_clause_span,
            );
            compatible = false;
        }
    }

    compatible
}

fn contains_error(ty: &Ty) -> bool {
    ty.recursive_iter().any(|ty| {
        matches!(
            ty,
            Ty::Application(application) if matches!(application.view(), ApplicationView::Error)
        )
    })
}

async fn types_are_incompatible(
    solver: &mut Solver,
    expected: &Interned<Ty>,
    actual: &Interned<Ty>,
) -> bool {
    if contains_error(expected) || contains_error(actual) {
        return false;
    }
    !solver.eq_without_unify(expected, actual).await
}

async fn trait_refs_are_incompatible(
    solver: &mut Solver,
    expected: &TraitRef,
    actual: &TraitRef,
) -> bool {
    if expected.contains_error() || actual.contains_error() {
        return false;
    }
    !solver.trait_refs_eq_without_unify(expected, actual).await
}

/// Checks given requirements using the already-validated parameter
/// correspondence.
async fn check_given_requirements(
    engine: &TrackedEngine,
    solver: &mut Solver,
    member: &InstanceMember,
    compatibility: &Compatibility<'_>,
) -> bool {
    let trait_poly_vars = engine.get_poly_var_map(member.trait_member_id()).await;
    let instance_poly_vars = engine.get_poly_var_map(member.instance_member_id()).await;
    let mut compatible = true;

    // Counts and kinds were checked when constructing the correspondence. Only
    // requirement equivalence needs normalization, which may query correspondence.
    for (index, (trait_id, trait_poly_var)) in trait_poly_vars.iter().enumerate() {
        let Some(expected) = trait_poly_var.trait_ref() else { continue };
        let Some(instance_id) = member
            .poly_var_substitution()
            .get(&GlobalPolyVarID::new(member.trait_member_id(), trait_id))
            .and_then(|ty| ty.as_poly_var())
        else {
            continue;
        };
        let instance_poly_var = &instance_poly_vars[instance_id.id()];
        let Some(actual) = instance_poly_var.trait_ref() else { continue };

        let expected = expected.apply_subst_or_clone(member.poly_var_substitution(), engine);
        let expected = solver.normalize(&expected).await;
        let actual = solver.normalize(actual).await;
        if trait_refs_are_incompatible(solver, &expected, &actual).await {
            compatibility.report_at(
                Mismatch::InstanceParameterTraitRef { index, expected, actual },
                trait_poly_var.span(),
                instance_poly_var.span(),
            );
            compatible = false;
        }
    }
    compatible
}

async fn check_method_parameters(
    engine: &TrackedEngine,
    solver: &mut Solver,
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

        let expected = trait_parameter.ty().apply_subst_or_clone(substitution, engine);
        let expected = solver.normalize(&expected).await;
        let actual = solver.normalize(instance_parameter.ty()).await;
        if types_are_incompatible(solver, &expected, &actual).await {
            compatibility.report_at(
                Mismatch::ParameterType { index, expected, actual },
                trait_parameter.span().unwrap_or(compatibility.trait_span()),
                instance_parameter.span().unwrap_or(compatibility.instance_span()),
            );
        }
    }
}

async fn check_method_return_type(
    engine: &TrackedEngine,
    solver: &mut Solver,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    substitution: &Subst,
    compatibility: &Compatibility<'_>,
) {
    let trait_return =
        engine.get_return_type(trait_member_id).await.apply_subst_or_clone(substitution, engine);
    let trait_return = solver.normalize(&trait_return).await;
    let instance_return = engine.get_return_type(instance_member_id).await;
    let instance_return = solver.normalize(&instance_return).await;

    if types_are_incompatible(solver, &trait_return, &instance_return).await {
        compatibility.report_at(
            Mismatch::ReturnType { expected: trait_return, actual: instance_return },
            engine
                .get_return_type_syntax(trait_member_id)
                .await
                .map_or(compatibility.trait_span(), |syntax| syntax.span()),
            engine
                .get_return_type_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span(), |syntax| syntax.span()),
        );
    }
}

async fn check_method_effect_row(
    engine: &TrackedEngine,
    solver: &mut Solver,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    substitution: &Subst,
    compatibility: &Compatibility<'_>,
) {
    let trait_effect =
        engine.get_effect_row(trait_member_id).await.apply_subst_or_clone(substitution, engine);
    let trait_effect = solver.normalize(&trait_effect).await;
    let instance_effect = engine.get_effect_row(instance_member_id).await;
    let instance_effect = solver.normalize(&instance_effect).await;
    if types_are_incompatible(solver, &trait_effect, &instance_effect).await {
        compatibility.report_at(
            Mismatch::EffectRow { expected: trait_effect, actual: instance_effect },
            engine
                .get_effect_row_syntax(trait_member_id)
                .await
                .map_or(compatibility.trait_span(), |syntax| syntax.span()),
            engine
                .get_effect_row_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span(), |syntax| syntax.span()),
        );
    }
}

async fn check_method_signature(
    engine: &TrackedEngine,
    solver: &mut Solver,
    trait_member_id: GlobalSymbolID,
    instance_member_id: GlobalSymbolID,
    substitution: &Subst,
    compatibility: &Compatibility<'_>,
) {
    check_method_parameters(
        engine,
        solver,
        trait_member_id,
        instance_member_id,
        substitution,
        compatibility,
    )
    .await;
    check_method_return_type(
        engine,
        solver,
        trait_member_id,
        instance_member_id,
        substitution,
        compatibility,
    )
    .await;
    check_method_effect_row(
        engine,
        solver,
        trait_member_id,
        instance_member_id,
        substitution,
        compatibility,
    )
    .await;
}

/// Returns given-requirement and method-signature diagnostics after structural
/// correspondence has been checked. This query produces no semantic element or
/// obligations and reuses the substitution stored in [`InstanceMember`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Diagnostic]>)]
pub struct ConformanceKey {
    pub symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn conformance_executor(
    &ConformanceKey { symbol_id }: &ConformanceKey,
    engine: &TrackedEngine,
) -> Interned<[Diagnostic]> {
    use rayc_type::instance_member::get_instance_member;

    // A missing or structurally invalid correspondence has already been
    // diagnosed. Its recovery substitution cannot be used for conformance.
    if !engine.query(&crate::build::DiagnosticKey::new(Key { symbol_id })).await.is_empty() {
        return engine.intern_unsized([]);
    }
    let Some(member) = engine.get_instance_member(symbol_id).await else {
        return engine.intern_unsized([]);
    };

    let diagnostics = Storage::new();
    let trait_member_id = member.trait_member_id();
    let instance_id = engine.get_parent_global(symbol_id).await.expect("instance member parent");
    let compatibility = Compatibility::new(
        engine.get_name(symbol_id).await,
        engine.get_span(symbol_id).await.expect("member span"),
        engine.get_span(trait_member_id).await.expect("member span"),
        &diagnostics,
    );
    // An optional implementation ascription must agree with the trait contract.
    if engine.get_symbol_kind(symbol_id).await == SymbolKind::InstanceType {
        use rayc_symbol::syntax::get_kind_ascription_syntax;
        use rayc_type::associated_type_kind::get_associated_type_kind;
        if let Some(ascription) = engine.get_kind_ascription_syntax(symbol_id).await {
            let expected = engine.get_associated_type_kind(trait_member_id).await;
            let actual = crate::associated_type_kind::resolve_kind(Some(ascription.clone()));
            if actual != expected {
                compatibility.report_at(
                    Mismatch::AssociatedTypeKind { expected, actual },
                    compatibility.trait_span(),
                    ascription.span(),
                );
            }
        }
    }
    let mut solver = Solver::new(engine.clone(), symbol_id).await;
    let givens_compatible =
        check_given_requirements(engine, &mut solver, &member, &compatibility).await;
    let where_clause_compatible =
        check_where_clause(engine, &member, instance_id, &compatibility).await;
    if givens_compatible
        && where_clause_compatible
        && engine.get_symbol_kind(symbol_id).await == SymbolKind::InstanceDef
    {
        check_method_signature(
            engine,
            &mut solver,
            trait_member_id,
            symbol_id,
            member.poly_var_substitution(),
            &compatibility,
        )
        .await;
    }
    engine.intern_unsized(diagnostics.into_vec())
}

#[distributed_slice(RAY_PROGRAM)]
static CONFORMANCE_EXECUTOR: Registration<Config> =
    Registration::new::<ConformanceKey, ConformanceExecutor>();
