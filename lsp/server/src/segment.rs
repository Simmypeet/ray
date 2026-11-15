//! Finds the segment of a path that the cursor is on.
//!
//! A path is a root, such as `target` or a first segment, followed by
//! continuations, each a `.` and a segment. A segment with arguments, such as
//! `Pair[int32]`, is found by its name alone, so that the cursor in its
//! arguments is on a path inside them instead.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{RelativeSpan, Tree};
use rayc_parser::concrete_tree::Node;
use rayc_qbice::TrackedEngine;
use rayc_resolution::prefix::PathPrefix;
use rayc_source_file::{AbsoluteSpan, ByteIndex, SourceElement};
use rayc_syntax::{
    expression::{Leaf, Postfix},
    path::{Path, PathContinuation, PathRoot},
};
use rayc_typed_ast::typed_function::TypedFunctionMap;

/// The segment of a path that the cursor is on, and the segments before it.
#[derive(Clone)]
pub(crate) struct SegmentAt {
    path: Path,
    root: PathRoot,

    /// The names of the segments between the root and the segment at the
    /// cursor.
    between: Vec<Interned<str>>,

    /// The continuation at the cursor, or `None` if the cursor is on the
    /// root.
    continuation: Option<PathContinuation>,
}

impl SegmentAt {
    /// Finds the deepest path holding `byte` in `syntax`, whose spans are
    /// relative to `token_tree`, and the first of its parts that `is_at`
    /// accepts the span of: its root, or a continuation with its `.`.
    ///
    /// Returns `None` if no part is accepted, or if a part before the
    /// accepted one has no name.
    pub(crate) fn find(
        token_tree: &Tree,
        syntax: &Node,
        byte: ByteIndex,
        is_at: impl Fn(AbsoluteSpan) -> bool,
    ) -> Option<Self> {
        let path = syntax.get_deepest_ast::<Path>(token_tree, byte)?;
        let root = path.root()?;
        if is_at(token_tree.absolute_span_of(&root.span_without_arguments()?)) {
            return Some(Self { path, root, between: Vec::new(), continuation: None });
        }

        let mut between = Vec::new();
        let mut continuation_at = None;
        for continuation in path.rest() {
            if is_at(token_tree.absolute_span_of(&continuation.span_without_arguments()?)) {
                continuation_at = Some(continuation);
                break;
            }
            between.push(continuation.segment()?.identifier()?.kind.0);
        }

        Some(Self { continuation: Some(continuation_at?), path, root, between })
    }

    /// Returns the path holding the segment.
    pub(crate) const fn path(&self) -> &Path { &self.path }

    /// Returns whether the path holding the segment is a local followed by
    /// field accesses, `p.x.y`, rather than a path naming a symbol.
    ///
    /// As the binder does, it is one when it is written as an expression or
    /// as a callee, and its first segment names a local of `functions` that
    /// is visible there.
    pub(crate) async fn is_rooted_at_local(
        &self,
        engine: &TrackedEngine,
        token_tree: &Tree,
        syntax: &Node,
        functions: &TypedFunctionMap,
    ) -> bool {
        let Some(root) = self.path.root_name() else {
            return false;
        };

        // only the path of an expression or of a call may name a local; the
        // path of a type never does
        let span = self.path.span();
        let start = token_tree.absolute_span_of(&span).start;
        let is_expression = syntax.get_deepest_ast::<Postfix>(token_tree, start).is_some_and(
            |postfix| match postfix.leaf() {
                Some(Leaf::Path(path)) => path.span() == span,
                Some(Leaf::DirectCall(call)) => call.path().is_some_and(|path| path.span() == span),
                Some(_) | None => false,
            },
        );
        if !is_expression {
            return false;
        }

        let root_end = token_tree.absolute_span_of(&root.span).end;
        functions
            .visible_name_bindings(engine, root_end)
            .await
            .any(|(_, name_binding)| *name_binding.name() == root.kind.0)
    }

    /// Returns whether the cursor is on the root of the path.
    pub(crate) const fn is_on_root(&self) -> bool { self.continuation.is_none() }

    /// Returns the span of the name of the segment at the cursor, or of the
    /// keyword of the root.
    ///
    /// Returns `None` if the segment has no name, such as after a dangling
    /// `.`.
    pub(crate) fn name_span(&self) -> Option<RelativeSpan> {
        match &self.continuation {
            Some(continuation) => Some(continuation.segment()?.identifier()?.span),
            None => self.root.span_without_arguments(),
        }
    }

    /// Returns the beginning of the path before the segment at the cursor.
    ///
    /// Returns `None` if the root is a segment without a name.
    ///
    /// This is generally used in completion.
    pub(crate) fn into_prefix_before(self) -> Option<PathPrefix> {
        PathPrefix::with_root_syntax(&self.root, self.between)
    }

    /// Returns the beginning of the path up to the segment at the cursor,
    /// that segment included.
    ///
    /// Returns `None` if the root or the segment at the cursor has no name.
    ///
    /// This is generally used in hover, go-to-definition, etc.
    pub(crate) fn into_prefix_through(self) -> Option<PathPrefix> {
        let mut names = self.between;
        if let Some(continuation) = &self.continuation {
            names.push(continuation.segment()?.identifier()?.kind.0);
        }

        PathPrefix::with_root_syntax(&self.root, names)
    }
}
