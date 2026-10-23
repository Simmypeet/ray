//! Finds where the cursor of a request is: the source file as the compiler
//! reads it, and the innermost symbol defined at the cursor.
//!
//! Language features work in the definition of a symbol, such as its
//! signature or the body of a `def`. The innermost symbol defined at the
//! cursor is the site its paths are resolved from, and a `def` site has the
//! typed AST of its parameters and body.

use std::path::Path;

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{RelativeSpan, Tree};
use rayc_parser::concrete_tree::Node;
use rayc_qbice::TrackedEngine;
use rayc_source_file::{ByteIndex, GlobalSourceID, SourceElement, SourceFile, get_stable_path_id};
use rayc_symbol::{
    GlobalSymbolID, member::try_get_members, span::get_definition_span,
    symbol_kind::get_symbol_kind, syntax::get_def_body_syntax,
};
use rayc_symbol_impl::index::get_source_file_module;
use rayc_syntax::statement::Block;
use rayc_target::TargetID;
use rayc_typed_ast::{TypedAst, get_typed_ast};
use tower_lsp::lsp_types::Range;

use crate::position::range_of;

/// The source file the cursor is in, as the compiler reads it.
pub(crate) struct File {
    /// The ID the spans in the file refer to.
    id: GlobalSourceID,

    /// The contents of the file, which the positions of the editor index.
    source: SourceFile,

    token_tree: Interned<Tree>,

    /// The syntax tree of the whole file.
    syntax: Node,
}

impl File {
    /// Loads the source file at `path` of the target `target_id`, or returns
    /// `None` if it cannot be loaded, such as when it lies outside of the
    /// target directory.
    pub(crate) async fn load(
        engine: &TrackedEngine,
        target_id: TargetID,
        path: &Path,
    ) -> Option<Self> {
        let path: Interned<Path> = engine.intern_unsized(path.to_path_buf());
        let id =
            target_id.make_global(engine.get_stable_path_id(path.clone(), target_id).await.ok()?);

        let source =
            engine.query(&rayc_source_file::Key { path: path.clone(), target_id }).await.ok()?;
        let (token_tree, _) =
            engine.query(&rayc_lexical::Key { path: path.clone(), target_id }).await.ok()?;
        let (content, _) = engine.query(&rayc_syntax::Key { path, target_id }).await.ok()?;

        Some(Self { id, source, token_tree, syntax: Node::Branch(content?.inner_tree().clone()) })
    }

    /// Returns the token tree of the file, which the spans of its syntax are
    /// relative to.
    pub(crate) fn token_tree(&self) -> &Tree { &self.token_tree }

    /// Returns the syntax tree of the whole file.
    pub(crate) const fn syntax(&self) -> &Node { &self.syntax }

    /// Returns the range the editor shows `span`, a span in this file, at.
    pub(crate) fn range_of(&self, span: &RelativeSpan) -> Range {
        range_of(&self.source, &self.token_tree.absolute_span_of(span))
    }

    /// Finds the innermost symbol whose definition holds `offset`, and its
    /// body if the offset is in it.
    ///
    /// Returns `None` if the file is not loaded into its target.
    pub(crate) async fn site_at(&self, engine: &TrackedEngine, offset: ByteIndex) -> Option<Site> {
        // definitions nest like the symbols they define, so the innermost one
        // holding the cursor is reached from the module the file defines,
        // descending into the member whose definition holds the cursor
        let mut symbol_id = engine.get_source_file_module(self.id).await?;
        while let Some(member_id) = self.member_at(engine, symbol_id, offset).await {
            symbol_id = member_id;
        }

        let body = self.body_at(engine, symbol_id, offset).await;
        Some(Site { symbol_id, body })
    }

    /// Returns the member of `symbol_id` whose definition in this file holds
    /// `offset`, if any.
    async fn member_at(
        &self,
        engine: &TrackedEngine,
        symbol_id: GlobalSymbolID,
        offset: ByteIndex,
    ) -> Option<GlobalSymbolID> {
        let members = engine.try_get_members(symbol_id).await?;

        // the definitions of the members do not overlap, and the ones of file
        // modules are in their own files
        for member_id in members.all_ids() {
            let member_id = symbol_id.target_id.make_global(member_id);
            let Some(span) = engine.get_definition_span(member_id).await else {
                continue;
            };
            if span.source_id == self.id && self.token_tree.span_contains_cursor(&span, offset) {
                return Some(member_id);
            }
        }

        None
    }

    /// Returns the body of `symbol_id`, if it has one holding `offset`.
    async fn body_at(
        &self,
        engine: &TrackedEngine,
        symbol_id: GlobalSymbolID,
        offset: ByteIndex,
    ) -> Option<Block> {
        if !engine.get_symbol_kind(symbol_id).await.has_def_body() {
            return None;
        }

        let body = engine.get_def_body_syntax(symbol_id).await?;
        self.token_tree.span_contains_cursor(&body.span(), offset).then_some(body)
    }
}

/// The innermost symbol defined at the cursor, the site its paths are
/// resolved from.
pub(crate) struct Site {
    symbol_id: GlobalSymbolID,

    /// The body of the symbol, if the cursor is in it.
    body: Option<Block>,
}

impl Site {
    /// Returns the symbol defined at the cursor.
    pub(crate) const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Returns the body of the symbol, if the cursor is in it.
    pub(crate) const fn body(&self) -> Option<&Block> { self.body.as_ref() }

    /// Returns the typed AST of the parameters and the body of the symbol, if
    /// it is a `def` with a body, wherever the cursor is in its definition.
    pub(crate) async fn typed_ast(&self, engine: &TrackedEngine) -> Option<Interned<TypedAst>> {
        if !engine.get_symbol_kind(self.symbol_id).await.has_def_body() {
            return None;
        }

        Some(engine.get_typed_ast(self.symbol_id).await)
    }
}
