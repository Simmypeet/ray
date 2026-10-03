//! The errors the borrow checker reports.
//!
//! Each error is an access that invalidates a loan while the loan is still
//! live, and points at three places: the access that invalidates the loan,
//! the borrow that issued it, and a later use of the borrow, which is why the
//! loan is still live. A loan that outlives the function, such as one stored
//! behind a parameter, has no later use within it.
//!
//! A borrow, a read or a move made to capture a place into a nested function
//! points at the expression creating that function, and says so.
//!
//! The exceptions are a relation between two universal lifetimes, and a type
//! outliving a universal lifetime, that the function requires but may not
//! assume. Neither involves a loan: they point at the instruction that
//! requires them.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::source_map::to_absolute_span;
use rayc_type::ty::{Mutability, Ty};

/// Where a place is borrowed, read or moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct AccessSite {
    /// The expression accessing the place, or, for a capture, the expression
    /// creating the nested function that captures it.
    span: RelativeSpan,

    /// Whether the place is accessed to capture it into a nested function: a
    /// closure, a handled body or an operation handler.
    is_capture: bool,
}

impl AccessSite {
    pub(crate) const fn new(span: RelativeSpan, is_capture: bool) -> Self {
        Self { span, is_capture }
    }

    /// Returns the highlight of the access, labelled `message`, which says
    /// what happens there.
    async fn highlight(&self, engine: &TrackedEngine, message: &str) -> Highlight<ByteIndex> {
        let message =
            if self.is_capture { format!("{message}, by a capture") } else { message.to_owned() };

        Highlight::builder()
            .span(engine.to_absolute_span(&self.span).await)
            .message(message)
            .build()
    }
}

/// The loan an access conflicts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ConflictingLoan {
    /// The borrow that issued the loan.
    borrow: AccessSite,

    /// Whether the loan is shared or mutable.
    mutability: Mutability,

    /// A use of the borrow after the conflicting access, if the borrow is used
    /// within the function.
    later_use_span: Option<RelativeSpan>,
}

impl ConflictingLoan {
    pub(crate) const fn new(
        borrow: AccessSite,
        mutability: Mutability,
        later_use_span: Option<RelativeSpan>,
    ) -> Self {
        Self { borrow, mutability, later_use_span }
    }

    /// Returns the highlight of the borrow, labelled `message`.
    async fn borrow_highlight(
        &self,
        engine: &TrackedEngine,
        message: String,
    ) -> Highlight<ByteIndex> {
        self.borrow.highlight(engine, &message).await
    }

    /// Returns the highlights of the borrow, labelled as a borrow of its
    /// mutability, and of its later use, if it has one.
    async fn related_highlights(&self, engine: &TrackedEngine) -> Vec<Highlight<ByteIndex>> {
        let borrow = format!("{} borrow occurs here", self.mutability.name());

        std::iter::once(self.borrow_highlight(engine, borrow).await)
            .chain(self.later_use_highlight(engine).await)
            .collect()
    }

    /// Returns the highlight of the later use of the borrow, if it has one.
    async fn later_use_highlight(&self, engine: &TrackedEngine) -> Option<Highlight<ByteIndex>> {
        let later_use_span = self.later_use_span.as_ref()?;
        Some(
            Highlight::builder()
                .span(engine.to_absolute_span(later_use_span).await)
                .message("borrow later used here")
                .build(),
        )
    }
}

/// A place is assigned while a loan of it is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct AssignToBorrowed {
    assign_span: RelativeSpan,
    loan: ConflictingLoan,
}

impl AssignToBorrowed {
    pub(crate) const fn new(assign_span: RelativeSpan, loan: ConflictingLoan) -> Self {
        Self { assign_span, loan }
    }
}

/// A place is borrowed while a conflicting loan of it is live: a mutable
/// borrow while it is borrowed at all, or a shared borrow while it is
/// mutably borrowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ConflictingBorrow {
    borrow: AccessSite,
    mutability: Mutability,
    loan: ConflictingLoan,
}

impl ConflictingBorrow {
    pub(crate) const fn new(
        borrow: AccessSite,
        mutability: Mutability,
        loan: ConflictingLoan,
    ) -> Self {
        Self { borrow, mutability, loan }
    }
}

/// A place is read, to copy its value, while a mutable loan of it is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct UseOfMutablyBorrowed {
    usage: AccessSite,
    loan: ConflictingLoan,
}

impl UseOfMutablyBorrowed {
    pub(crate) const fn new(usage: AccessSite, loan: ConflictingLoan) -> Self {
        Self { usage, loan }
    }
}

/// A value is moved out of a place while a loan of it is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct MoveOfBorrowed {
    moved: AccessSite,
    loan: ConflictingLoan,
}

impl MoveOfBorrowed {
    pub(crate) const fn new(moved: AccessSite, loan: ConflictingLoan) -> Self {
        Self { moved, loan }
    }
}

/// A variable goes out of scope while a loan of it is live.
///
/// Scopes carry no span of their own, so the variable's declaration stands
/// for the end of its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct DoesNotLiveLongEnough {
    binding_span: RelativeSpan,
    loan: ConflictingLoan,
}

impl DoesNotLiveLongEnough {
    pub(crate) const fn new(binding_span: RelativeSpan, loan: ConflictingLoan) -> Self {
        Self { binding_span, loan }
    }
}

/// A temporary goes out of scope, at the end of the statement creating it,
/// while a loan of it is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct TemporaryDroppedWhileBorrowed {
    /// The expression whose value the temporary holds.
    temporary_span: RelativeSpan,
    loan: ConflictingLoan,
}

impl TemporaryDroppedWhileBorrowed {
    pub(crate) const fn new(temporary_span: RelativeSpan, loan: ConflictingLoan) -> Self {
        Self { temporary_span, loan }
    }
}

/// A value is dropped while a loan of what its `Drop` implementation may use
/// is live, such as the memory behind a mutable reference it holds.
///
/// A loan of the storage of the value itself is reported where that storage
/// ends instead, as [`DoesNotLiveLongEnough`] or [`AssignToBorrowed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct BorrowedWhenDropped {
    /// The declaration of the binding holding the dropped value.
    binding_span: RelativeSpan,
    loan: ConflictingLoan,
}

impl BorrowedWhenDropped {
    pub(crate) const fn new(binding_span: RelativeSpan, loan: ConflictingLoan) -> Self {
        Self { binding_span, loan }
    }
}

/// The function returns a value that holds a loan of a place it owns: a
/// variable, a temporary, a parameter or a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ReturnsBorrowOfLocal {
    /// The returned value.
    return_span: RelativeSpan,
    loan: ConflictingLoan,
}

impl ReturnsBorrowOfLocal {
    pub(crate) const fn new(return_span: RelativeSpan, loan: ConflictingLoan) -> Self {
        Self { return_span, loan }
    }
}

/// The function requires a universal lifetime to outlive another, which
/// neither its where clause nor its signature lets it assume.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct LifetimeMayNotLiveLongEnough {
    /// The instruction that requires the relation: where a value of the
    /// longer lifetime flows into the shorter one.
    span: RelativeSpan,

    /// The lifetime required to outlive `shorter`.
    longer: Interned<Ty>,

    /// The lifetime `longer` is required to outlive.
    shorter: Interned<Ty>,

    /// Where the value of the longer lifetime comes from, when that is not
    /// at `span` itself.
    origin_span: Option<RelativeSpan>,
}

impl LifetimeMayNotLiveLongEnough {
    pub(crate) const fn new(
        span: RelativeSpan,
        longer: Interned<Ty>,
        shorter: Interned<Ty>,
        origin_span: Option<RelativeSpan>,
    ) -> Self {
        Self { span, longer, shorter, origin_span }
    }
}

/// The function requires a type parameter or a projection to outlive a
/// universal lifetime, which neither its where clause nor its signature lets
/// it assume.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct TypeMayNotLiveLongEnough {
    /// The instruction that requires the type to outlive a lifetime.
    span: RelativeSpan,

    /// The type parameter or projection required to outlive `bound`.
    subject: Interned<Ty>,

    /// The universal lifetime `subject` is required to outlive.
    bound: Interned<Ty>,

    /// Where the lifetime required at `span` is itself required to outlive
    /// `bound`, when that is not at `span`.
    bound_span: Option<RelativeSpan>,
}

impl TypeMayNotLiveLongEnough {
    pub(crate) const fn new(
        span: RelativeSpan,
        subject: Interned<Ty>,
        bound: Interned<Ty>,
        bound_span: Option<RelativeSpan>,
    ) -> Self {
        Self { span, subject, bound, bound_span }
    }
}

/// An error found by the borrow checker.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    AssignToBorrowed(AssignToBorrowed),
    ConflictingBorrow(ConflictingBorrow),
    DoesNotLiveLongEnough(DoesNotLiveLongEnough),
    UseOfMutablyBorrowed(UseOfMutablyBorrowed),
    MoveOfBorrowed(MoveOfBorrowed),
    TemporaryDroppedWhileBorrowed(TemporaryDroppedWhileBorrowed),
    BorrowedWhenDropped(BorrowedWhenDropped),
    ReturnsBorrowOfLocal(ReturnsBorrowOfLocal),
    LifetimeMayNotLiveLongEnough(LifetimeMayNotLiveLongEnough),
    TypeMayNotLiveLongEnough(TypeMayNotLiveLongEnough),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::AssignToBorrowed(diagnostic) => {
                assign_to_borrowed_report(engine, diagnostic).await
            }
            Self::ConflictingBorrow(diagnostic) => {
                conflicting_borrow_report(engine, diagnostic).await
            }
            Self::DoesNotLiveLongEnough(diagnostic) => {
                does_not_live_long_enough_report(engine, diagnostic).await
            }
            Self::UseOfMutablyBorrowed(diagnostic) => {
                use_of_mutably_borrowed_report(engine, diagnostic).await
            }
            Self::MoveOfBorrowed(diagnostic) => move_of_borrowed_report(engine, diagnostic).await,
            Self::TemporaryDroppedWhileBorrowed(diagnostic) => {
                temporary_dropped_while_borrowed_report(engine, diagnostic).await
            }
            Self::BorrowedWhenDropped(diagnostic) => {
                borrowed_when_dropped_report(engine, diagnostic).await
            }
            Self::ReturnsBorrowOfLocal(diagnostic) => {
                returns_borrow_of_local_report(engine, diagnostic).await
            }
            Self::LifetimeMayNotLiveLongEnough(diagnostic) => {
                lifetime_may_not_live_long_enough_report(engine, diagnostic).await
            }
            Self::TypeMayNotLiveLongEnough(diagnostic) => {
                type_may_not_live_long_enough_report(engine, diagnostic).await
            }
        }
    }
}

async fn assign_to_borrowed_report(
    engine: &TrackedEngine,
    diagnostic: &AssignToBorrowed,
) -> Rendered<ByteIndex> {
    Rendered::builder()
        .message("cannot assign to a place because it is borrowed")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.assign_span).await)
                .message("the borrowed place is assigned here")
                .build(),
        )
        .related(diagnostic.loan.related_highlights(engine).await)
        .build()
}

async fn use_of_mutably_borrowed_report(
    engine: &TrackedEngine,
    diagnostic: &UseOfMutablyBorrowed,
) -> Rendered<ByteIndex> {
    Rendered::builder()
        .message("cannot use a place because it is mutably borrowed")
        .primary_highlight(
            diagnostic.usage.highlight(engine, "the borrowed place is used here").await,
        )
        .related(diagnostic.loan.related_highlights(engine).await)
        .build()
}

async fn move_of_borrowed_report(
    engine: &TrackedEngine,
    diagnostic: &MoveOfBorrowed,
) -> Rendered<ByteIndex> {
    Rendered::builder()
        .message("cannot move out of a place because it is borrowed")
        .primary_highlight(
            diagnostic.moved.highlight(engine, "the borrowed place is moved out of here").await,
        )
        .related(diagnostic.loan.related_highlights(engine).await)
        .build()
}

async fn temporary_dropped_while_borrowed_report(
    engine: &TrackedEngine,
    diagnostic: &TemporaryDroppedWhileBorrowed,
) -> Rendered<ByteIndex> {
    let temporary = Highlight::builder()
        .span(engine.to_absolute_span(&diagnostic.temporary_span).await)
        .message("this creates a temporary value, which is dropped at the end of the statement")
        .build();
    let borrow = "the temporary value is borrowed here".to_owned();

    Rendered::builder()
        .message("temporary value dropped while borrowed")
        .primary_highlight(diagnostic.loan.borrow_highlight(engine, borrow).await)
        .related(
            std::iter::once(temporary)
                .chain(diagnostic.loan.later_use_highlight(engine).await)
                .collect(),
        )
        .help_message("consider binding the value with `let`, so that it lives longer")
        .build()
}

async fn borrowed_when_dropped_report(
    engine: &TrackedEngine,
    diagnostic: &BorrowedWhenDropped,
) -> Rendered<ByteIndex> {
    let dropped = Highlight::builder()
        .span(engine.to_absolute_span(&diagnostic.binding_span).await)
        .message("the value of this binding is dropped while the borrow is still in use")
        .build();
    let borrow = "what the dropped value may use is borrowed here".to_owned();

    Rendered::builder()
        .message("borrow may still be in use when the value is dropped")
        .primary_highlight(diagnostic.loan.borrow_highlight(engine, borrow).await)
        .related(
            std::iter::once(dropped)
                .chain(diagnostic.loan.later_use_highlight(engine).await)
                .collect(),
        )
        .build()
}

async fn returns_borrow_of_local_report(
    engine: &TrackedEngine,
    diagnostic: &ReturnsBorrowOfLocal,
) -> Rendered<ByteIndex> {
    let borrow = "the data is borrowed here".to_owned();

    Rendered::builder()
        .message("cannot return a value referencing data owned by the current function")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.return_span).await)
                .message("this returns a value referencing data owned by the current function")
                .build(),
        )
        .related(vec![diagnostic.loan.borrow_highlight(engine, borrow).await])
        .build()
}

async fn conflicting_borrow_report(
    engine: &TrackedEngine,
    diagnostic: &ConflictingBorrow,
) -> Rendered<ByteIndex> {
    let new = diagnostic.mutability.name();
    let existing = diagnostic.loan.mutability.name();

    // Two borrows of one kind are told apart by their order.
    let (message, new_label, existing_label) =
        if diagnostic.mutability == diagnostic.loan.mutability {
            (
                format!("cannot borrow a place as {new} more than once at a time"),
                format!("second {new} borrow occurs here"),
                format!("first {existing} borrow occurs here"),
            )
        } else {
            (
                format!("cannot borrow a place as {new} because it is also borrowed as {existing}"),
                format!("{new} borrow occurs here"),
                format!("{existing} borrow occurs here"),
            )
        };

    Rendered::builder()
        .message(message)
        .primary_highlight(diagnostic.borrow.highlight(engine, &new_label).await)
        .related(
            std::iter::once(diagnostic.loan.borrow_highlight(engine, existing_label).await)
                .chain(diagnostic.loan.later_use_highlight(engine).await)
                .collect(),
        )
        .build()
}

async fn does_not_live_long_enough_report(
    engine: &TrackedEngine,
    diagnostic: &DoesNotLiveLongEnough,
) -> Rendered<ByteIndex> {
    let scope_end = Highlight::builder()
        .span(engine.to_absolute_span(&diagnostic.binding_span).await)
        .message("this variable goes out of scope while it is still borrowed")
        .build();
    let borrow = "borrowed value does not live long enough".to_owned();

    Rendered::builder()
        .message("borrowed value does not live long enough")
        .primary_highlight(diagnostic.loan.borrow_highlight(engine, borrow).await)
        .related(
            std::iter::once(scope_end)
                .chain(diagnostic.loan.later_use_highlight(engine).await)
                .collect(),
        )
        .build()
}

async fn lifetime_may_not_live_long_enough_report(
    engine: &TrackedEngine,
    diagnostic: &LifetimeMayNotLiveLongEnough,
) -> Rendered<ByteIndex> {
    let longer = diagnostic.longer.display(engine).await.to_string();
    let shorter = diagnostic.shorter.display(engine).await.to_string();

    let mut origin = Vec::new();
    if let Some(origin_span) = &diagnostic.origin_span {
        origin.push(
            Highlight::builder()
                .span(engine.to_absolute_span(origin_span).await)
                .message(format!("the value of lifetime `{longer}` flows in here"))
                .build(),
        );
    }

    // `'static` is not a lifetime a where clause can make another outlive
    // in any useful way: the caller could then only pass `'static` data.
    let help = (!diagnostic.shorter.is_static_lifetime())
        .then(|| format!("consider adding `{longer}: {shorter}` to the where clause"));

    Rendered::builder()
        .message("lifetime may not live long enough")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.span).await)
                .message(format!("this requires `{longer}` to outlive `{shorter}`"))
                .build(),
        )
        .related(origin)
        .maybe_help_message(help)
        .build()
}

async fn type_may_not_live_long_enough_report(
    engine: &TrackedEngine,
    diagnostic: &TypeMayNotLiveLongEnough,
) -> Rendered<ByteIndex> {
    let subject = diagnostic.subject.display(engine).await.to_string();
    let bound = diagnostic.bound.display(engine).await.to_string();

    let mut related = Vec::new();
    if let Some(bound_span) = &diagnostic.bound_span {
        related.push(
            Highlight::builder()
                .span(engine.to_absolute_span(bound_span).await)
                .message(format!("the value is required to be valid for `{bound}` here"))
                .build(),
        );
    }

    Rendered::builder()
        .message(format!("the type `{subject}` may not live long enough"))
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.span).await)
                .message(format!("this requires `{subject}` to outlive `{bound}`"))
                .build(),
        )
        .related(related)
        .help_message(format!("consider adding `{subject}: {bound}` to the where clause"))
        .build()
}
