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

use crate::{
    build::{DiagnosticKey, ObligationKey},
    obligation::solve_obligations,
    variance::UnusedLifetimeKey,
};

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
#[expect(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn single_rendered_executor(
    &SingleRenderedKey { symbol_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let mut rendered = Vec::new();
    let mut obligations = Vec::new();
    let kind = engine.get_symbol_kind(symbol_id).await;

    if kind.has_where_clause() {
        let where_clause_key = rayc_type::where_clause::DeclaredKey { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(where_clause_key)).await;
        let generated = engine.query(&ObligationKey::new(where_clause_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

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
        let instance_key = rayc_type::trait_ref::InstanceTraitRefKey { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(instance_key)).await;
        let generated = engine.query(&ObligationKey::new(instance_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind == rayc_symbol::symbol_kind::SymbolKind::MarkerImplementation {
        let marker_implementation_key =
            rayc_semantic_element::marker_implementation::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(marker_implementation_key)).await;
        // The head's obligations are not checked: the implementation assumes
        // them as implied predicates of its where clause.
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind == rayc_symbol::symbol_kind::SymbolKind::Strut {
        let key = rayc_semantic_element::struct_body::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(key)).await;
        let generated = engine.query(&ObligationKey::new(key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if matches!(
        kind,
        rayc_symbol::symbol_kind::SymbolKind::InstanceDef
            | rayc_symbol::symbol_kind::SymbolKind::InstanceType
    ) {
        let conformance_key = crate::instance_member::ConformanceKey { symbol_id };
        for diagnostic in engine.query(&conformance_key).await.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
        let instance_member_key = rayc_type::instance_member::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(instance_member_key)).await;
        let generated = engine.query(&ObligationKey::new(instance_member_key)).await;

        obligations.extend(generated.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind == rayc_symbol::symbol_kind::SymbolKind::InstanceType {
        let key = rayc_type::type_definition::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(key)).await;
        obligations.extend(engine.query(&ObligationKey::new(key)).await.iter().cloned());
        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    // Structs and effects report the lifetime parameters that their variance
    // shows to be unused.
    if kind.has_variance_map() {
        for diagnostic in engine.query(&UnusedLifetimeKey { symbol_id }).await.iter() {
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
    rendered.extend(solve_obligations(obligations, symbol_id, engine).await);
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
