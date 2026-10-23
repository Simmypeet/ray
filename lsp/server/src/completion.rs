//! Completes names, members and paths.
//!
//! Completion runs in the definition of a symbol, such as its signature or the
//! body of a `def`. The innermost symbol defined at the cursor is the site its
//! paths are resolved from.
//!
//! - In a path, it offers what the segment at the cursor can name: see
//!   [`path`].
//! - After a `.` in a body, it offers the members of the receiver's type: the
//!   fields of a struct that the def can access, or the indices of a tuple. The
//!   typed AST records every member access with its receiver, including the
//!   ones that fail to bind, such as a dangling `p.`.
//! - Elsewhere in a body, it offers the local variables visible at the cursor,
//!   as the typed AST records them, then the first segments of a path.

mod path;

use std::path::Path;

use qbice::storage::intern::Interned;
use rayc_lexical::{kind, tree::Tree};
use rayc_parser::concrete_tree::Node;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::struct_body::get_struct_body;
use rayc_source_file::ByteIndex;
use rayc_symbol::GlobalSymbolID;
use rayc_syntax::irrefutable_pattern::IrrefutablePattern;
use rayc_target::TargetID;
use rayc_type::{subst::Substitutable, ty::Ty};
use rayc_typed_ast::{get_typed_ast, typed_function::TypedFunctionMap};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::{
    completion::path::{PathCursor, Position, Rank, first_segments},
    site::File,
};

/// Lists the completions at `offset`, a byte index in the source file at
/// `path` of the target `target_id`.
pub(crate) async fn complete(
    engine: &TrackedEngine,
    target_id: TargetID,
    path: &Path,
    offset: ByteIndex,
) -> Vec<CompletionItem> {
    // only the definition of a symbol is completed
    let Some(file) = File::load(engine, target_id, path).await else {
        return Vec::new();
    };
    let Some(site) = file.site_at(engine, offset).await else {
        return Vec::new();
    };

    // a path, in a signature or a body
    if let Some(cursor) = PathCursor::at(file.token_tree(), file.syntax(), offset) {
        let mut items = cursor.complete(engine, site.symbol_id()).await;

        // in a body, a call may call a local closure by its name
        if cursor.may_name_local() && site.body().is_some() {
            let typed_ast = engine.get_typed_ast(site.symbol_id()).await;
            items.extend(complete_names(engine, typed_ast.functions(), offset).await);
        }

        return items;
    }

    // the rest is only completed in a body
    let Some(body) = site.body() else {
        return Vec::new();
    };
    let body = Node::Branch(body.inner_tree().clone());
    let typed_ast = engine.get_typed_ast(site.symbol_id()).await;
    let functions = typed_ast.functions();

    // a path being typed, before the `(` or `{` that makes it one
    if let Some(cursor) =
        PathCursor::in_postfix(engine, file.token_tree(), &body, functions, offset).await
    {
        return cursor.complete(engine, site.symbol_id()).await;
    }

    // the member of an access, whose receiver the binder recorded
    if let Some(receiver_ty) = functions.receiver_type_at(engine, offset).await {
        return complete_members(engine, site.symbol_id(), receiver_ty).await;
    }

    match Context::at(file.token_tree(), &body, offset) {
        Context::Name => {
            let mut items = complete_names(engine, functions, offset).await;
            items.extend(first_segments(engine, site.symbol_id(), Position::Expression).await);
            items
        }
        Context::AfterDot | Context::Unsupported => Vec::new(),
    }
}

/// What the text before the cursor asks to complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// A name, such as a variable, which may be partly typed.
    Name,

    /// What follows a `.` that is neither a recorded member access nor a
    /// path, such as `target.` before its call is typed.
    AfterDot,

    /// Nothing can be completed, such as inside a literal.
    Unsupported,
}

impl Context {
    /// Classifies the cursor at `offset` in `body`, where the spans of `body`
    /// are relative to `token_tree`.
    fn at(token_tree: &Tree, body: &Node, offset: ByteIndex) -> Self {
        let token_before = |offset: ByteIndex| {
            offset.checked_sub(1).and_then(|before| body.get_pointing_token(token_tree, before))
        };

        // the cursor sits right after the token being typed; after whitespace
        // or a comment, a name starts
        let Some(token) = token_before(offset) else {
            return Self::Name;
        };
        if !token.kind.is_word() {
            return Self::after(&token.kind);
        }

        // a word being typed in a pattern declares a new name
        if body.get_deepest_ast::<IrrefutablePattern>(token_tree, offset - 1).is_some() {
            return Self::Unsupported;
        }

        // otherwise, it completes what the token before it asks for
        let start = token_tree.absolute_location_of(token.start_location());
        token_before(start).map_or(Self::Name, |previous| Self::after(&previous.kind))
    }

    /// Returns what a cursor right after a token of `kind` completes.
    const fn after(kind: &kind::Kind) -> Self {
        match kind {
            kind::Kind::Punctuation(kind::Punctuation('.')) => Self::AfterDot,
            kind::Kind::Identifier(_)
            | kind::Kind::Keyword(_)
            | kind::Kind::Punctuation(_)
            | kind::Kind::NewLine(_) => Self::Name,
            kind::Kind::Character(_) | kind::Kind::String(_) | kind::Kind::Numeric(_) => {
                Self::Unsupported
            }
        }
    }
}

/// Lists the local variables in `functions` visible at `offset`, sorted by
/// name. They sort before the other names a first segment can name.
async fn complete_names(
    engine: &TrackedEngine,
    functions: &TypedFunctionMap,
    offset: ByteIndex,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for (_, name_binding) in functions.visible_name_bindings(engine, offset).await {
        let label = name_binding.name().to_string();
        items.push(CompletionItem {
            sort_text: Some(format!("{}{label}", Rank::Local.prefix())),
            label,
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some(name_binding.ty().display(engine).await.to_string()),
            ..CompletionItem::default()
        });
    }

    // the bindings are found in no particular order
    items.sort_by(|left, right| left.label.cmp(&right.label));
    items
}

/// Lists the members of a receiver of type `ty` that `def_id` can access,
/// sorted by name.
///
/// A reference has no members: field access does not dereference it.
async fn complete_members(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    ty: &Interned<Ty>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // the fields of a struct accessible from the def, typed with the struct's
    // generic arguments
    if let Some(st) = ty.as_struct_view() {
        let body = engine.get_struct_body(st.symbol_id()).await;
        let subst = st.create_subst(engine).await;

        for (_, field) in body.iter() {
            if !field.accessibility().is_accessible_from(def_id, engine).await {
                continue;
            }

            let field_ty = field.ty().apply_subst_or_clone(&subst, engine);
            items.push(member_item(engine, field.name().to_string(), &field_ty).await);
        }
    }

    // the indices of a tuple
    if let Some(tuple) = ty.as_tuple_view() {
        for (index, element_ty) in tuple.args().iter().enumerate() {
            items.push(member_item(engine, index.to_string(), element_ty).await);
        }
    }

    // the fields are found in no particular order
    items.sort_by(|left, right| left.label.cmp(&right.label));
    items
}

/// Creates the item of the member `label` whose type is `ty`.
async fn member_item(engine: &TrackedEngine, label: String, ty: &Interned<Ty>) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(CompletionItemKind::FIELD),
        detail: Some(ty.display(engine).await.to_string()),
        ..CompletionItem::default()
    }
}
