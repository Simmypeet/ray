//! Diagnostics emitted while building semantic elements.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{get_all_symbol_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::poly_var;

use crate::build::{DiagnosticKey, ObligationKey};

/// Retrieves all rendered semantic-element diagnostics for a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct SingleRenderedKey {
    /// The symbol whose semantic-element diagnostics should be rendered.
    symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
#[expect(clippy::cognitive_complexity)]
async fn single_rendered_executor(
    &SingleRenderedKey { symbol_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let mut rendered = Vec::new();
    let mut obligations = Vec::new();
    let kind = engine.get_symbol_kind(symbol_id).await;

    if kind.has_effect_row_annotation() {
        let effect_row_key = rayc_semantic_element::effect_row::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(effect_row_key)).await;
        let generated = engine.query(&ObligationKey::new(effect_row_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind.has_parameter_list() {
        let parameter_key = rayc_semantic_element::parameter::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(parameter_key)).await;
        let generated = engine.query(&ObligationKey::new(parameter_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind.has_return_type() {
        let return_type_key = rayc_semantic_element::return_type::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(return_type_key)).await;
        let generated = engine.query(&ObligationKey::new(return_type_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind == rayc_symbol::symbol_kind::SymbolKind::Instance {
        let instance_key = rayc_semantic_element::instance_trait_ref::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(instance_key)).await;
        let generated = engine.query(&ObligationKey::new(instance_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind == rayc_symbol::symbol_kind::SymbolKind::InstanceDef {
        let instance_def_key = rayc_semantic_element::instance_def::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(instance_def_key)).await;
        let generated = engine.query(&ObligationKey::new(instance_def_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    // If the symbol owns polymorphic variables, query for their diagnostics and
    // render them.
    if kind.has_poly_var_map() {
        let key = poly_var::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(key)).await;
        let generated = engine.query(&ObligationKey::new(key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    // Solve together after collecting all semantic elements for this symbol.
    rendered.extend(solve_obligations(obligations, engine).await);
    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static SINGLE_RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<SingleRenderedKey, SingleRenderedExecutor>();

/// Retrieves all rendered semantic-element diagnostics for a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Interned<[Rendered<ByteIndex>]>]>)]
pub struct RenderedKey {
    /// The target whose semantic-element diagnostics should be rendered.
    pub target_id: TargetID,
}

#[executor(config = Config)]
async fn rendered_executor(
    &RenderedKey { target_id }: &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Interned<[Rendered<ByteIndex>]>]> {
    let mut rendered_by_def = Vec::new();
    let ids = engine.get_all_symbol_ids(target_id).await;

    for id in ids.iter().copied().map(|x| target_id.make_global(x)) {
        rendered_by_def.push(engine.query(&SingleRenderedKey { symbol_id: id }).await);
    }

    engine.intern_unsized(rendered_by_def)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();

/// Solve together after construction, retaining each source obligation for
/// diagnostics.
async fn solve_obligations(
    obligations: Vec<rayc_resolution::Obligation>,
    engine: &TrackedEngine,
) -> Vec<Rendered<ByteIndex>> {
    use rayc_solver::ty_relate::Step;
    use rayc_type::{
        reduce::Reduce,
        subst::{Subst, Substitutable},
    };

    let mut solver = rayc_solver::Solver::new(engine.clone());
    let mut constraints = Vec::new();
    let mut failed = std::collections::BTreeSet::new();
    // Expand trait checks only after all semantic elements are available.
    for (index, obligation) in obligations.iter().enumerate() {
        match obligation {
            rayc_resolution::Obligation::TraitRefCheck(check) => {
                match solver.entail_instance_trait_ref(check.constraint()).await {
                    Ok(Step::Derived(derived)) => constraints
                        .extend(derived.into_iter().map(|derived| (index, derived.ty_relate))),
                    Ok(Step::NoProgress) | Err(_) => {
                        failed.insert(index);
                    }
                    Ok(Step::Subst(_)) => unreachable!("trait checks only derive type relations"),
                }
            }
        }
    }
    let mut subst = Subst::new_empty();
    let mut residual = Vec::new();
    while let Some((index, constraint)) = constraints.pop() {
        match solver.entail_ty_relate(&constraint).await {
            Ok(Step::Derived(derived)) => {
                constraints.extend(derived.into_iter().map(|derived| (index, derived.ty_relate)));
            }
            Ok(Step::Subst(new)) => {
                subst.compose(&new, engine);
                constraints.append(&mut residual);
                for (_, constraint) in &mut constraints {
                    constraint.apply_in_place(&new, engine);
                }
            }
            Ok(Step::NoProgress) => {
                if let Some(reduced) = constraint.reduce(engine) {
                    constraints.push((index, reduced));
                } else {
                    residual.push((index, constraint));
                }
            }
            Err(_) => {
                failed.insert(index);
            }
        }
    }
    failed.extend(residual.into_iter().map(|(index, _)| index));
    let mut rendered = Vec::new();
    for index in failed {
        match &obligations[index] {
            rayc_resolution::Obligation::TraitRefCheck(check) => {
                rendered.push(check.apply_subst_or_clone(&subst, engine).report(engine).await);
            }
        }
    }
    rendered
}
