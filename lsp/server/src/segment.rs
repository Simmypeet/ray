//! Finds the segment of a path that the cursor is on.
//!
//! A path is a root, such as `target` or a first segment, followed by
//! continuations, each a `.` and a segment. A segment with arguments, such as
//! `Pair[int32]`, is found by its name alone, so that the cursor in its
//! arguments is on a path inside them instead.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{RelativeSpan, Tree};
use rayc_parser::concrete_tree::Node;
use rayc_resolution::prefix::PathPrefix;
use rayc_source_file::{AbsoluteSpan, ByteIndex};
use rayc_syntax::path::{Path, PathContinuation, PathRoot};

/// The segment of a path that the cursor is on, and the segments before it.
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
