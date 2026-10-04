//! Checks the type tests of an IR function: the outlives environment must
//! entail `p: 'u` for each universal region `'u` the bound of a test `p: 'r`
//! stands for. One it does not is an error, or a requirement of the creator.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_ir::ir_function::IRFunction;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_type::{
    rewrite::{RewriteAsync, TyRewriterAsync},
    ty::{Ty, lifetime::Lifetime},
    where_clause::OutlivesPredicate,
};

use crate::{
    constraint::{LocalizedConstraints, TypeTest},
    diagnostic::{Diagnostic, TypeMayNotLiveLongEnough},
    requirement::ExternalRequirements,
    subset_graph::{Reached, SubsetGraph},
};

/// Returns the type tests of `constraints` that the outlives environment does
/// not entail. One about an external lifetime is added to `requirements`
/// instead.
pub(crate) async fn check_type_tests(
    function: &IRFunction,
    constraints: &LocalizedConstraints,
    graph: &SubsetGraph,
    solver: &mut Solver,
    requirements: &mut ExternalRequirements,
) -> Vec<Diagnostic> {
    let mut checker = TypeTestChecker {
        function,
        graph,
        solver,
        requirements,
        handled: FxHashSet::default(),
        diagnostics: Vec::new(),
    };

    for test in constraints.type_tests() {
        checker.check(test).await;
    }

    checker.diagnostics
}

/// Checks the type tests of an IR function for [`check_type_tests`].
struct TypeTestChecker<'a> {
    function: &'a IRFunction,
    graph: &'a SubsetGraph,
    solver: &'a mut Solver,

    /// What the function leaves for its creator to prove.
    requirements: &'a mut ExternalRequirements,

    /// The subject, universal region and source of each test handled so far,
    /// since one call often requires the same thing more than once.
    handled: FxHashSet<(Interned<Ty>, Interned<Ty>, RelativeSpan)>,

    /// The errors found so far.
    diagnostics: Vec<Diagnostic>,
}

impl TypeTestChecker<'_> {
    /// Checks that the subject of `test` outlives each universal region its
    /// bound stands for.
    async fn check(&mut self, test: &TypeTest) {
        // A universal bound is asked of the environment as it is.
        if test.bound().is_universal_region() {
            if !self.entails(test.subject(), test.bound()).await {
                self.fail(test, test.bound(), None).await;
            }

            return;
        }

        // A bound of the body stands for the universal regions it must outlive;
        // the path to them is only searched for to explain an error.
        let graph = self.graph;
        let mut reached = None;

        for universal in graph.reachable_universals(test.bound()) {
            if self.entails(test.subject(), universal).await {
                continue;
            }

            let reached = reached.get_or_insert_with(|| graph.reach_universals_from(test.bound()));
            self.fail(test, universal, Some(reached)).await;
        }
    }

    /// Returns whether the environment entails `subject: 'universal`.
    async fn entails(&mut self, subject: &Interned<Ty>, universal: &Interned<Ty>) -> bool {
        // The graph tells how the regions of the body in a projection relate
        // to the universal regions a fact is about.
        let predicate = OutlivesPredicate::new(subject.clone(), universal.clone());
        self.solver.entails_outlives_with(&predicate, self.graph).await
    }

    /// Handles the subject of `test` not being known to outlive `universal`:
    /// required of the creator when about an external lifetime, and an error
    /// otherwise. Each is handled once per source.
    async fn fail(
        &mut self,
        test: &TypeTest,
        universal: &Interned<Ty>,
        reached: Option<&Reached<'_>>,
    ) {
        let span = test
            .span(self.function)
            .expect("an instruction requiring a type test should have a source");

        if !self.handled.insert((test.subject().clone(), universal.clone(), span)) {
            return;
        }

        // State the subject over universal regions, which the creator of
        // the function knows, and the programmer wrote.
        let engine = self.solver.engine();
        let mut promoter = RegionPromoter { graph: self.graph, engine, is_promoted: true };
        let subject = test.subject().rewrite_async_or_clone(&mut promoter, engine).await;

        // The creator of a nested function chooses its external lifetimes,
        // so only the creator can tell what outlives them.
        let is_external = universal.is_external_lifetime()
            || subject.recursive_iter().any(Ty::is_external_lifetime);

        if promoter.is_promoted && is_external {
            self.requirements.require(subject, universal.clone(), span);
            return;
        }

        // Point at where the bound is required to outlive the universal
        // region too, when another instruction requires that.
        let bound_span = reached
            .and_then(|reached| reached.path_spans(universal, self.function).last().copied())
            .flatten()
            .filter(|&bound_span| bound_span != span);

        self.diagnostics.push(Diagnostic::TypeMayNotLiveLongEnough(TypeMayNotLiveLongEnough::new(
            span,
            subject,
            universal.clone(),
            bound_span,
        )));
    }
}

/// Replaces each region of the body in a type with a universal region the
/// constraints make the same lifetime, or with the erased lifetime if none.
struct RegionPromoter<'a> {
    graph: &'a SubsetGraph,
    engine: &'a TrackedEngine,

    /// Whether every region of the body met so far was replaced with a
    /// universal region.
    is_promoted: bool,
}

impl TyRewriterAsync for RegionPromoter<'_> {
    async fn rewrite(&mut self, ty: &Interned<Ty>) -> Option<Interned<Ty>> {
        if !ty.is_lifetime(self.engine).await || ty.is_universal_region() {
            return None;
        }

        let promoted = self.graph.equal_universal(ty).cloned();
        self.is_promoted &= promoted.is_some();

        Some(promoted.unwrap_or_else(|| Ty::new_lifetime(Lifetime::Erased, self.engine)))
    }
}
