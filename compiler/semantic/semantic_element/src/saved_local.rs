//! The saved locals of a coroutine body: the types of what it holds while it
//! is suspended.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Ty, application::ClosureID};

/// A body that a coroutine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum CoroutineBody {
    /// The body of a `def`, or of an instance `def`.
    Def(GlobalSymbolID),
    /// The body of a closure.
    Closure(ClosureCoroutineBodyID),
}

impl CoroutineBody {
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID {
        match self {
            Self::Def(def_id) => *def_id,
            Self::Closure(closure_body_id) => closure_body_id.owner_id(),
        }
    }

    #[must_use]
    pub const fn closure_id(&self) -> Option<ClosureID> {
        match self {
            Self::Def(_) => None,
            Self::Closure(closure_body_id) => Some(closure_body_id.local_closure_id()),
        }
    }
}

/// Identifies a particular closure body in an owning definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct ClosureCoroutineBodyID {
    owner_id: GlobalSymbolID,
    local_closure_id: ClosureID,
}

impl ClosureCoroutineBodyID {
    #[must_use]
    pub const fn new(owner_id: GlobalSymbolID, local_closure_id: ClosureID) -> Self {
        Self { owner_id, local_closure_id }
    }

    #[must_use]
    pub const fn owner_id(&self) -> GlobalSymbolID { self.owner_id }

    #[must_use]
    pub const fn local_closure_id(&self) -> ClosureID { self.local_closure_id }
}

/// What a saved local is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum SavedLocalKind {
    /// A variable declared in source.
    Variable,

    /// A temporary, which gives a place to a computed value.
    Temporary,

    /// A parameter.
    Parameter,

    /// An evaluated expression value that is not stored in a local yet.
    Value,

    /// The coroutine of a callee, which the body waits for.
    Callee,

    /// A capture of the handled body or of the operation handlers of a
    /// `run ... with`, which the body waits for.
    RunCapture,

    /// The continuation that a forwarding `handle` resumes.
    Continuation,
}

/// Where a saved local comes from, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct SavedLocalOrigin {
    kind: SavedLocalKind,
    span: RelativeSpan,
    suspension_span: RelativeSpan,
    runs: Vec<RelativeSpan>,
}

impl SavedLocalOrigin {
    #[must_use]
    pub const fn new(
        kind: SavedLocalKind,
        span: RelativeSpan,
        suspension_span: RelativeSpan,
        runs: Vec<RelativeSpan>,
    ) -> Self {
        Self { kind, span, suspension_span, runs }
    }

    /// Returns what is saved.
    #[must_use]
    pub const fn kind(&self) -> SavedLocalKind { self.kind }

    /// Returns the span of what is saved: the declaration of a variable or a
    /// parameter, or the expression of anything else, such as the call of a
    /// callee or the `run ... with` of a capture.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    /// Returns the span of the suspension point the local is held across.
    #[must_use]
    pub const fn suspension_span(&self) -> RelativeSpan { self.suspension_span }

    /// Returns the spans of the `run ... with`s whose nested bodies hold the
    /// local, outermost first. It is empty for a local of the body itself.
    #[must_use]
    pub fn runs(&self) -> &[RelativeSpan] { &self.runs }
}

/// One type that a coroutine body holds while it is suspended.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct SavedLocal {
    ty: Interned<Ty>,
    origin: SavedLocalOrigin,
}

impl SavedLocal {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, origin: SavedLocalOrigin) -> Self { Self { ty, origin } }

    /// Returns the type, written over the polymorphic parameters of the
    /// definition the body is written in, with its lifetimes erased.
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    /// Returns where the local comes from.
    #[must_use]
    pub const fn origin(&self) -> &SavedLocalOrigin { &self.origin }

    /// Returns the span of what is saved: where it is declared or evaluated.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.origin.span() }
}

/// Computes the saved locals of a coroutine body, from the IR of the
/// definition it is written in.
///
/// Every local is listed at every suspension point it is held across, so
/// one type may appear several times, each with its own origin. The value is
/// `None` when that IR has errors: its drops may not be inserted, so the list
/// could be incomplete.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Interned<[SavedLocal]>>)]
#[extend(by_val, name = get_saved_locals)]
pub struct SavedLocals {
    pub body: CoroutineBody,
}
