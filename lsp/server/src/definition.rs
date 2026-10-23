//! Finds where what the cursor refers to, as [`Referent`] finds it, is
//! defined: the pattern declaring a local variable or parameter, or the name
//! of a symbol.

use std::path::Path;

use rayc_qbice::TrackedEngine;
use rayc_source_file::{AbsoluteSpan, ByteIndex, Span};
use rayc_symbol::{
    GlobalSymbolID,
    source_map::to_absolute_span,
    span::{get_definition_span, get_span},
};
use rayc_target::TargetID;
use tower_lsp::lsp_types::Location;

use crate::{
    overlay::Overlay,
    position::{load_source, range_of},
    referent::Referent,
    site::File,
};

/// Finds the definition of what the cursor at `offset`, a byte index in the
/// source file at `path` of the target `target_id`, refers to, located in
/// the documents of `overlay`.
pub(crate) async fn definition(
    engine: &TrackedEngine,
    overlay: &Overlay,
    target_id: TargetID,
    path: &Path,
    offset: ByteIndex,
) -> Option<Location> {
    let file = File::load(engine, target_id, path).await?;
    let (referent, _) = Referent::at(engine, &file, offset).await?;

    let span = match referent {
        Referent::Local(local) => engine.to_absolute_span(local.span()).await,
        Referent::Symbol(symbol_id) => symbol_definition(engine, symbol_id).await?,
    };

    // the definition may be in another file, or in the bundled core library
    let source = load_source(engine, span.source_id).await?;
    let uri = overlay.source_uri(span.source_id.target_id, source.path())?;
    Some(Location::new(uri, range_of(&source, &span)))
}

/// Returns where `symbol_id` is defined: its name, or the top of the file
/// defining it if its name is not written in that file.
///
/// A file module is declared by its name in the file of its parent, but
/// defined by its own file, and the root module of a target has no name
/// written anywhere.
async fn symbol_definition(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
) -> Option<AbsoluteSpan> {
    let definition = engine.get_definition_span(symbol_id).await;
    if let Some(name) = engine.get_span(symbol_id).await
        && definition.is_none_or(|definition| definition.source_id == name.source_id)
    {
        return Some(engine.to_absolute_span(&name).await);
    }

    // the definition of a module starts at its first member, after the
    // comments at the top of its file
    Some(Span::new(0, 0, definition?.source_id))
}
