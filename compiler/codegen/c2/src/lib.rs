//! Worklist-driven C code generation from [`rayc_mono_ir::MonoIR`].
//!
//! The backend consumes independently lowered definition fragments. Calls to
//! other fragments schedule them on a worklist, and every aggregate type the
//! generated code mentions is collected so its `struct` can be defined before
//! use.
//!
//! The crate is organized in layers:
//!
//! - [`c`] renders C syntax (names, types, expressions) as allocation-free
//!   [`std::fmt::Display`] adaptors;
//! - `worklist`, `functions`, and `aggregates` hold the program-wide state
//!   discovered while generating;
//! - `collect` discovers a fragment's dependencies, `print` writes its
//!   functions, and `generator` drives both;
//! - `unit` assembles the final translation unit.

use std::io::{self, Write};

use rayc_mono_ir::{MonoDefInstance, MonoIR};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_all_def_with_body_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::Subst,
    ty::{Ty, TyKind, lifetime::Lifetime},
};

use crate::generator::Generator;

mod aggregates;
mod c;
mod collect;
mod functions;
mod generator;
mod print;
mod unit;
mod worklist;

/// Options controlling the contents of a generated C translation unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CTranslationUnitOptions {
    entry_point: Option<GlobalSymbolID>,
}

impl CTranslationUnitOptions {
    /// Creates options for an ordinary translation unit without a C entry
    /// point wrapper.
    #[must_use]
    pub const fn ordinary() -> Self { Self { entry_point: None } }

    /// Creates options for an executable translation unit targeting the given
    /// validated Ray entry point.
    #[must_use]
    pub const fn executable(entry_point: GlobalSymbolID) -> Self {
        Self { entry_point: Some(entry_point) }
    }
}

/// Generates a C translation unit for all definition instantiations reachable
/// from the definitions in `target_id` that are polymorphic over lifetimes at
/// most.
pub async fn write_c_translation_unit(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    output: &mut impl Write,
) -> io::Result<()> {
    // Every root key is normalized with the same solver.
    let solver = Solver::without_givens(engine.clone()).await;
    let roots = root_definitions(engine, target_id, &solver).await;
    let entry_point = match options.entry_point {
        Some(entry_point) => Some(
            lifetime_erased_instance(engine, entry_point, &solver)
                .await
                .expect("a validated entry point should not be polymorphic"),
        ),
        None => None,
    };

    // The worklist future holds per-fragment lowering state such as its
    // solver; keep it on the heap so callers' futures stay small.
    let unit =
        Box::pin(Generator::new(engine, roots, std::iter::empty(), entry_point).generate()).await;
    write!(output, "{unit}")
}

/// Generates a C translation unit starting with already-lowered `MonoIR`
/// fragments. Calls to fragments not present in `definitions` are lowered on
/// demand through `rayc_mono_ir_builder`.
pub async fn write_c_translation_unit_from_mono_ir(
    engine: &TrackedEngine,
    definitions: impl IntoIterator<Item = MonoIR>,
    output: &mut impl Write,
) -> io::Result<()> {
    let definitions = definitions.into_iter().collect::<Vec<_>>();
    let roots =
        definitions.iter().map(|definition| definition.instance().clone()).collect::<Vec<_>>();
    let unit = Generator::new(engine, roots, definitions, None).generate().await;
    write!(output, "{unit}")
}

/// The definitions of `target_id` that need no caller to choose their
/// arguments.
async fn root_definitions(
    engine: &TrackedEngine,
    target_id: TargetID,
    solver: &Solver,
) -> Vec<MonoDefInstance> {
    let mut roots = Vec::new();
    for def_id in engine.get_all_def_with_body_ids(target_id).await.iter().copied() {
        let def_id = target_id.make_global(def_id);
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
async fn lifetime_erased_instance(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    solver: &Solver,
) -> Option<MonoDefInstance> {
    let poly_vars = engine.get_enclosing_poly_var_maps(def_id).await;
    if !poly_vars.all_poly_vars_with_kind().all(|(_, kind)| is_erased_by_codegen(kind)) {
        return None;
    }

    let erased = Ty::new_lifetime(Lifetime::Erased, engine);
    let substitution =
        poly_vars.all_poly_vars().map(|poly_var| (poly_var, erased.clone())).collect::<Subst>();
    Some(MonoDefInstance::new(def_id, substitution, solver).await)
}

/// Whether arguments of this kind are erased before code generation.
const fn is_erased_by_codegen(kind: TyKind) -> bool {
    match kind {
        TyKind::Lifetime => true,
        TyKind::Star | TyKind::EffectRow | TyKind::Instance => false,
    }
}

/// Whether a symbol with a body is generated from that body, as opposed to
/// being provided by the C environment.
fn is_lowered_from_body(kind: SymbolKind) -> bool {
    match kind {
        SymbolKind::Def | SymbolKind::InstanceDef => true,
        SymbolKind::ExternDef => false,

        SymbolKind::Effect
        | SymbolKind::EffectOperation
        | SymbolKind::Instance
        | SymbolKind::Marker
        | SymbolKind::MarkerImplementation
        | SymbolKind::Module
        | SymbolKind::Strut
        | SymbolKind::Trait
        | SymbolKind::TraitType
        | SymbolKind::InstanceType
        | SymbolKind::TraitDef => {
            panic!("non-definition symbol returned by the definition inventory")
        }
    }
}

#[cfg(test)]
mod tests;
