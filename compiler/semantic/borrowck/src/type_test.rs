//! Checks the type tests of an IR function.
//!
//! A type test `p: 'r` requires a type parameter or a rigid projection `p` to
//! outlive a lifetime. No constraint between regions expresses it: `p` holds
//! no region of the body, and only the outlives environment of the function,
//! its where clause and the bounds implied by its signature, can tell what
//! `p` outlives.
//!
//! When `'r` is a universal region, the environment must entail `p: 'r`.
//! When `'r` is a region of the body, every type in scope is valid throughout
//! the body, so `'r` only asks more of `p` than that through the universal
//! regions it must outlive: the environment must entail `p: 'u` for every
//! universal region `'u` reachable from `'r` in the location-insensitive
//! constraint graph. A region that reaches none is satisfied by any type.
//!
//! A projection may mention regions of the body, as `i.Assoc['r]` does, which
//! no fact of the environment mentions. The arguments of a projection are
//! invariant, so a fact about `i.Assoc['x]` applies only when the constraints
//! of the body make `'r` and `'x` the same lifetime: when each must outlive
//! the other. A projection is never proven from what it projects from.
//!
//! The environment states nothing of an external lifetime: the function
//! creating the nested function chooses it. So a test the environment does
//! not entail, whose bound is an external lifetime or whose subject mentions
//! one, is not an error, but a [requirement](crate::requirement) for the
//! creator to prove, where it creates the nested function. The creator knows
//! no region of the body, so the subject is first stated over universal
//! regions: each region of the body in it is replaced with a universal region
//! the constraints make the same lifetime. A subject with a region that is
//! the same lifetime as none cannot be required of the creator, and is an
//! error (rustc: `try_promote_type_test_subject`).
//!
//! An error names a projection the same way, by the universal regions its
//! regions are the same lifetime as, since a region of the body has no name
//! to show.

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

/// Checks that every type test of `constraints` follows from the outlives
/// environment, and returns the ones that do not.
///
/// A test about an external lifetime is added to `requirements` instead, for
/// the function creating `function` to prove.
///
/// `graph` must be built from `constraints`, and `solver` must be created at
/// the definition `function` belongs to.
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

    /// The subject, the universal region and the source of each test that
    /// was reported, or required of the creator, so far. The predicates of
    /// one call often require the same thing more than once.
    handled: FxHashSet<(Interned<Ty>, Interned<Ty>, RelativeSpan)>,

    /// The errors found so far.
    diagnostics: Vec<Diagnostic>,
}

impl TypeTestChecker<'_> {
    /// Checks that the subject of `test` outlives each universal region its
    /// bound stands for. Each one the environment does not entail is required
    /// of the creator of the function, or recorded as an error.
    async fn check(&mut self, test: &TypeTest) {
        // A universal bound is asked of the environment as it is.
        if test.bound().is_universal_region() {
            if !self.entails(test.subject(), test.bound()).await {
                self.fail(test, test.bound(), None).await;
            }

            return;
        }

        // A bound of the body stands for the universal regions it must
        // outlive. The search for the constraints that require it to is left
        // for when there is an error to explain.
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

    /// Handles the subject of `test` not being known to outlive `universal`,
    /// which is the bound of the test or a universal region `reached` from
    /// it, unless that was handled for the same source already.
    ///
    /// When the predicate is about an external lifetime, it is required of
    /// the creator of the function. Otherwise, it is recorded as an error.
    ///
    /// # Panics
    ///
    /// Panics if `test` has no source. A test is required by a where clause,
    /// which only an expression or a drop proves, or by a nested function.
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
/// constraints make the same lifetime, to state the type over lifetimes that
/// are known outside the body.
///
/// A region that is the same lifetime as no universal region is replaced with
/// the erased lifetime, which is only fit for showing the type in an error.
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
