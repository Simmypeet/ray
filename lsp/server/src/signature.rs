//! Writes the signature of a symbol: its definition as written, without the
//! bodies of its defs.
//!
//! A struct is shown with its fields, a trait or an effect with the
//! signatures of its members, and an instance with the signatures of its
//! members without their bodies. A module is shown by its name alone, as its
//! members are too many to show.

use rayc_qbice::TrackedEngine;
use rayc_source_file::{ByteIndex, SourceElement, SourceFile};
use rayc_symbol::{
    GlobalSymbolID,
    member::try_get_members,
    name::get_name,
    source_map::to_absolute_span,
    span::get_definition_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_def_body_syntax,
};
use rayc_syntax::statement::Block;

use crate::position::load_source;

/// Returns the signature of `symbol_id`, or `None` if its definition cannot
/// be read.
pub(crate) async fn signature(engine: &TrackedEngine, symbol_id: GlobalSymbolID) -> Option<String> {
    if engine.get_symbol_kind(symbol_id).await == SymbolKind::Module {
        let name = engine.get_name(symbol_id).await;
        return Some(format!("module {}", &*name));
    }

    // the definition, from the file it is written in
    let span = engine.get_definition_span(symbol_id).await?;
    let source = load_source(engine, span.source_id).await?;
    let definition = engine.to_absolute_span(&span).await;

    // the bodies are left out, which are those of the def itself or of the
    // defs of its members, in the order they are written
    let mut def_ids = vec![symbol_id];
    if let Some(members) = engine.try_get_members(symbol_id).await {
        def_ids
            .extend(members.all_ids().map(|member_id| symbol_id.target_id.make_global(member_id)));
    }

    let mut bodies = Vec::new();
    for def_id in def_ids {
        if let Some(body) = def_body(engine, def_id).await {
            bodies.push(engine.to_absolute_span(&body.span()).await);
        }
    }
    bodies.sort_by_key(|body| body.start);

    // a body starts at the `:` ending its signature, so the text around the
    // bodies is the signatures alone
    let mut text = String::new();
    let mut start = definition.start;
    for body in bodies {
        text.push_str(&source.slice(start..body.start));
        start = body.end;
    }
    text.push_str(&source.slice(start..definition.end));

    Some(dedent(&text, indentation_at(&source, definition.start)))
}

/// Returns the body of `def_id`, if it is a def with one.
async fn def_body(engine: &TrackedEngine, def_id: GlobalSymbolID) -> Option<Block> {
    if !engine.get_symbol_kind(def_id).await.has_def_body() {
        return None;
    }

    engine.get_def_body_syntax(def_id).await
}

/// Returns how many bytes precede `byte_index` on its line in `source`.
fn indentation_at(source: &SourceFile, byte_index: ByteIndex) -> usize {
    byte_index - source.line_to_byte(source.byte_to_line(byte_index))
}

/// Removes up to `indentation` leading spaces from the lines of `text` after
/// the first, which starts where the indentation ends, and the whitespace at
/// the end of every line.
fn dedent(text: &str, indentation: usize) -> String {
    let mut lines = text.lines();
    let mut dedented = lines.next().unwrap_or_default().trim_end().to_owned();
    for line in lines {
        let indented = line.len() - line.trim_start_matches(' ').len();
        dedented.push('\n');
        dedented.push_str(line[indented.min(indentation)..].trim_end());
    }

    dedented.trim_end().to_owned()
}
