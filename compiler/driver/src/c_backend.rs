//! The choice of C code generator while `rayc_c3` is built to replace
//! `rayc_c2`.
//!
//! `rayc_c2` generates the C unless the `RAYC_C_BACKEND` environment variable
//! selects `rayc_c3`. Once `rayc_c3` is complete, it becomes the only backend
//! and this module goes away.

use std::{
    env,
    io::{self, Write},
};

use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

/// Options controlling the contents of a generated C translation unit,
/// whichever [`CBackend`] generates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CTranslationUnitOptions {
    entry_point: Option<GlobalSymbolID>,
}

impl CTranslationUnitOptions {
    /// Options for a translation unit without a C entry point wrapper.
    pub(super) const fn ordinary() -> Self { Self { entry_point: None } }

    /// Options for an executable translation unit whose `main` runs the given
    /// validated Ray entry point.
    pub(super) const fn executable(entry_point: GlobalSymbolID) -> Self {
        Self { entry_point: Some(entry_point) }
    }

    fn to_c2(self) -> rayc_c2::CTranslationUnitOptions {
        self.entry_point.map_or(
            rayc_c2::CTranslationUnitOptions::ordinary(),
            rayc_c2::CTranslationUnitOptions::executable,
        )
    }

    fn to_c3(self) -> rayc_c3::CTranslationUnitOptions {
        self.entry_point.map_or(
            rayc_c3::CTranslationUnitOptions::ordinary(),
            rayc_c3::CTranslationUnitOptions::executable,
        )
    }
}

/// A C code generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CBackend {
    C2,
    C3,
}

impl CBackend {
    /// The environment variable that selects the backend: `c2` (the default)
    /// or `c3`.
    const ENVIRONMENT_VARIABLE: &str = "RAYC_C_BACKEND";

    /// The backend selected by [`Self::ENVIRONMENT_VARIABLE`].
    ///
    /// # Panics
    ///
    /// Panics if the variable is set to anything other than `c2` or `c3`, so
    /// that a misspelling never silently tests the wrong backend.
    pub(super) fn from_environment() -> Self {
        let Some(value) = env::var_os(Self::ENVIRONMENT_VARIABLE) else {
            return Self::C2;
        };

        match value.to_str() {
            Some("c2") => Self::C2,
            Some("c3") => Self::C3,
            _ => panic!(
                "{} should be `c2` or `c3`, but is `{}`",
                Self::ENVIRONMENT_VARIABLE,
                value.display()
            ),
        }
    }

    /// Generates the C translation unit of `target_id` into `output`.
    pub(super) async fn write_translation_unit(
        self,
        engine: &TrackedEngine,
        target_id: TargetID,
        options: CTranslationUnitOptions,
        output: &mut impl Write,
    ) -> io::Result<()> {
        match self {
            Self::C2 => {
                rayc_c2::write_c_translation_unit(engine, target_id, options.to_c2(), output).await
            }
            Self::C3 => {
                rayc_c3::write_c_translation_unit(engine, target_id, options.to_c3(), output).await
            }
        }
    }

    /// Writes the runtime translation unit that the generated unit is linked
    /// with.
    pub(super) fn write_runtime(self, output: &mut impl Write) -> io::Result<()> {
        match self {
            Self::C2 => rayc_c2::write_c_runtime(output),
            Self::C3 => rayc_c3::write_c_runtime(output),
        }
    }
}
