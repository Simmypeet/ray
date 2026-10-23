//! The member accesses of a def, recorded so that a language server can
//! complete the member after a `.`.

use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::ByteIndex;
use rayc_symbol::source_map::to_absolute_span;

use crate::{typed_expr::TypedExprID, typed_function::TypedFunctionLocalID};

/// A `.` followed by a field name or a tuple index, such as `p.x` or `t.0`,
/// or a `.` whose member is still missing.
///
/// An access is recorded whether or not its member exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct MemberAccess {
    /// The dot and the member after it, or only the dot when the member is
    /// missing.
    span: RelativeSpan,

    /// The expression whose member is accessed.
    receiver: TypedFunctionLocalID<TypedExprID>,
}

impl MemberAccess {
    /// Creates the access of the member at `span` of `receiver`.
    #[must_use]
    pub const fn new(span: RelativeSpan, receiver: TypedFunctionLocalID<TypedExprID>) -> Self {
        Self { span, receiver }
    }

    /// Returns the expression whose member is accessed.
    #[must_use]
    pub const fn receiver(&self) -> TypedFunctionLocalID<TypedExprID> { self.receiver }

    /// Returns whether the cursor at `offset`, a byte index in the source
    /// file of the def, is on the member: right after the dot, or on the
    /// member being typed.
    pub async fn holds_cursor(&self, engine: &TrackedEngine, offset: ByteIndex) -> bool {
        engine.to_absolute_span(&self.span).await.contains_cursor(offset)
    }
}
