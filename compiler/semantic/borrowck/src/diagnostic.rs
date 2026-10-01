//! The errors the borrow checker reports.
//!
//! Each error is an access that invalidates a loan while the loan is still
//! live, and points at three places: the access that invalidates the loan,
//! the borrow that issued it, and a later use of the borrow, which is why the
//! loan is still live. A loan that outlives the function, such as one stored
//! behind a parameter, has no later use within it.

use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::source_map::to_absolute_span;
use rayc_type::ty::Mutability;

/// The loan an access conflicts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ConflictingLoan {
    /// The borrow that issued the loan.
    borrow_span: RelativeSpan,

    /// Whether the loan is shared or mutable.
    mutability: Mutability,

    /// A use of the borrow after the conflicting access, if the borrow is used
    /// within the function.
    later_use_span: Option<RelativeSpan>,
}

impl ConflictingLoan {
    pub(crate) const fn new(
        borrow_span: RelativeSpan,
        mutability: Mutability,
        later_use_span: Option<RelativeSpan>,
    ) -> Self {
        Self { borrow_span, mutability, later_use_span }
    }

    /// Returns the highlight of the borrow, labelled `message`.
    async fn borrow_highlight(
        &self,
        engine: &TrackedEngine,
        message: String,
    ) -> Highlight<ByteIndex> {
        Highlight::builder()
            .span(engine.to_absolute_span(&self.borrow_span).await)
            .message(message)
            .build()
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
    borrow_span: RelativeSpan,
    mutability: Mutability,
    loan: ConflictingLoan,
}

impl ConflictingBorrow {
    pub(crate) const fn new(
        borrow_span: RelativeSpan,
        mutability: Mutability,
        loan: ConflictingLoan,
    ) -> Self {
        Self { borrow_span, mutability, loan }
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

/// An error found by the borrow checker.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    AssignToBorrowed(AssignToBorrowed),
    ConflictingBorrow(ConflictingBorrow),
    DoesNotLiveLongEnough(DoesNotLiveLongEnough),
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
        }
    }
}

async fn assign_to_borrowed_report(
    engine: &TrackedEngine,
    diagnostic: &AssignToBorrowed,
) -> Rendered<ByteIndex> {
    let borrow = format!("{} borrow occurs here", mutability_name(diagnostic.loan.mutability));

    Rendered::builder()
        .message("cannot assign to a place because it is borrowed")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.assign_span).await)
                .message("the borrowed place is assigned here")
                .build(),
        )
        .related(
            std::iter::once(diagnostic.loan.borrow_highlight(engine, borrow).await)
                .chain(diagnostic.loan.later_use_highlight(engine).await)
                .collect(),
        )
        .build()
}

async fn conflicting_borrow_report(
    engine: &TrackedEngine,
    diagnostic: &ConflictingBorrow,
) -> Rendered<ByteIndex> {
    let new = mutability_name(diagnostic.mutability);
    let existing = mutability_name(diagnostic.loan.mutability);

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
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.borrow_span).await)
                .message(new_label)
                .build(),
        )
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

/// Returns how a borrow of `mutability` is named in messages.
const fn mutability_name(mutability: Mutability) -> &'static str {
    match mutability {
        Mutability::Immutable => "immutable",
        Mutability::Mutable => "mutable",
    }
}
