//! The constraints of proving the where clause of a declaration.

use rayc_ir::cfg::Point;
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::givens::get_givens;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    outlives::OutlivesComponent,
    subst::{Subst, Substitutable},
    ty::Ty,
    variance::Variance,
    where_clause::{OutlivesPredicate, PredicateKind},
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Requires, at `point`, the predicates that `symbol_id` assumes,
    /// instantiated with `substitution`: its where clause and implied bounds,
    /// and those of the declarations enclosing it.
    pub(super) async fn collect_where_clause(
        &mut self,
        point: Point,
        symbol_id: GlobalSymbolID,
        substitution: &Subst,
    ) {
        let predicates = self.solver.engine().get_givens(symbol_id).await;

        for predicate in predicates.iter() {
            let predicate = predicate.apply_subst_or_clone(substitution, self.solver.engine());
            self.collect_predicate(point, &predicate).await;
        }
    }

    /// Adds, at `point`, the outlives constraints that proving the
    /// instantiated where-clause `predicate` requires.
    pub(super) async fn collect_predicate(&mut self, point: Point, predicate: &PredicateKind) {
        match predicate {
            // The two sides were proven equal modulo lifetimes by type
            // checking; their lifetimes must be equal too.
            PredicateKind::AssociatedTypeEquality(equality) => {
                self.relate(point, equality.left(), equality.right(), Variance::Invariant).await;
            }

            PredicateKind::Outlives(outlives) => self.collect_outlives(point, outlives).await,

            // Lifetimes never decide whether a type satisfies a marker, so
            // type checking proved it in full.
            PredicateKind::Marker(_) => {}
        }
    }

    /// Adds, at `point`, the requirements of the instantiated predicate
    /// `subject: bound`: an outlives constraint for each lifetime in `subject`,
    /// and a type test for each type parameter and projection.
    pub(super) async fn collect_outlives(&mut self, point: Point, predicate: &OutlivesPredicate) {
        self.collect_outlives_blaming(point, predicate, None).await;
    }

    /// As [`Self::collect_outlives`], blaming the source `blame` when there is
    /// one.
    pub(super) async fn collect_outlives_blaming(
        &mut self,
        point: Point,
        predicate: &OutlivesPredicate,
        blame: Option<RelativeSpan>,
    ) {
        // Normalizing the subject may use a given equality that matches it
        // up to lifetimes, which then requires those lifetimes to be equal.
        let (subject, outlives) = self.solver.normalize_with_outlives(predicate.lesser()).await;
        for constraint in outlives.iter() {
            self.constraints.add_blaming(point, constraint, blame);
        }

        for component in Ty::outlives_components(&subject, self.solver.engine()).await {
            match component {
                OutlivesComponent::Region(region) => {
                    self.constraints.add_blaming(
                        point,
                        &OutlivesPredicate::new(region, predicate.greater().clone()),
                        blame,
                    );
                }

                // Only the outlives environment can tell whether a type
                // parameter or a projection outlives `bound`.
                OutlivesComponent::Opaque(subject) => {
                    self.constraints.add_type_test(
                        point,
                        subject,
                        predicate.greater().clone(),
                        blame,
                    );
                }
            }
        }
    }
}
