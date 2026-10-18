use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::obligation::PredicateObligation;
use rayc_semantic_element::drop_plan::linear_drop_help;
use rayc_solver::instance_resolution::InstanceResolutionError;
use rayc_symbol::{name::get_qualified_name, source_map::to_absolute_span};
use rayc_type::trait_ref::TraitRef;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct UseAfterMove {
    use_span: RelativeSpan,
    move_spans: Vec<RelativeSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct UseAfterPartialMove {
    use_span: RelativeSpan,
    move_spans: Vec<RelativeSpan>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct UseBeforeInitialization {
    use_span: RelativeSpan,
}

/// No `Drop` dictionary could be selected for a value dropped when its scope
/// ends.
///
/// The scope may also end because the function is dropped while it is
/// suspended. Every value held across a suspension point is dropped then,
/// even one that the function consumes later on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct UnresolvedScopeDrop {
    /// The declaration of the binding whose value is dropped.
    binding_span: RelativeSpan,
    trait_ref: TraitRef,
    error: InstanceResolutionError,

    /// The suspension point the value is held across, when it is dropped
    /// only because the function is abandoned there.
    suspension_span: Option<RelativeSpan>,
}

/// An operation handler moves a value out of one of its captures.
///
/// The handlers of a `run` share their captures across every call, so no call
/// may move out of them, even if it puts a value back before returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct MoveOutOfHandlerCapture {
    move_span: RelativeSpan,
    capture_span: RelativeSpan,
}

/// A value moved out of memory behind a reference (Rust E0507). The memory is
/// only borrowed, so it must still hold a value when the borrow ends.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct MoveOutOfBorrow {
    move_span: RelativeSpan,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable, From,
)]
pub enum Diagnostic {
    UseAfterMove(UseAfterMove),
    UseAfterPartialMove(UseAfterPartialMove),
    UseBeforeInitialization(UseBeforeInitialization),
    UnresolvedScopeDrop(UnresolvedScopeDrop),
    MoveOutOfHandlerCapture(MoveOutOfHandlerCapture),
    MoveOutOfBorrow(MoveOutOfBorrow),

    /// A selected `Drop` instance requires a where-clause predicate that does
    /// not hold at the dropping site.
    UnsatisfiedScopeDropPredicate(PredicateObligation),
}

async fn moved_value_report(
    engine: &TrackedEngine,
    use_span: &RelativeSpan,
    move_spans: &[RelativeSpan],
    partial: bool,
) -> Rendered<ByteIndex> {
    let related = futures::future::join_all(move_spans.iter().map(|span| async move {
        Highlight::builder()
            .span(engine.to_absolute_span(span).await)
            .message(if partial { "a component was moved here" } else { "value moved here" })
            .build()
    }))
    .await;

    Rendered::builder()
        .message(if partial { "use of partially moved value" } else { "use of moved value" })
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(use_span).await)
                .message(if partial {
                    "value used here after one of its components was moved"
                } else {
                    "value used here after it was moved"
                })
                .build(),
        )
        .related(related)
        .build()
}

async fn unresolved_scope_drop_report(
    engine: &TrackedEngine,
    diagnostic: &UnresolvedScopeDrop,
) -> Rendered<ByteIndex> {
    // Match the wording used for implicit instances selected during type
    // checking, so the same failure reads the same at every site.
    let message = match &diagnostic.error {
        InstanceResolutionError::NotReady(_) => "cannot infer instance requirement",
        InstanceResolutionError::ContainsError(_) => "instance requirement contains an error",
        InstanceResolutionError::NoInstance { .. } => "no implicit instance found",
        InstanceResolutionError::AmbiguousLexical { .. } => "ambiguous lexical instances",
        InstanceResolutionError::AmbiguousGlobal { .. } => "ambiguous global instances",
        InstanceResolutionError::Cycle(_) => "cyclic instance resolution",
        InstanceResolutionError::Limit { .. } => "instance resolution limit exceeded",
    };

    // The `Drop` dictionary of a coroutine is assembled from one dictionary
    // per argument, and the coroutine type cannot be written in source. The
    // argument that cannot be dropped is what the programmer can act on.
    let drops_coroutine = diagnostic.drops_coroutine();
    let trait_ref = match diagnostic.error.failed_requirement() {
        Some(failed) if drops_coroutine => failed,
        Some(_) | None => &diagnostic.trait_ref,
    };

    let name = engine.get_qualified_name(trait_ref.trait_id()).await;
    let mut args = Vec::new();
    for arg in trait_ref.args().iter() {
        args.push(arg.display(engine).await.to_string());
    }

    let linear_help = match &diagnostic.error {
        InstanceResolutionError::NoInstance { .. } => linear_drop_help(engine, trait_ref).await,
        InstanceResolutionError::NotReady(_)
        | InstanceResolutionError::ContainsError(_)
        | InstanceResolutionError::AmbiguousLexical { .. }
        | InstanceResolutionError::AmbiguousGlobal { .. }
        | InstanceResolutionError::Cycle(_)
        | InstanceResolutionError::Limit { .. } => None,
    };

    let mut related = Vec::new();
    let (binding_message, help) = if let Some(suspension_span) = &diagnostic.suspension_span {
        related.push(
            Highlight::builder()
                .span(engine.to_absolute_span(suspension_span).await)
                .message("it is dropped here if the coroutine is dropped while suspended")
                .build(),
        );
        let help = linear_help.unwrap_or_else(|| {
            "a value held across a suspension point needs a Drop instance, even if it is consumed \
             later"
                .to_owned()
        });
        ("this value is held across a suspension point", Some(help))
    } else if drops_coroutine {
        ("the arguments of this coroutine are dropped if it is never resumed", linear_help)
    } else {
        ("this value is dropped when it goes out of scope", linear_help)
    };

    Rendered::builder()
        .message(format!("{message}: `{name}[{}]`", args.join(", ")))
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.binding_span).await)
                .message(binding_message)
                .build(),
        )
        .related(related)
        .maybe_help_message(help)
        .build()
}

async fn move_out_of_handler_capture_report(
    engine: &TrackedEngine,
    diagnostic: &MoveOutOfHandlerCapture,
) -> Rendered<ByteIndex> {
    Rendered::builder()
        .message("cannot move out of a value captured by an operation handler")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.move_span).await)
                .message("value moved out here")
                .build(),
        )
        .related(vec![
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.capture_span).await)
                .message("every call to the handler shares this captured value")
                .build(),
        ])
        .build()
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::UseAfterMove(diagnostic) => {
                moved_value_report(engine, &diagnostic.use_span, &diagnostic.move_spans, false)
                    .await
            }
            Self::UseAfterPartialMove(diagnostic) => {
                moved_value_report(engine, &diagnostic.use_span, &diagnostic.move_spans, true).await
            }
            Self::UseBeforeInitialization(diagnostic) => Rendered::builder()
                .message("use of value before initialization")
                .primary_highlight(
                    Highlight::builder()
                        .span(engine.to_absolute_span(&diagnostic.use_span).await)
                        .message("value may be uninitialized here")
                        .build(),
                )
                .build(),
            Self::UnresolvedScopeDrop(diagnostic) => {
                unresolved_scope_drop_report(engine, diagnostic).await
            }
            Self::UnsatisfiedScopeDropPredicate(diagnostic) => diagnostic.report(engine).await,
            Self::MoveOutOfHandlerCapture(diagnostic) => {
                move_out_of_handler_capture_report(engine, diagnostic).await
            }
            Self::MoveOutOfBorrow(diagnostic) => Rendered::builder()
                .message("cannot move out of a borrow")
                .primary_highlight(
                    Highlight::builder()
                        .span(engine.to_absolute_span(&diagnostic.move_span).await)
                        .message("value moved out of a reference here")
                        .build(),
                )
                .help_message("the value is only borrowed; borrow it instead of moving it")
                .build(),
        }
    }
}

impl UseAfterMove {
    pub(crate) const fn new(use_span: RelativeSpan, move_spans: Vec<RelativeSpan>) -> Self {
        Self { use_span, move_spans }
    }
}

impl UseAfterPartialMove {
    pub(crate) const fn new(use_span: RelativeSpan, move_spans: Vec<RelativeSpan>) -> Self {
        Self { use_span, move_spans }
    }
}

impl UseBeforeInitialization {
    pub(crate) const fn new(use_span: RelativeSpan) -> Self { Self { use_span } }
}

impl UnresolvedScopeDrop {
    pub(crate) const fn new(
        binding_span: RelativeSpan,
        trait_ref: TraitRef,
        error: InstanceResolutionError,
        suspension_span: Option<RelativeSpan>,
    ) -> Self {
        Self { binding_span, trait_ref, error, suspension_span }
    }

    /// Returns whether the value that cannot be dropped is the coroutine a
    /// `coro` stores, rather than a binding declared in source.
    fn drops_coroutine(&self) -> bool {
        self.trait_ref
            .args()
            .interned_iter()
            .next()
            .is_some_and(|dropped| dropped.as_coroutine_view().is_some())
    }
}

impl MoveOutOfHandlerCapture {
    pub(crate) const fn new(move_span: RelativeSpan, capture_span: RelativeSpan) -> Self {
        Self { move_span, capture_span }
    }
}
