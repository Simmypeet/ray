//! Checks each access of an IR function against the loans active just before
//! it.
//!
//! An access conflicts with an active loan when:
//!
//! - it assigns a place that overlaps the borrowed place;
//! - it borrows a place that overlaps the borrowed place, and at least one of
//!   the two borrows is mutable;
//! - it ends the scope of a variable whose own storage is borrowed. A borrow of
//!   the memory behind a pointer held in the variable does not end there.
//!
//! Reads and drops of borrowed places are not checked yet.
//!
//! Each conflict is reported with the borrow of the loan and a later use of
//! it. The later use is searched for forward from the access, through the
//! points where a region holding the loan stays live, until an instruction
//! uses the value that owns the region.

use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_ir::{
    address::{Address, Local},
    cfg::{Instruction, Point, Store},
    ir_expr::IRExprID,
    ir_function::IRFunction,
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::{Mutability, Ty};

use crate::{
    active_loans::{ActiveLoans, LoanActivity},
    constraint::{Loan, LoanID, LocalizedConstraints},
    diagnostic::{
        AssignToBorrowed, ConflictingBorrow, ConflictingLoan, Diagnostic, DoesNotLiveLongEnough,
    },
    live_loans::Traversal,
    region_liveness::RegionLiveness,
};

/// Checks every reachable access of `function` against the loans active
/// before it, and returns the conflicts found.
pub(crate) fn check_conflicts(
    function: &IRFunction,
    constraints: &LocalizedConstraints,
    liveness: &RegionLiveness,
    traversal: &Traversal<'_>,
    activity: &LoanActivity<'_>,
) -> Vec<Diagnostic> {
    let mut checker =
        ConflictChecker { function, constraints, liveness, traversal, diagnostics: Vec::new() };

    let reachables = function.reachables();
    for block_id in reachables.blocks() {
        activity.visit_block(block_id, |point, instruction, active| {
            if let Some(instruction) = instruction {
                checker.check_instruction(point, instruction, active);
            }
        });
    }

    checker.diagnostics
}

/// Checks the accesses of an IR function for [`check_conflicts`].
struct ConflictChecker<'a> {
    function: &'a IRFunction,
    constraints: &'a LocalizedConstraints,
    liveness: &'a RegionLiveness,
    traversal: &'a Traversal<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl ConflictChecker<'_> {
    /// Checks the accesses of the instruction at `point` against the loans
    /// in `active`.
    #[allow(clippy::match_same_arms)]
    fn check_instruction(&mut self, point: Point, instruction: &Instruction, active: &ActiveLoans) {
        match instruction {
            Instruction::Store(store) => self.check_store(point, store, active),
            Instruction::Expression(expression_id) => {
                self.check_expression(point, *expression_id, active);
            }
            Instruction::ScopePop(scope_id) => self.check_scope_end(point, *scope_id, active),

            // A new scope has no storage in use yet.
            Instruction::ScopePush(_) => {}

            // TODO: a drop of a borrowed place invalidates its loans, which is
            // not checked yet.
            Instruction::ExprDiscard(_) | Instruction::AddressDrop(_) => {}
        }
    }

    /// Checks that `store` does not assign a borrowed place.
    fn check_store(&mut self, point: Point, store: &Store, active: &ActiveLoans) {
        let Some(loan) =
            self.first_conflict(active, |loan| loan.address().overlaps(store.address()))
        else {
            return;
        };

        let loan = self.conflicting_loan(loan, point);
        self.diagnostics
            .push(Diagnostic::AssignToBorrowed(AssignToBorrowed::new(store.span(), loan)));
    }

    /// Checks that the expression `expression_id`, if it is a borrow, does not
    /// borrow a place in a way that conflicts with a loan of it.
    ///
    /// Reads of borrowed places are not checked yet, so only borrows are.
    fn check_expression(&mut self, point: Point, expression_id: IRExprID, active: &ActiveLoans) {
        let Some(borrow) = self.constraints.load_of_ref_of(expression_id) else {
            return;
        };

        // Two borrows conflict unless both are shared.
        let conflicts = |loan: &Loan| {
            loan.address().overlaps(borrow.address())
                && (borrow.mutability() == Mutability::Mutable
                    || loan.mutability() == Mutability::Mutable)
        };

        let Some(loan) = self.first_conflict(active, conflicts) else {
            return;
        };

        let loan = self.conflicting_loan(loan, point);
        self.diagnostics.push(Diagnostic::ConflictingBorrow(ConflictingBorrow::new(
            borrow.span(),
            borrow.mutability(),
            loan,
        )));
    }

    /// Checks that the variables whose scope `scope_id` ends are not borrowed
    /// there.
    fn check_scope_end(&mut self, point: Point, scope_id: ScopeID, active: &ActiveLoans) {
        for variable_id in self.function.declared_variables(scope_id) {
            let local = Local::Variable(variable_id);
            let owns_place = |address: &Address| address.direct_local() == Some(local);
            let Some(loan) = self.first_conflict(active, |loan| owns_place(loan.address())) else {
                continue;
            };

            let binding_span = self.function.get_variable(variable_id).span();
            let loan = self.conflicting_loan(loan, point);
            self.diagnostics.push(Diagnostic::DoesNotLiveLongEnough(DoesNotLiveLongEnough::new(
                binding_span,
                loan,
            )));
        }
    }

    /// Returns the first loan in `active`, in the order they were issued,
    /// that `conflicts` with the access being checked.
    ///
    /// Only one conflict is reported per access, and taking the first keeps
    /// the report deterministic.
    fn first_conflict(
        &self,
        active: &ActiveLoans,
        mut conflicts: impl FnMut(&Loan) -> bool,
    ) -> Option<LoanID> {
        active.iter().filter(|&loan| conflicts(self.constraints.get_loan(loan))).min()
    }

    /// Describes the loan `loan_id`, which an access at `point` conflicts
    /// with.
    fn conflicting_loan(&self, loan_id: LoanID, point: Point) -> ConflictingLoan {
        let loan = self.constraints.get_loan(loan_id);
        ConflictingLoan::new(loan.span(), loan.mutability(), self.later_use(loan, point))
    }

    /// Returns the source of a use of `loan` after `point`, where it is live.
    ///
    /// The loan is live at `point` because it flows into regions live there;
    /// the first of them whose value is used later gives the use.
    fn later_use(&self, loan: &Loan, point: Point) -> Option<RelativeSpan> {
        let mut regions = self.traversal.regions_holding(loan, point);
        regions.find_map(|region| self.first_use_after(&region, point))
    }

    /// Returns the source of the first use of the value owning `region` that
    /// the control flow reaches after `start` while `region` stays live.
    fn first_use_after(&self, region: &Interned<Ty>, start: Point) -> Option<RelativeSpan> {
        let mut pending: VecDeque<Point> = self.function.successor_points(start).collect();
        let mut visited = FxHashSet::default();

        while let Some(point) = pending.pop_front() {
            // A dead region holds no loan, so no use down this path can be
            // why the loan is live.
            if !visited.insert(point) || !self.liveness.is_live(region, point) {
                continue;
            }

            if self.liveness.is_used_at(self.function, region, point) {
                return self.function.point_span(point);
            }
            pending.extend(self.function.successor_points(point));
        }

        None
    }
}
