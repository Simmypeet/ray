//! Resolves and validates executable entry points.

use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{
    GlobalSymbolID, get_target_root_module_id,
    member::get_member_by_name,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::ty::{Primitive, Ty, application::View as ApplicationView};

/// An error encountered while validating an executable entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryPointError {
    /// The root module does not contain a member named `main`.
    Missing,

    /// The root member named `main` is not a function definition.
    NotDefinition { symbol_id: GlobalSymbolID },

    /// The root `main` definition does not have the required signature.
    InvalidSignature { symbol_id: GlobalSymbolID },
}

impl Report for EntryPointError {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match *self {
            Self::Missing => Rendered::builder()
                .message("main function not found; expected def main() -> int32")
                .help_message("add `def main() -> int32` to the root module")
                .build(),
            Self::NotDefinition { symbol_id } => Rendered::builder()
                .message("invalid main entry point; expected a function definition")
                .maybe_primary_highlight(
                    entry_point_highlight(engine, symbol_id, "`main` is declared here").await,
                )
                .help_message("declare the entry point as `def main() -> int32`")
                .build(),
            Self::InvalidSignature { symbol_id } => Rendered::builder()
                .message("invalid main function signature; expected def main() -> int32")
                .maybe_primary_highlight(
                    entry_point_highlight(engine, symbol_id, "invalid entry point signature").await,
                )
                .help_message("change the entry point signature to `def main() -> int32`")
                .build(),
        }
    }
}

/// Resolves and validates the root entry point for the given target.
pub(super) async fn validate_entry_point(
    engine: &TrackedEngine,
    target_id: TargetID,
) -> Result<GlobalSymbolID, EntryPointError> {
    let root_module_id = engine.get_target_root_module_id(target_id).await;
    let root_module_id = target_id.make_global(root_module_id);
    let Some(entry_point_id) = engine.get_member_by_name(root_module_id, "main").await else {
        return Err(EntryPointError::Missing);
    };

    match engine.get_symbol_kind(entry_point_id).await {
        SymbolKind::Def => {}
        SymbolKind::Effect
        | SymbolKind::EffectOperation
        | SymbolKind::ExternDef
        | SymbolKind::Instance
        | SymbolKind::InstanceDef
        | SymbolKind::Module
        | SymbolKind::Trait
        | SymbolKind::TraitDef => {
            return Err(EntryPointError::NotDefinition { symbol_id: entry_point_id });
        }
    }

    if !engine.get_parameter_map(entry_point_id).await.is_empty() {
        return Err(EntryPointError::InvalidSignature { symbol_id: entry_point_id });
    }

    let return_type = engine.get_return_type(entry_point_id).await;
    if !is_int32(&return_type) {
        return Err(EntryPointError::InvalidSignature { symbol_id: entry_point_id });
    }

    Ok(entry_point_id)
}

fn is_int32(ty: &Ty) -> bool {
    match ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Primitive(primitive) => match primitive {
                Primitive::Int32 => true,
                Primitive::Float32 | Primitive::Bool | Primitive::CInt | Primitive::CStr => false,
            },
            ApplicationView::Tuple(_)
            | ApplicationView::Lambda(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Instance(_)
            | ApplicationView::Error => false,
        },
        Ty::Inference(_) | Ty::PolyVar(_) => false,
        Ty::EffectRow(_) => todo!("validate an effect-row type as an entry-point return type"),
    }
}

async fn entry_point_highlight(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    message: &str,
) -> Option<Highlight<ByteIndex>> {
    let span = engine.get_span(symbol_id).await?;
    Some(Highlight::new(engine.to_absolute_span(&span).await, Some(message.to_owned())))
}
