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
use rayc_type::{poly_var::get_poly_var_map, subst::Subst};

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
/// from the non-generic definitions in `target_id`.
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
        Some(entry_point) => {
            Some(MonoDefInstance::new(entry_point, Subst::new_empty(), &solver).await)
        }
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

/// The monomorphic definitions of `target_id`, which need no caller to
/// choose their type arguments.
async fn root_definitions(
    engine: &TrackedEngine,
    target_id: TargetID,
    solver: &Solver,
) -> Vec<MonoDefInstance> {
    let mut roots = Vec::new();
    for def_id in engine.get_all_def_with_body_ids(target_id).await.iter().copied() {
        let def_id = target_id.make_global(def_id);
        if is_free_standing_definition(engine.get_symbol_kind(def_id).await)
            && engine.get_poly_var_map(def_id).await.is_empty()
        {
            roots.push(MonoDefInstance::new(def_id, Subst::new_empty(), solver).await);
        }
    }
    roots
}

/// Whether a symbol with a body is a free-standing definition rather than
/// one reached only through an instance or an extern declaration.
fn is_free_standing_definition(kind: SymbolKind) -> bool {
    match kind {
        SymbolKind::Def => true,
        SymbolKind::InstanceDef | SymbolKind::ExternDef => false,

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
