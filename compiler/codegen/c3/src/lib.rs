//! Parallel C code generation from `MonoFunction`s.
//!
//! This is the third iteration of the C backend. It is meant to replace
//! `rayc_c2` and is built up incrementally next to it; the driver selects it
//! with `RAYC_C_BACKEND=c3`.
//!
//! The crate is organized around one unit of work, generating a function:
//!
//! - `roots` finds the functions generation starts from;
//! - `function` generates one function, reporting the functions it refers to;
//! - `coordinator` schedules every function reachable from the roots, each
//!   once;
//! - `unit` assembles the generated functions into a translation unit.
//!
//! The generated code needs some functions from the C environment, such as
//! heap allocation. They form a second translation unit, the runtime, which
//! [`write_c_runtime`] writes. The two units are compiled separately and
//! linked together.

use std::io::{self, Write};

use rayc_mono_ir::MonoFunctionInstance;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

use crate::{
    coordinator::Coordinator,
    roots::{lifetime_erased_instance, root_definitions},
    unit::TranslationUnit,
};

mod coordinator;
mod function;
mod roots;
mod unit;

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
///
/// For now, the reachable functions are only listed in comments.
pub async fn write_c_translation_unit(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    output: &mut impl Write,
) -> io::Result<()> {
    // every root instance is normalized with the same solver
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

    // generate every function reachable from the roots
    let functions = Coordinator::new(engine).generate(roots).await;

    // the entry point has no type parameters, so it is a root itself
    if let Some(entry_point) = entry_point {
        assert!(
            functions.contains_key(&MonoFunctionInstance::Body(entry_point)),
            "the executable entry point should be among the generated functions"
        );
    }

    write!(output, "{}", TranslationUnit::new(functions))
}

/// The source of the Ray runtime, a C translation unit that every program is
/// linked with.
///
/// It is kept apart from the generated unit so that the C headers it includes
/// cannot conflict with the `extern def`s of a Ray program.
const RUNTIME: &str = include_str!("../resource/runtime.c");

/// Writes the Ray runtime: the C translation unit that defines the functions
/// the generated code calls, such as `ray_alloc` and `ray_free`.
///
/// Every generated translation unit must be linked with it.
pub fn write_c_runtime(output: &mut impl Write) -> io::Result<()> {
    output.write_all(RUNTIME.as_bytes())
}
