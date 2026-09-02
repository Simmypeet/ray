//! Worklist-driven C code generation from [`rayc_mono_ir::MonoIR`].
//!
//! This backend consumes independently lowered definition fragments. Global
//! calls enqueue further definition instances, while concrete aggregate types
//! enter a separate layout worklist. It intentionally has no dependency on
//! `rayc_mono`.

use std::io::{self, Write};

use rayc_mono_ir::{MonoDefInstance, MonoIR};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_all_def_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::{poly_var::get_poly_var_map, subst::Subst};

use crate::generator::Generator;

mod c_type;
mod emit;
mod generator;
mod name;

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
    let mut initial_definitions = Vec::new();
    for def_id in engine.get_all_def_ids(target_id).await.iter().copied() {
        let def_id = target_id.make_global(def_id);
        match engine.get_symbol_kind(def_id).await {
            SymbolKind::Def => {
                if engine.get_poly_var_map(def_id).await.is_empty() {
                    initial_definitions.push(MonoDefInstance::new(def_id, Subst::new_empty()));
                }
            }
            SymbolKind::ExternDef => {}
            SymbolKind::Effect | SymbolKind::EffectOperation | SymbolKind::Module => {
                panic!("non-definition symbol returned by the definition inventory")
            }
        }
    }
    initial_definitions.sort();

    let entry_point = options
        .entry_point
        .map(|entry_point| MonoDefInstance::new(entry_point, Subst::new_empty()));
    let generated = Generator::new(engine, initial_definitions, std::iter::empty(), entry_point)
        .generate()
        .await;
    output.write_all(generated.as_bytes())
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
    let initial_definitions =
        definitions.iter().map(|definition| definition.instance().clone()).collect::<Vec<_>>();
    let generated = Generator::new(engine, initial_definitions, definitions, None).generate().await;
    output.write_all(generated.as_bytes())
}

#[cfg(test)]
mod tests;
