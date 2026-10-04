//! Checks the relations an IR function requires between its universal regions:
//! `'a: 'b` whenever its constraints lead from `'a` to `'b`. One the
//! environment does not entail is an error, or a requirement of the creator.

use qbice::storage::intern::Interned;
use rayc_ir::ir_function::IRFunction;
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::Solver;
use rayc_type::ty::Ty;

use crate::{
    diagnostic::{Diagnostic, HandlerCaptureEscapes, LifetimeMayNotLiveLongEnough},
    requirement::ExternalRequirements,
    subset_graph::{Reached, SubsetGraph},
};

/// Returns the relations between universal regions that `function` requires but
/// may not assume. One with an external lifetime is added to `requirements`
/// instead.
pub(crate) fn check_universal_regions(
    function: &IRFunction,
    graph: &SubsetGraph,
    solver: &Solver,
    requirements: &mut ExternalRequirements,
) -> Vec<Diagnostic> {
    let checker = UniversalRegionChecker { function, solver, graph };

    checker.check(requirements)
}

/// Checks the universal regions of an IR function for
/// [`check_universal_regions`].
struct UniversalRegionChecker<'a> {
    function: &'a IRFunction,
    solver: &'a Solver,
    graph: &'a SubsetGraph,
}

impl UniversalRegionChecker<'_> {
    /// Returns the errors found, adding what is left for the creator to prove
    /// to `requirements`.
    fn check(&self, requirements: &mut ExternalRequirements) -> Vec<Diagnostic> {
        let environment = self.solver.outlives_environment();
        let mut diagnostics = Vec::new();

        for longer in self.graph.universals() {
            // Nearly every function requires nothing it may not assume,
            // which the closure of the graph tells without a search.
            if self
                .graph
                .reachable_universals(longer)
                .all(|shorter| environment.region_outlives(longer, shorter))
            {
                continue;
            }

            // Search for the relations that do not hold, and for the
            // constraints that require them.
            let reached = self.graph.reach_universals_from(longer);

            for shorter in reached.universals() {
                if environment.region_outlives(longer, shorter) {
                    continue;
                }

                let source = self.source(&reached, shorter);

                // A run of an operation handler borrows its environment for a
                // lifetime nobody chooses, so nothing can make it outlive
                // another.
                if self.function.environment_lifetime() == Some(longer) {
                    diagnostics.push(Self::explain_escape(source));
                    continue;
                }

                // The creator of a nested function chooses its external
                // lifetimes, so only the creator can tell what they outlive.
                if longer.is_external_lifetime() || shorter.is_external_lifetime() {
                    requirements.require(longer.clone(), shorter.clone(), source.span);
                } else {
                    diagnostics.push(Self::explain(longer, shorter, source));
                }
            }
        }

        diagnostics
    }

    /// Returns where the relation between the source of `reached` and `shorter`
    /// arises: the last and the first constraint of the path between them.
    fn source(&self, reached: &Reached<'_>, shorter: &Interned<Ty>) -> RequirementSource {
        let mut spans = reached.path_spans(shorter, self.function).into_iter().flatten();

        let origin_span =
            spans.next().expect("a constraint path between universal regions should have a source");
        let span = spans.next_back().unwrap_or(origin_span);

        RequirementSource { span, origin_span }
    }

    /// Describes the unproven requirement `longer: shorter` arising at
    /// `source`.
    fn explain(
        longer: &Interned<Ty>,
        shorter: &Interned<Ty>,
        source: RequirementSource,
    ) -> Diagnostic {
        Diagnostic::LifetimeMayNotLiveLongEnough(LifetimeMayNotLiveLongEnough::new(
            source.span,
            longer.clone(),
            shorter.clone(),
            (source.origin_span != source.span).then_some(source.origin_span),
        ))
    }

    /// Describes a borrow of a capture that leaves the run of an operation
    /// handler, arising at `source`.
    fn explain_escape(source: RequirementSource) -> Diagnostic {
        Diagnostic::HandlerCaptureEscapes(HandlerCaptureEscapes::new(
            source.span,
            (source.origin_span != source.span).then_some(source.origin_span),
        ))
    }
}

/// Where a relation between two universal regions arises in a function.
#[derive(Debug, Clone, Copy)]
struct RequirementSource {
    /// The last constraint of the path: where the value flows into the shorter
    /// lifetime.
    span: RelativeSpan,

    /// The first constraint of the path: where the value comes from.
    origin_span: RelativeSpan,
}
