//! Finds what the cursor refers to: a local variable or parameter, or the
//! symbol that a segment of a path names.
//!
//! - In the definition of a `def`, the typed AST records every name binding
//!   with the pattern declaring it, and every identifier of a body with the
//!   binding it refers to, so the cursor may be on either.
//! - On any segment of a path, the segments up to it are looked up as the
//!   resolver does, from the site the path is written at.
//!
//! A local is found first, as the binder looks a bare identifier up among the
//! locals first: the callee of a call of a local closure is a path too.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::prefix::PathPrefix;
use rayc_source_file::ByteIndex;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;
use rayc_typed_ast::name_binding::NameBinding;

use crate::{segment::SegmentAt, site::File};

/// What the cursor refers to.
pub(crate) enum Referent {
    /// A local variable or parameter.
    Local(Local),

    /// The symbol that a segment of a path names.
    Symbol(GlobalSymbolID),
}

impl Referent {
    /// Finds what the cursor at `offset` in `file` refers to, with the span
    /// of the name the cursor is on: a pattern, an identifier, or the name of
    /// a segment.
    pub(crate) async fn at(
        engine: &TrackedEngine,
        file: &File,
        offset: ByteIndex,
    ) -> Option<(Self, RelativeSpan)> {
        let site = file.site_at(engine, offset).await?;

        // a local variable or parameter, which only the parameters and the
        // body of a `def` declare
        if let Some(typed_ast) = site.typed_ast(engine).await
            && let Some((name_binding, span)) =
                typed_ast.functions().name_binding_at(engine, offset).await
        {
            return Some((Self::Local(Local::new(name_binding)), span));
        }

        // a segment of a path, naming a symbol
        let (prefix, span) = segment_at(file, offset)?;
        let symbol_id = prefix.find_symbol(engine, site.symbol_id()).await?;
        Some((Self::Symbol(symbol_id), span))
    }
}

/// A local variable or parameter, as declared by a pattern.
pub(crate) struct Local {
    name: Interned<str>,
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl Local {
    fn new(name_binding: &NameBinding) -> Self {
        Self {
            name: name_binding.name().clone(),
            ty: name_binding.ty().clone(),
            span: *name_binding.span(),
        }
    }

    /// Returns the name of the local.
    pub(crate) fn name(&self) -> &str { &self.name }

    /// Returns the type of the local.
    pub(crate) const fn ty(&self) -> &Interned<Ty> { &self.ty }

    /// Returns the span of the pattern declaring the local.
    pub(crate) const fn span(&self) -> &RelativeSpan { &self.span }
}

/// Finds the segment of a path that the cursor at `offset` touches in `file`,
/// returning the beginning of the path up to that segment and the span of its
/// name.
fn segment_at(file: &File, offset: ByteIndex) -> Option<(PathPrefix, RelativeSpan)> {
    // the editor points at the character after the cursor, while a cursor
    // right after a name, as when it is typed, is in the path before it
    for byte in [Some(offset), offset.checked_sub(1)].into_iter().flatten() {
        let Some(segment_at) = SegmentAt::find(file.token_tree(), file.syntax(), byte, |span| {
            span.touches_cursor(offset)
        }) else {
            continue;
        };

        let span = segment_at.name_span()?;
        return Some((segment_at.into_prefix_through()?, span));
    }

    None
}
