//! The one renderer of goals that could not be proved.

use qbice::{Decode, Encode, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Note, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{drop_plan::DropPlanError, parameter::get_parameter_map};
use rayc_symbol::{
    GlobalSymbolID, name::get_qualified_name, source_map::to_absolute_span, span::get_span,
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    trait_ref::TraitRef,
    ty::Ty,
};

use super::{CandidateFailure, DropComponent, Goal, Leaf, ProofFailure, Reason, Step};
use crate::instance_resolution::{
    InstanceResolutionCycle, InstanceResolutionFrame, InstanceResolutionLimit,
};

/// The most step notes shown for one chain. A longer chain shows its first
/// [`HEAD_STEPS`] and last [`TAIL_STEPS`] steps.
const MAX_STEPS: usize = 6;
const HEAD_STEPS: usize = 2;
const TAIL_STEPS: usize = 3;

/// The most failed candidates explained one by one.
const MAX_CANDIDATES: usize = 4;

/// The most goals shown of a search that stopped at a limit.
const MAX_LIMIT_GOALS: usize = 5;

/// A goal that could not be proved, reported where it was required.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct UnsatisfiedGoal {
    span: RelativeSpan,
    required_by: RequiredBy,
    failure: ProofFailure,
}

impl UnsatisfiedGoal {
    #[must_use]
    pub const fn new(span: RelativeSpan, required_by: RequiredBy, failure: ProofFailure) -> Self {
        Self { span, required_by, failure }
    }
}

/// Why the site required the goal. It decides the label at the primary span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum RequiredBy {
    /// An implicit given of a call or a path.
    ImplicitInstance,

    /// A value dropped when its scope ends, or, with a suspension span,
    /// because its function may be dropped while suspended there. Every
    /// value held across a suspension point is dropped then, even one that
    /// the function consumes later on.
    ScopeDrop { suspension: Option<RelativeSpan> },

    /// The arguments a `coro` stores, which are dropped if the coroutine is
    /// never resumed, or, with a suspension span, as for
    /// [`RequiredBy::ScopeDrop`].
    ///
    /// The coroutine type cannot be written in source, so the headline names
    /// the argument that cannot be dropped instead of the coroutine.
    CoroutineArguments { suspension: Option<RelativeSpan> },
}

impl RequiredBy {
    /// Returns the failure the headline is about.
    fn headline_failure(self, failure: &ProofFailure) -> &ProofFailure {
        match self {
            Self::CoroutineArguments { .. } => failure.innermost_part(),
            Self::ImplicitInstance | Self::ScopeDrop { .. } => failure,
        }
    }

    /// Returns the label of the primary span.
    const fn label(self) -> Option<&'static str> {
        match self {
            Self::ImplicitInstance => None,
            Self::ScopeDrop { suspension: Some(_) }
            | Self::CoroutineArguments { suspension: Some(_) } => {
                Some("this value is held across a suspension point")
            }
            Self::ScopeDrop { suspension: None } => {
                Some("this value is dropped when it goes out of scope")
            }
            Self::CoroutineArguments { suspension: None } => {
                Some("the arguments of this coroutine are dropped if it is never resumed")
            }
        }
    }

    /// Returns the suspension point the value is held across, if it is
    /// dropped only because the function is abandoned there.
    const fn suspension(self) -> Option<RelativeSpan> {
        match self {
            Self::ScopeDrop { suspension } | Self::CoroutineArguments { suspension } => suspension,
            Self::ImplicitInstance => None,
        }
    }
}

/// The kind of failure the headline states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Headline {
    NoInstance,
    AmbiguousLexical,
    AmbiguousGlobal,
    Cycle,
    Limit,
    NotReady,
}

impl Headline {
    const fn message(self) -> &'static str {
        match self {
            Self::NoInstance => "no implicit instance found",
            Self::AmbiguousLexical => "ambiguous lexical instances",
            Self::AmbiguousGlobal => "ambiguous global instances",
            Self::Cycle => "cyclic instance resolution",
            Self::Limit => "instance resolution limit exceeded",
            Self::NotReady => "cannot infer instance requirement",
        }
    }
}

impl Reason {
    /// Returns the headline of a goal that failed for this reason.
    const fn headline(&self) -> Headline {
        match self {
            Self::Requires { .. } | Self::CandidatesFailed(_) => Headline::NoInstance,
            Self::Leaf(leaf) => leaf.headline(),
        }
    }

    /// Describes how a goal that failed for this reason failed, after the
    /// goal itself, as in "`Show[Secret]` has no instance".
    const fn summary(&self) -> &'static str {
        match self {
            Self::Requires { .. } | Self::CandidatesFailed(_) => "has no instance that applies",
            Self::Leaf(leaf) => leaf.summary(),
        }
    }
}

impl Leaf {
    /// Returns the headline of a goal that failed for this leaf.
    const fn headline(&self) -> Headline {
        match self {
            Self::NoMatchingInstance
            | Self::UndeterminedParameter(_)
            | Self::LinearType
            | Self::DropPlan(_) => Headline::NoInstance,
            Self::AmbiguousLexical(_) => Headline::AmbiguousLexical,
            Self::AmbiguousGlobal(_) => Headline::AmbiguousGlobal,
            Self::Cycle(_) => Headline::Cycle,
            Self::Limit { .. } => Headline::Limit,
            Self::NotReady => Headline::NotReady,
        }
    }

    /// See [`Reason::summary`].
    const fn summary(&self) -> &'static str {
        match self {
            Self::NoMatchingInstance => "has no instance",
            Self::AmbiguousLexical(_) | Self::AmbiguousGlobal(_) => "has several instances",
            Self::UndeterminedParameter(_) => "has an instance that cannot be completed",
            Self::LinearType => "is linear",
            Self::DropPlan(_) => "has no generated `Drop` instance",
            Self::Cycle(_) => "requires itself",
            Self::Limit { .. } => "is too deep to resolve",
            Self::NotReady => "is not inferred yet",
        }
    }
}

impl ProofFailure {
    /// Returns the instance goal that failed.
    const fn trait_ref(&self) -> &TraitRef {
        let Goal::Instance(trait_ref) = self.goal();
        trait_ref
    }

    /// Returns the failure at the end of the chain of sub-goals.
    fn deepest(&self) -> &Self {
        match self.reason() {
            Reason::Requires { failure, .. } => failure.deepest(),
            Reason::CandidatesFailed(_) | Reason::Leaf(_) => self,
        }
    }
}

/// The notes, related highlights and help that explain a failure.
#[derive(Debug, Default)]
struct Explanation {
    notes: Vec<Note<ByteIndex>>,
    related: Vec<Highlight<ByteIndex>>,
    help: Option<String>,
}

impl Explanation {
    fn note(&mut self, message: String, highlight: Option<Highlight<ByteIndex>>) {
        self.notes
            .push(Note::builder().message(message).maybe_primary_highlight(highlight).build());
    }

    /// Explains the chain of steps from `failure` to its leaf, one note per
    /// step, followed by the leaf.
    async fn explain(&mut self, engine: &TrackedEngine, failure: &ProofFailure) {
        // Walk the chain down to the failure that ends it.
        let mut steps = Vec::new();
        let mut current = failure;
        let end = loop {
            match current.reason() {
                Reason::Requires { step, failure: child } => {
                    steps.push((current.trait_ref(), *step, child.trait_ref()));
                    current = child;
                }
                Reason::CandidatesFailed(_) | Reason::Leaf(_) => break current,
            }
        };

        // A long chain keeps its first steps, where it starts from the
        // site, and its last ones, which lead to the leaf.
        if steps.len() > MAX_STEPS {
            let omitted = steps.len() - HEAD_STEPS - TAIL_STEPS;
            for (parent, step, child) in &steps[..HEAD_STEPS] {
                self.step(engine, parent, *step, child).await;
            }
            self.note(format!("… {omitted} more requirements"), None);
            for (parent, step, child) in &steps[steps.len() - TAIL_STEPS..] {
                self.step(engine, parent, *step, child).await;
            }
        } else {
            for (parent, step, child) in &steps {
                self.step(engine, parent, *step, child).await;
            }
        }

        // The end of the chain is what the programmer acts on.
        let at_root = steps.is_empty();
        match end.reason() {
            Reason::CandidatesFailed(candidates) => self.candidates(engine, end, candidates).await,
            Reason::Leaf(leaf) => self.leaf(engine, end.trait_ref(), leaf, at_root).await,
            Reason::Requires { .. } => unreachable!("the chain ends at a failure of its own"),
        }
    }

    /// Explains why `parent` requires `child`.
    async fn step(
        &mut self,
        engine: &TrackedEngine,
        parent: &TraitRef,
        step: Step,
        child: &TraitRef,
    ) {
        let child_display = child.display(engine).await;
        let (message, highlight) = match step {
            Step::NestedInstanceGiven { instance_id, parameter } => {
                let (name, span) = poly_var(engine, parameter).await;
                let message = format!(
                    "`{}` matches `{}`, but its given `{name}` needs `{child_display}`",
                    engine.get_qualified_name(instance_id).await,
                    parent.display(engine).await,
                );
                (message, Some(highlight(engine, span).await))
            }
            Step::DropComponent(component) => {
                let owner = subject(engine, parent).await;
                let value = subject(engine, child).await;
                match component {
                    DropComponent::TupleElement(index) => (
                        format!(
                            "dropping `{owner}` drops its element `{index}`, of type `{value}`"
                        ),
                        None,
                    ),
                    DropComponent::ClosureCapture(_) => (
                        format!(
                            "dropping the closure drops a value it captures, of type `{value}`"
                        ),
                        None,
                    ),
                    DropComponent::CoroutineUpvar(index) => {
                        coroutine_upvar(engine, parent, index, &value).await
                    }
                    DropComponent::DropPlanRequirement(_) => (
                        format!(
                            "the generated `Drop` of `{owner}` drops a value of type `{value}`"
                        ),
                        None,
                    ),
                }
            }
            Step::ContinuationCoroutine => (
                format!(
                    "calling `{}` resumes its coroutine, which needs `{child_display}`",
                    subject(engine, parent).await
                ),
                None,
            ),
        };

        self.note(message, highlight);
    }

    /// Explains a goal that failed for a reason of its own.
    ///
    /// At the root, the headline already names the goal, so the leaf adds
    /// only what the headline does not say.
    async fn leaf(&mut self, engine: &TrackedEngine, goal: &TraitRef, leaf: &Leaf, at_root: bool) {
        match leaf {
            Leaf::NoMatchingInstance => {
                if !at_root {
                    self.note(
                        format!("no instance of `{}` exists", goal.display(engine).await),
                        None,
                    );
                }

                // A requirement on a type variable can be passed in by the
                // caller; one on a concrete type cannot.
                if goal.contains_poly_var() {
                    self.help = Some(format!("add `given (_: {})`", goal.display(engine).await));
                }
            }
            Leaf::UndeterminedParameter(parameter) => {
                let (name, span) = poly_var(engine, *parameter).await;
                self.note(
                    format!(
                        "`{}` matches `{}`, but does not determine its parameter `{name}`",
                        engine.get_qualified_name(parameter.parent_id()).await,
                        goal.display(engine).await,
                    ),
                    Some(highlight(engine, span).await),
                );
            }
            Leaf::AmbiguousGlobal(candidates) => {
                self.ambiguous_global(engine, goal, candidates, at_root).await;
            }
            Leaf::AmbiguousLexical(givens) => {
                self.ambiguous_lexical(engine, goal, givens, at_root).await;
            }
            Leaf::LinearType => {
                self.help = Some(format!(
                    "`{}` is a linear type: it has no Drop instance and must be consumed \
                     explicitly",
                    subject(engine, goal).await
                ));
            }
            Leaf::DropPlan(error) => self.drop_plan(engine, goal, error).await,
            Leaf::Cycle(cycle) => self.cycle(engine, cycle).await,
            Leaf::Limit { limit, recent_goals } => self.limit(engine, *limit, recent_goals).await,
            Leaf::NotReady => {}
        }
    }

    /// Points at each global instance that provides the goal.
    async fn ambiguous_global(
        &mut self,
        engine: &TrackedEngine,
        goal: &TraitRef,
        candidates: &[GlobalSymbolID],
        at_root: bool,
    ) {
        let mut highlights = Vec::new();
        for candidate in candidates {
            let message = format!("candidate `{}`", engine.get_qualified_name(*candidate).await);
            if let Some(span) = engine.get_span(*candidate).await {
                highlights.push(labeled(engine, span, message).await);
            }
        }
        self.ambiguity(engine, goal, "several instances provide", highlights, at_root).await;
    }

    /// Points at each given in scope that provides the goal.
    async fn ambiguous_lexical(
        &mut self,
        engine: &TrackedEngine,
        goal: &TraitRef,
        givens: &[GlobalPolyVarID],
        at_root: bool,
    ) {
        let mut highlights = Vec::new();
        for parameter in givens {
            let (name, span) = poly_var(engine, *parameter).await;
            highlights.push(labeled(engine, span, format!("given `{name}`")).await);
        }
        self.ambiguity(engine, goal, "several givens provide", highlights, at_root).await;
    }

    /// Follows a cycle around to the goal it repeats.
    async fn cycle(&mut self, engine: &TrackedEngine, cycle: &InstanceResolutionCycle) {
        let path = cycle.path();
        self.frames(engine, path).await;
        if let Some(repeated) = path.last() {
            self.note(
                format!(
                    "`{}` is required again while it is being resolved",
                    repeated.goal().display(engine).await
                ),
                None,
            );
        }
    }

    /// Shows the last goals of a search that stopped at a limit.
    async fn limit(
        &mut self,
        engine: &TrackedEngine,
        limit: InstanceResolutionLimit,
        recent_goals: &[InstanceResolutionFrame],
    ) {
        let shown = recent_goals.len().saturating_sub(MAX_LIMIT_GOALS);
        self.frames(engine, &recent_goals[shown..]).await;
        self.help = Some(match limit {
            InstanceResolutionLimit::Depth { limit } => {
                format!("the search stopped at its depth limit of {limit}")
            }
            InstanceResolutionLimit::CandidateVisits { limit, .. } => {
                format!("the search stopped after trying {limit} instances")
            }
        });
    }

    /// Shows the highlights of an ambiguity: in the diagnostic itself when
    /// the goal is the one in the headline, and in a note otherwise.
    async fn ambiguity(
        &mut self,
        engine: &TrackedEngine,
        goal: &TraitRef,
        message: &str,
        highlights: Vec<Highlight<ByteIndex>>,
        at_root: bool,
    ) {
        if at_root {
            self.related.extend(highlights);
        } else {
            self.notes.push(
                Note::builder()
                    .message(format!("{message} `{}`", goal.display(engine).await))
                    .related(highlights)
                    .build(),
            );
        }
    }

    /// Explains why the `Drop` plan of a struct cannot be generated.
    async fn drop_plan(&mut self, engine: &TrackedEngine, goal: &TraitRef, error: &DropPlanError) {
        let owner = subject(engine, goal).await;
        match error {
            DropPlanError::LinearField(field) => {
                self.help = Some(format!(
                    "`{owner}` has no generated Drop instance because it contains the linear type \
                     `{}`; consume it explicitly or implement Drop for it by hand",
                    field.display(engine).await
                ));
            }
            DropPlanError::MultipleInstances(instances) => {
                let mut highlights = Vec::new();
                for instance in instances {
                    if let Some(span) = engine.get_span(*instance).await {
                        let message =
                            format!("`{}` is one", engine.get_qualified_name(*instance).await);
                        highlights.push(labeled(engine, span, message).await);
                    }
                }
                self.notes.push(
                    Note::builder()
                        .message(format!("`{owner}` has several `Drop` instances"))
                        .related(highlights)
                        .build(),
                );
            }
            DropPlanError::InvalidExplicitInstance(instance) => {
                let span = match engine.get_span(*instance).await {
                    Some(span) => Some(highlight(engine, span).await),
                    None => None,
                };
                self.note(
                    format!(
                        "the `Drop` instance `{}` of `{owner}` is invalid",
                        engine.get_qualified_name(*instance).await
                    ),
                    span,
                );
            }
            DropPlanError::MissingFieldDictionary(field) => self.note(
                format!(
                    "the `Drop` of `{owner}` cannot be generated, because it stores a value of \
                     type `{}`, which has no `Drop`",
                    field.display(engine).await
                ),
                None,
            ),
            DropPlanError::NonConvergentRequirements => {
                self.note(format!("the `Drop` of `{owner}` cannot be generated"), None);
            }
        }
    }

    /// Explains why every matching instance head failed, one note per
    /// candidate, each naming the deepest goal it failed on.
    async fn candidates(
        &mut self,
        engine: &TrackedEngine,
        failure: &ProofFailure,
        candidates: &[CandidateFailure],
    ) {
        let goal = failure.trait_ref().display(engine).await.to_string();
        for candidate in candidates.iter().take(MAX_CANDIDATES) {
            let instance_id = candidate.instance_id();
            let instance = engine.get_qualified_name(instance_id).await;
            let message = match candidate.reason() {
                Reason::Requires { failure, .. } => {
                    let deepest = failure.deepest();
                    format!(
                        "`{instance}` matches `{goal}`, but needs `{}`, which {}",
                        deepest.trait_ref().display(engine).await,
                        deepest.reason().summary()
                    )
                }
                Reason::Leaf(Leaf::UndeterminedParameter(parameter)) => {
                    let (name, _) = poly_var(engine, *parameter).await;
                    format!(
                        "`{instance}` matches `{goal}`, but does not determine its parameter \
                         `{name}`"
                    )
                }
                reason @ (Reason::CandidatesFailed(_) | Reason::Leaf(_)) => {
                    format!("`{instance}` matches `{goal}`, but it {}", reason.summary())
                }
            };

            let span = match engine.get_span(instance_id).await {
                Some(span) => Some(highlight(engine, span).await),
                None => None,
            };
            self.note(message, span);
        }

        match candidates.len().saturating_sub(MAX_CANDIDATES) {
            0 => {}
            1 => self.note("and 1 more candidate".to_owned(), None),
            more => self.note(format!("and {more} more candidates"), None),
        }
    }

    /// Explains a path of goals the search entered, one note per step.
    async fn frames(&mut self, engine: &TrackedEngine, frames: &[InstanceResolutionFrame]) {
        for pair in frames.windows(2) {
            let [parent, child] = pair else { unreachable!("windows of two") };
            match child.introduced_by() {
                Some(step) => self.step(engine, parent.goal(), step, child.goal()).await,
                None => self.note(
                    format!(
                        "`{}` requires `{}`",
                        parent.goal().display(engine).await,
                        child.goal().display(engine).await
                    ),
                    None,
                ),
            }
        }
    }
}

/// Describes the value a trait reference is about: its only argument, such
/// as the type a `Drop` requirement drops, or the whole reference.
async fn subject(engine: &TrackedEngine, trait_ref: &TraitRef) -> String {
    let mut args = trait_ref.args().iter();
    match (args.next(), args.next()) {
        (Some(ty), None) => ty.display(engine).await.to_string(),
        (None | Some(_), _) => trait_ref.display(engine).await.to_string(),
    }
}

/// Returns the name and declaration of a polymorphic variable, such as a
/// given parameter.
async fn poly_var(engine: &TrackedEngine, parameter: GlobalPolyVarID) -> (String, RelativeSpan) {
    let map = engine.get_poly_var_map(parameter.parent_id()).await;
    (map.name_of(parameter.id()).to_owned(), map[parameter.id()].span())
}

/// Explains a dropped argument of a coroutine, at the parameter it was
/// passed for.
async fn coroutine_upvar(
    engine: &TrackedEngine,
    coroutine: &TraitRef,
    index: usize,
    value: &str,
) -> (String, Option<Highlight<ByteIndex>>) {
    let Some(view) = coroutine.args().iter().next().and_then(Ty::as_coroutine_view) else {
        return (format!("dropping the coroutine drops its argument of type `{value}`"), None);
    };

    let signature_id = view.signature_id(engine).await;
    let parameters = engine.get_parameter_map(signature_id).await;
    let message = format!(
        "dropping the coroutine of `{}` drops its argument of type `{value}`",
        engine.get_qualified_name(signature_id).await
    );
    let highlight = match parameters.iter().nth(index).and_then(|(_, parameter)| parameter.span()) {
        Some(span) => Some(highlight(engine, span).await),
        None => None,
    };

    (message, highlight)
}

async fn highlight(engine: &TrackedEngine, span: RelativeSpan) -> Highlight<ByteIndex> {
    Highlight::builder().span(engine.to_absolute_span(&span).await).build()
}

async fn labeled(
    engine: &TrackedEngine,
    span: RelativeSpan,
    message: String,
) -> Highlight<ByteIndex> {
    Highlight::builder().span(engine.to_absolute_span(&span).await).message(message).build()
}

impl Report for UnsatisfiedGoal {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        // The headline states what failed, taken from the innermost part of
        // a compiler-built dictionary, and names the goal the site asked for.
        let failure = self.required_by.headline_failure(&self.failure);
        let headline = self.failure.innermost_part().reason().headline();
        let message =
            format!("{}: `{}`", headline.message(), failure.trait_ref().display(engine).await);

        // The primary span says why the site required the goal.
        let primary = Highlight::builder()
            .span(engine.to_absolute_span(&self.span).await)
            .maybe_message(self.required_by.label())
            .build();

        // A value held across a suspension point is dropped there if the
        // function is abandoned, which needs explaining.
        let mut explanation = Explanation::default();
        if let Some(suspension) = self.required_by.suspension() {
            explanation.related.push(
                labeled(
                    engine,
                    suspension,
                    "it is dropped here if the coroutine is dropped while suspended".to_owned(),
                )
                .await,
            );
        }

        // The notes follow the chain from the goal down to its leaf.
        explanation.explain(engine, failure).await;
        if self.required_by.suspension().is_some() && explanation.help.is_none() {
            explanation.help = Some(
                "a value held across a suspension point needs a Drop instance, even if it is \
                 consumed later"
                    .to_owned(),
            );
        }

        Rendered::builder()
            .message(message)
            .primary_highlight(primary)
            .related(explanation.related)
            .notes(explanation.notes)
            .maybe_help_message(explanation.help)
            .build()
    }
}
