//! Worklist-driven C code generation from [`MonoFunction`]s.
//!
//! The backend consumes independently lowered functions. Calls to other
//! functions schedule them on a worklist, and every aggregate type the
//! generated code mentions is collected so its `struct` can be defined before
//! use.
//!
//! The crate is organized in layers:
//!
//! - [`c`] renders C syntax (names, types, expressions) as allocation-free
//!   [`std::fmt::Display`] adaptors;
//! - `program` holds the program-wide state discovered while generating: the
//!   function `worklist` and the `aggregates` used so far. Everything else,
//!   such as signatures and linkage, comes from engine queries;
//! - `print` writes a function, recording what it refers to, and `generator`
//!   drives it over the worklist;
//! - `unit` assembles the final translation unit.

use std::io::{self, Write};

use rayc_mono_ir::{MonoBodyInstance, function::MonoFunction};
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

use crate::generator::Generator;

mod aggregates;
mod c;
mod generator;
mod print;
mod program;
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

    // The worklist future holds per-function lowering state such as its
    // solver; keep it on the heap so callers' futures stay small.
    let unit =
        Box::pin(Generator::new(engine, roots, std::iter::empty(), entry_point).generate()).await;
    write!(output, "{unit}")
}

/// Generates a C translation unit starting with already-lowered
/// `MonoFunction`s. Calls to functions not present in `functions` are lowered
/// on demand through `rayc_mono_ir_builder`.
pub async fn write_c_translation_unit_from_mono_ir(
    engine: &TrackedEngine,
    functions: impl IntoIterator<Item = MonoFunction>,
    output: &mut impl Write,
) -> io::Result<()> {
    let functions = functions.into_iter().collect::<Vec<_>>();
    let roots = functions.iter().map(|function| function.instance().clone()).collect::<Vec<_>>();
    let unit = Generator::new(engine, roots, functions, None).generate().await;
    write!(output, "{unit}")
}

/// The definitions of `target_id` that need no caller to choose their
/// arguments.
async fn root_definitions(
    engine: &TrackedEngine,
    target_id: TargetID,
    solver: &Solver,
) -> Vec<MonoBodyInstance> {
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

#[cfg(test)]
mod tests;
