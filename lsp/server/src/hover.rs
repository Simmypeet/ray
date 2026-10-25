//! Shows what the cursor refers to, as [`Referent`] finds it.
//!
//! - A local variable or parameter is shown by its name and type, such as
//!   `count: int32`.
//! - A symbol that a segment of a path names is shown by its signature: see
//!   [`signature`].

use std::path::Path;

use rayc_qbice::TrackedEngine;
use rayc_source_file::ByteIndex;
use rayc_target::TargetID;
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Range};

use crate::{
    referent::{Local, Referent},
    signature::signature,
    site::File,
};

/// Describes what the cursor at `offset`, a byte index in the source file at
/// `path` of the target `target_id`, is on.
pub(crate) async fn hover(
    engine: &TrackedEngine,
    target_id: TargetID,
    path: &Path,
    offset: ByteIndex,
) -> Option<Hover> {
    let file = File::load(engine, target_id, path).await?;
    let (referent, span) = Referent::at(engine, &file, offset).await?;

    let code = match referent {
        Referent::Local(local) => declaration(engine, &local).await,
        Referent::Symbol(symbol_id) => signature(engine, symbol_id).await?,
    };
    Some(code_hover(&code, file.range_of(&span)))
}

/// Shows `code`, written in Ray, for the text at `range`.
fn code_hover(code: &str, range: Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: format!("```ray\n{code}\n```"),
        }),
        range: Some(range),
    }
}

/// Writes `local` with its type, such as `count: int32`.
async fn declaration(engine: &TrackedEngine, local: &Local) -> String {
    let name = local.name();
    let ty = local.ty().display(engine).await;

    format!("{name}: {ty}")
}
