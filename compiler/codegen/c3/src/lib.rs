//! Parallel C code generation from `MonoFunction`s.
//!
//! This is the third iteration of the C backend. It is meant to replace
//! `rayc_c2` and is built up incrementally next to it; the driver selects it
//! with `RAYC_C_BACKEND=c3`.
//!
//! The generated code needs some functions from the C environment, such as
//! heap allocation. They form a second translation unit, the runtime, which
//! [`write_c_runtime`] writes. The two units are compiled separately and
//! linked together.

use std::io::{self, Write};

use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

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
/// For now, only the prelude is written.
#[expect(
    clippy::unused_async,
    unused_variables,
    reason = "no function is generated yet, so the engine is not queried"
)]
pub async fn write_c_translation_unit(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    output: &mut impl Write,
) -> io::Result<()> {
    output.write_all(unit::PRELUDE.as_bytes())
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
