//! The functions that code generation starts from.

use rayc_mono_ir::MonoBodyInstance;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_all_def_with_body_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, lifetime::Lifetime},
};

/// The definitions of `target_id` that need no caller to choose their
/// arguments: those polymorphic over lifetimes at most.
///
/// Every other function is generated only once something reachable from these
/// roots refers to it, with the arguments that the reference chooses.
pub(crate) async fn root_definitions(
    engine: &TrackedEngine,
    target_id: TargetID,
    solver: &Solver,
) -> Vec<MonoBodyInstance> {
    let mut roots = Vec::new();
    for def_id in engine.get_all_def_with_body_ids(target_id).await.iter().copied() {
        let def_id = target_id.make_global(def_id);

        // an `extern def` is provided by the C environment, so it is only
        // generated, as a declaration, once something calls it
        if !is_lowered_from_body(engine.get_symbol_kind(def_id).await) {
            continue;
        }

        if let Some(root) = lifetime_erased_instance(engine, def_id, solver).await {
            roots.push(root);
        }
    }
    roots
}

/// The only instance of `def_id` when neither it nor any enclosing symbol
/// declares a polymorphic variable other than a lifetime.
///
/// Lifetimes do not affect code generation, so they are instantiated with the
/// erased lifetime; any other variable needs a caller to choose it.
pub(crate) async fn lifetime_erased_instance(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    solver: &Solver,
) -> Option<MonoBodyInstance> {
    let poly_vars = engine.get_enclosing_poly_var_maps(def_id).await;
    if !poly_vars.has_only_lifetimes() {
        return None;
    }

    let erased = Ty::new_lifetime(Lifetime::Erased, engine);
    let substitution =
        poly_vars.create_subst_from_args(std::iter::repeat_n(erased, poly_vars.all_poly_var_len()));
    Some(MonoBodyInstance::new_definition(def_id, substitution, solver).await)
}

/// Whether a symbol with a body is generated from that body, as opposed to
/// being provided by the C environment.
fn is_lowered_from_body(kind: SymbolKind) -> bool {
    match kind {
        SymbolKind::Def | SymbolKind::InstanceDef => true,
        SymbolKind::ExternDef => false,

        SymbolKind::Effect
        | SymbolKind::EffectOperation
        | SymbolKind::Enum
        | SymbolKind::Instance
        | SymbolKind::Marker
        | SymbolKind::MarkerImplementation
        | SymbolKind::Module
        | SymbolKind::Struct
        | SymbolKind::Trait
        | SymbolKind::TraitType
        | SymbolKind::InstanceType
        | SymbolKind::TraitDef
        | SymbolKind::Variant => {
            panic!("non-definition symbol returned by the definition inventory")
        }
    }
}
