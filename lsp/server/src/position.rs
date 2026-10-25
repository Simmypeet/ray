//! Converts between the locations of the protocol and the ones of the
//! compiler.
//!
//! The column of a position counts UTF-16 code units.

use rayc_qbice::TrackedEngine;
use rayc_source_file::{
    AbsoluteSpan, ByteIndex, EditorLocation, GlobalSourceID, SourceFile, get_source_file_path,
};
use tower_lsp::lsp_types::{Position, Range};

/// Returns the position of the byte index `byte_index` in `source`.
pub(crate) fn position_of(source: &SourceFile, byte_index: ByteIndex) -> Position {
    let location = source.get_utf16_location_from_byte_index(byte_index);
    Position::new(
        u32::try_from(location.line).unwrap_or(u32::MAX),
        u32::try_from(location.column).unwrap_or(u32::MAX),
    )
}

/// Returns the range of `span`, a span in `source`.
pub(crate) fn range_of(source: &SourceFile, span: &AbsoluteSpan) -> Range {
    Range::new(position_of(source, span.start), position_of(source, span.end))
}

/// Returns the byte index of `position` in `source`.
pub(crate) fn byte_index_of(source: &SourceFile, position: Position) -> ByteIndex {
    source.get_byte_index_from_utf16_location(&EditorLocation::new(
        position.line as usize,
        position.character as usize,
    ))
}

/// Loads the source file that the spans of `source_id` index, as the compiler
/// reads it.
pub(crate) async fn load_source(
    engine: &TrackedEngine,
    source_id: GlobalSourceID,
) -> Option<SourceFile> {
    let path = engine.get_source_file_path(source_id).await;
    engine.query(&rayc_source_file::Key { path, target_id: source_id.target_id }).await.ok()
}
