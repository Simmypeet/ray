use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    effect_row::get_effect_row, instance_trait_ref::get_instance_trait_ref,
    parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_solver::Solver;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::get_member_by_name,
    name::{get_name, get_qualified_name},
    parent::get_parent_global,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{get_effect_row_syntax, get_return_type_syntax, get_where_clause_syntax},
};
use rayc_type::{
    instance_member::{InstanceMember, Key},
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{
        Ty, TyKind, application::View as ApplicationView, args::Args, self_instance::SelfInstance,
    },
    where_clause::{PredicateKind, get_where_clause},
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
    MissingWhereClausePredicate { expected: PredicateKind },
    ExtraneousWhereClausePredicate { actual: PredicateKind },
    MemberKind { expected: SymbolKind, actual: SymbolKind },
    AssociatedTypeKind { expected: TyKind, actual: TyKind },
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

async fn display_predicate(predicate: &PredicateKind, engine: &TrackedEngine) -> String {
    match predicate {
        PredicateKind::AssociatedTypeEquality(equality) => format!(
            "`{}` must equal `{}`",
            equality.left().display(engine).await,
            equality.right().display(engine).await
        ),
    }
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let related_message = match &self.mismatch {
            Mismatch::MissingWhereClausePredicate { .. } => {
                "the required trait predicate is declared here"
            }
            Mismatch::ExtraneousWhereClausePredicate { .. } => {
                "the corresponding trait member declares no equivalent requirement"
            }
            Mismatch::ParameterCount { .. }
            | Mismatch::ParameterType { .. }
            | Mismatch::ReturnType { .. }
            | Mismatch::EffectRow { .. }
            | Mismatch::PolyVarCount { .. }
            | Mismatch::PolyVarKind { .. }
            | Mismatch::InstanceParameterTraitRef { .. }
            | Mismatch::AssociatedTypeKind { .. }
            | Mismatch::MemberKind { .. } => "the corresponding trait declaration is here",
        };
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
            Mismatch::MissingWhereClausePredicate { expected } => (
                "missing where-clause predicate".to_owned(),
                format!("missing requirement: {}", display_predicate(expected, engine).await),
            ),
            Mismatch::ExtraneousWhereClausePredicate { actual } => (
                "extraneous where-clause predicate".to_owned(),
                format!("extra requirement: {}", display_predicate(actual, engine).await),
            ),
            Mismatch::AssociatedTypeKind { expected, actual } => (
                "associated type kind mismatch".to_owned(),
                format!("expected {}, found {}", kind_name(*expected), kind_name(*actual)),
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
                    .message(related_message)
                    .build(),
            ])
            .build()
    }
}

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
    let trait_key = rayc_type::where_clause::Key { symbol_id: trait_member_id };
    let instance_key = rayc_type::where_clause::Key { symbol_id: instance_member_id };
    if !engine.query(&DiagnosticKey::new(trait_key)).await.is_empty()
        || !engine.query(&DiagnosticKey::new(instance_key)).await.is_empty()
    {
        return false;
    }

    let trait_clause = engine.get_where_clause(trait_member_id).await;
    let instance_clause = engine.get_where_clause(instance_member_id).await;
    let substed_trait_predicates = trait_clause
        .iter()
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

    let mut trait_givens = ambient.clone();
    trait_givens.extend(substed_trait_predicates.iter().cloned());
    let mut trait_solver = Solver::with_givens(engine.clone(), instance_member_id, trait_givens);

    let mut instance_givens = ambient;
    instance_givens.extend(instance_clause.iter().map(|predicate| predicate.kind().clone()));
    let mut instance_solver =
        Solver::with_givens(engine.clone(), instance_member_id, instance_givens);

    let mut compatible = true;
    for predicate in instance_clause.iter() {
        if !trait_solver.entails_predicate(predicate.kind()).await {
            compatibility.report_at(
                Mismatch::ExtraneousWhereClausePredicate { actual: predicate.kind().clone() },
                compatibility.trait_span,
                predicate.span(),
            );
            compatible = false;
        }
    }

    let instance_clause_span = engine
        .get_where_clause_syntax(instance_member_id)
        .await
        .map_or(compatibility.instance_span, |syntax| syntax.span());

    for (predicate, expected) in trait_clause.iter().zip(substed_trait_predicates) {
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

    let instance_id = engine.get_parent_global(instance_member_id).await.expect("instance parent");
    let instance_parameters = engine.get_poly_var_map(instance_id).await;
    let identity = Args::new(
        instance_parameters
            .iter()
            .map(|(id, _)| Ty::new_poly_var(GlobalPolyVarID::new(instance_id, id), engine)),
        engine,
    );
    substitution.insert(
        SelfInstance::new(trait_ref.trait_id()),
        Ty::new_instance(instance_id, identity, engine),
    );

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
        }
    }

    compatible.then_some(substitution)
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
    for (index, ((_, trait_poly_var), (_, instance_poly_var))) in
        trait_poly_vars.iter().zip(instance_poly_vars.iter()).enumerate()
    {
        if let (Some(expected), Some(actual)) =
            (trait_poly_var.trait_ref(), instance_poly_var.trait_ref())
        {
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
                trait_parameter.span().unwrap_or(compatibility.trait_span),
                instance_parameter.span().unwrap_or(compatibility.instance_span),
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
                .map_or(compatibility.trait_span, |syntax| syntax.span()),
            engine
                .get_return_type_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span, |syntax| syntax.span()),
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
                .map_or(compatibility.trait_span, |syntax| syntax.span()),
            engine
                .get_effect_row_syntax(instance_member_id)
                .await
                .map_or(compatibility.instance_span, |syntax| syntax.span()),
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

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();

        let instance_id = engine
            .get_parent_global(symbol_id)
            .await
            .expect("an instance member symbol should have a parent instance");
        let Some(trait_ref) = engine.get_instance_trait_ref(instance_id).await else {
            return Output::new_with(None, [], [], engine);
        };
        let name = engine.get_name(symbol_id).await;
        let Some(trait_member_id) = engine.get_member_by_name(trait_ref.trait_id(), &name).await
        else {
            return Output::new_with(None, [], [], engine);
        };
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

        let definition = InstanceMember::new(
            trait_member_id,
            symbol_id,
            substitution.unwrap_or_else(Subst::new_empty),
        );
        Output::new_with(Some(engine.intern(definition)), diagnostics.into_vec(), [], engine)
    }
}

register_build!(Key);

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
    let compatibility = Compatibility {
        name: engine.get_name(symbol_id).await,
        instance_span: engine.get_span(symbol_id).await.expect("member span"),
        trait_span: engine.get_span(trait_member_id).await.expect("member span"),
        diagnostics: &diagnostics,
    };
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
                    compatibility.trait_span,
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
