//! Checks each access of an IR function against the loans active just before
//! it.
//!
//! An access conflicts with an active loan when:
//!
//! - it assigns a place whose storage is borrowed, or which lies within the
//!   borrowed place. A borrow of the memory behind a pointer held in the
//!   assigned place is not invalidated: the assignment only replaces the
//!   pointer.
//! - it borrows a place that overlaps the borrowed place, and at least one of
//!   the two borrows is mutable;
//! - it reads a place that overlaps the borrowed place, to copy its value, and
//!   the loan is mutable;
//! - it moves a value out of a place that overlaps the borrowed place;
//! - it drops a value whose `Drop` dictionary may use the borrowed place; see
//!   [`Loan::is_used_by_drop_of`];
//! - it ends the scope of a variable whose own storage is borrowed. A borrow of
//!   the memory behind a pointer held in the variable does not end there.
//! - it ends the root scope of the function while the storage of a parameter or
//!   a capture the function owns is borrowed. That storage ends with the
//!   function, and only a loan held by a universal region, or by the returned
//!   value, is still live there.
//!
//! The operands a closure or a `handle` captures are borrows, reads and moves
//! like any other. Their errors point at the expression that creates the
//! nested function.
//!
//! Each conflict is reported with the borrow of the loan and a later use of
//! it. The later use is searched for forward from the access, through the
//! points where a region holding the loan stays live, until an instruction
//! uses the value that owns the region.
//!
//! A value is dropped right before its place is overwritten, or its storage
//! ends. A loan that both the drop and what follows it conflict with is
//! reported once, by what follows the drop.

use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Local},
    cfg::{AddressDrop, BlockID, Instruction, Point, Store, Terminator},
    ir_expr::{
        IRExprID, IRExprKind,
        load::{Load, LoadEffect},
    },
    ir_function::{IRContext, IRFunction},
    ir_lambda::CaptureMap,
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_solver::Solver;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{
    ty::{Mutability, Ty},
    where_clause::MarkerPredicate,
};

use crate::{
    active_loans::{ActiveLoans, LoanActivity},
    constraint::{Loan, LoanID, LocalizedConstraints},
    diagnostic::{
        AccessSite, AssignToBorrowed, BorrowedWhenDropped, ConflictingBorrow, ConflictingLoan,
        Diagnostic, DoesNotLiveLongEnough, MoveOfBorrowed, ReturnsBorrowOfLocal,
        TemporaryDroppedWhileBorrowed, UseOfMutablyBorrowed,
    },
    live_loans::Traversal,
    region_liveness::RegionLiveness,
};

/// Checks every reachable access of `function` against the loans active
/// before it, and returns the conflicts found.
///
/// `captures` is the capture layout of a nested function, and `None` for the
/// definition function. `solver` must be created at the definition the
/// function belongs to.
pub(crate) async fn check_conflicts(
    function: &IRFunction,
    captures: Option<&CaptureMap>,
    constraints: &LocalizedConstraints,
    liveness: &RegionLiveness,
    traversal: &Traversal<'_>,
    activity: &LoanActivity<'_>,
    solver: &mut Solver,
) -> Vec<Diagnostic> {
    let mut checker =
        ConflictChecker::new(function, captures, constraints, liveness, traversal, solver);

    let reachables = function.reachables();
    for block_id in reachables.blocks() {
        activity
            .visit_block(block_id, async |point, instruction, active| {
                // A terminator accesses no place.
                if let Some(instruction) = instruction {
                    checker.check_instruction(point, instruction, active).await;
                }
            })
            .await;
    }

    checker.finish()
}

/// Returns the inputs of `function` whose storage ends with its root scope:
/// its parameters, followed by the captures it owns.
///
/// An operation handler only borrows its captures, which the function
/// creating it drops after the handled body.
async fn owned_inputs(
    function: &IRFunction,
    captures: Option<&CaptureMap>,
    solver: &Solver,
) -> Vec<Local> {
    let capture_locals = || {
        captures
            .into_iter()
            .flat_map(|captures| captures.iter().map(|(capture_id, _)| Local::Capture(capture_id)))
    };

    match function.context() {
        IRContext::Def => {
            let parameters = solver.engine().get_parameter_map(solver.site()).await;
            parameters.iter().map(|(parameter_id, _)| Local::Parameter(parameter_id)).collect()
        }
        IRContext::Lambda(context) => context
            .parameters()
            .map(|(parameter_id, _)| Local::LambdaParameter(parameter_id))
            .chain(capture_locals())
            .collect(),
        IRContext::Thunk(_) => capture_locals().collect(),
        IRContext::OperationHandler(context) => context
            .parameters()
            .map(|(parameter_id, _)| Local::OperationHandlerParameter(parameter_id))
            .collect(),
    }
}

/// A loan that a drop conflicts with.
struct DroppedLoan {
    loan_id: LoanID,

    /// The local the dropped place is in.
    local: Local,

    diagnostic: Diagnostic,
}

/// Checks the accesses of an IR function for [`check_conflicts`].
struct ConflictChecker<'a> {
    function: &'a IRFunction,

    /// The capture layout of a nested function.
    captures: Option<&'a CaptureMap>,

    constraints: &'a LocalizedConstraints,
    liveness: &'a RegionLiveness,
    traversal: &'a Traversal<'a>,

    /// Decides whether the type of a loaded value is `Copy`, and finds where
    /// a parameter is declared.
    solver: &'a mut Solver,

    /// For each operand captured into a nested function, the expression
    /// creating that function.
    capture_sites: FxHashMap<IRExprID, RelativeSpan>,

    /// For each loan issued by a captured operand, the expression creating
    /// the nested function that captures it.
    captured_loans: FxHashMap<LoanID, RelativeSpan>,

    /// The loans reported where a place of a local is overwritten or its
    /// storage ends, which a drop of the value in the place is not reported
    /// for again.
    ended: FxHashSet<(LoanID, Local)>,

    /// The loans that drops conflict with, in the order they were found.
    dropped: Vec<DroppedLoan>,

    diagnostics: Vec<Diagnostic>,
}

impl<'a> ConflictChecker<'a> {
    fn new(
        function: &'a IRFunction,
        captures: Option<&'a CaptureMap>,
        constraints: &'a LocalizedConstraints,
        liveness: &'a RegionLiveness,
        traversal: &'a Traversal<'a>,
        solver: &'a mut Solver,
    ) -> Self {
        // Find the operands captured into nested functions, and the loans
        // they issue.
        let mut capture_sites = FxHashMap::default();
        let mut captured_loans = FxHashMap::default();

        let reachables = function.reachables();
        for block_id in reachables.blocks() {
            for instruction in function.block_instructions(block_id) {
                let Instruction::Expression(expression_id) = instruction else {
                    continue;
                };

                let expression = function.get_expression(*expression_id);
                for operand in expression.kind().capture_operands() {
                    capture_sites.insert(operand, expression.span());

                    if let Some(loan_id) = constraints.loan_id_of_ref_of(operand) {
                        captured_loans.insert(loan_id, expression.span());
                    }
                }
            }
        }

        Self {
            function,
            captures,
            constraints,
            liveness,
            traversal,
            solver,
            capture_sites,
            captured_loans,
            ended: FxHashSet::default(),
            dropped: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    /// Returns the errors found, once every access is checked.
    fn finish(mut self) -> Vec<Diagnostic> {
        // A drop is only reported for the loans that nothing following it
        // was reported for.
        for dropped in std::mem::take(&mut self.dropped) {
            if self.ended.insert((dropped.loan_id, dropped.local)) {
                self.diagnostics.push(dropped.diagnostic);
            }
        }

        self.diagnostics
    }

    /// Checks the accesses of the instruction at `point` against the loans
    /// in `active`.
    async fn check_instruction(
        &mut self,
        point: Point,
        instruction: &Instruction,
        active: &ActiveLoans,
    ) {
        match instruction {
            Instruction::Store(store) => self.check_store(point, store, active),
            Instruction::Expression(expression_id) => {
                self.check_expression(point, *expression_id, active).await;
            }
            Instruction::ScopePop(scope_id) => {
                self.check_scope_end(point, *scope_id, active).await;
            }
            Instruction::AddressDrop(drop) => self.check_drop(point, drop, active),

            // A new scope has no storage in use yet, and a discarded value is
            // held in no place a loan could be of.
            Instruction::ScopePush(_) | Instruction::ExprDiscard(_) => {}
        }
    }

    /// Checks that `store` does not assign a borrowed place.
    fn check_store(&mut self, point: Point, store: &Store, active: &ActiveLoans) {
        // The assignment only writes the storage of the place: what a pointer
        // held there points to stays as it is.
        let assigned = store.address();

        //
        // The first condition is an assignment within the borrowed place:
        //
        //     let r = pair.&
        //     pair.0 = 1        # `pair` contains `pair.0`
        //
        // The second is an assignment of a place that the borrowed place is
        // stored in:
        //
        //     let r = pair.0.&
        //     pair = (3, 4)     # `pair` holds `pair.0`
        //
        // It leaves out a borrowed place which is only pointed to:
        //
        //     let r = p.*.&mut
        //     p = other.&mut    # `p` does not hold `p.*`
        let conflicts =
            |loan: &Loan| loan.address().contains(assigned) || assigned.holds(loan.address());

        let Some(loan_id) = self.first_conflict(active, conflicts) else {
            return;
        };

        if let Some(local) = assigned.local() {
            self.ended.insert((loan_id, local));
        }

        let loan = self.conflicting_loan(loan_id, point);
        self.diagnostics
            .push(Diagnostic::AssignToBorrowed(AssignToBorrowed::new(store.span(), loan)));
    }

    /// Checks the accesses of the expression `expression_id`: a borrow or a
    /// load of a borrowed place.
    async fn check_expression(
        &mut self,
        point: Point,
        expression_id: IRExprID,
        active: &ActiveLoans,
    ) {
        let function = self.function;
        let expression = function.get_expression(expression_id);

        match expression.kind() {
            IRExprKind::RefOf(ref_of) => {
                // A raw borrow creates no reference, so it is not checked.
                if let Some(reference) = expression.ty().as_reference_view() {
                    self.check_borrow(
                        point,
                        expression_id,
                        ref_of.address(),
                        reference.mutability(),
                        active,
                    );
                }
            }
            IRExprKind::Load(load) => {
                let moves = self.load_moves(load, expression.ty()).await;
                self.check_load(point, expression_id, load, moves, active);
            }

            // These only take evaluated values as operands, and access no
            // place.
            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefToPointer(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Call(_)
            | IRExprKind::Perform(_)
            | IRExprKind::Handle(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::Closure(_)
            | IRExprKind::StructInitialization(_) => {}
        }
    }

    /// Checks that the borrow `expression_id` of `borrowed`, as a reference
    /// of `mutability`, does not conflict with a loan of the place.
    fn check_borrow(
        &mut self,
        point: Point,
        expression_id: IRExprID,
        borrowed: &Address,
        mutability: Mutability,
        active: &ActiveLoans,
    ) {
        // Two borrows conflict unless both are shared.
        let conflicts = |loan: &Loan| {
            loan.address().overlaps(borrowed)
                && (mutability == Mutability::Mutable || loan.mutability() == Mutability::Mutable)
        };

        let Some(loan_id) = self.first_conflict(active, conflicts) else {
            return;
        };

        let loan = self.conflicting_loan(loan_id, point);
        self.diagnostics.push(Diagnostic::ConflictingBorrow(ConflictingBorrow::new(
            self.access_site(expression_id),
            mutability,
            loan,
        )));
    }

    /// Returns whether `load`, producing a value of type `ty`, moves the
    /// value out of its place. A load that does not copies the value.
    ///
    /// The memory analysis decides it the same way; see [`Load::effect`].
    async fn load_moves(&mut self, load: &Load, ty: &Interned<Ty>) -> bool {
        match load.effect() {
            LoadEffect::Copies => false,
            LoadEffect::Moves => true,
            LoadEffect::MovesUnlessCopy => {
                let marker_id = self.solver.engine().get_core_item(CoreItem::Copy).await;
                let is_copy = MarkerPredicate::new(marker_id, ty.clone());
                !self.solver.entails_marker_predicate(is_copy).await
            }
        }
    }

    /// Checks that the load `expression_id` neither moves a value out of a
    /// borrowed place, when it `moves`, nor reads a mutably borrowed one.
    fn check_load(
        &mut self,
        point: Point,
        expression_id: IRExprID,
        load: &Load,
        moves: bool,
        active: &ActiveLoans,
    ) {
        let loaded = load.address();

        // A move leaves the place without a value, which conflicts with any
        // loan of it.
        if moves {
            let Some(loan_id) = self.first_conflict(active, |loan| loan.address().overlaps(loaded))
            else {
                return;
            };

            let conflict = self.conflicting_loan(loan_id, point);
            self.diagnostics.push(Diagnostic::MoveOfBorrowed(MoveOfBorrowed::new(
                self.access_site(expression_id),
                conflict,
            )));
        } else {
            // A copy only reads the place, which a shared loan allows.
            let conflicts = |loan: &Loan| {
                loan.mutability() == Mutability::Mutable && loan.address().overlaps(loaded)
            };
            let Some(loan_id) = self.first_conflict(active, conflicts) else {
                return;
            };

            let conflict = self.conflicting_loan(loan_id, point);
            self.diagnostics.push(Diagnostic::UseOfMutablyBorrowed(UseOfMutablyBorrowed::new(
                self.access_site(expression_id),
                conflict,
            )));
        }
    }

    /// Checks that `drop` does not drop a value whose `Drop` dictionary may
    /// use a borrowed place.
    fn check_drop(&mut self, point: Point, drop: &AddressDrop, active: &ActiveLoans) {
        // A no-op dictionary uses nothing of the value.
        if drop.drop_instance().is_no_op_drop_instance() {
            return;
        }

        let Some(local) = drop.address().local() else {
            return;
        };
        let uses = |loan: &Loan| loan.is_used_by_drop_of(drop.address());
        let Some(loan_id) = self.first_conflict(active, uses) else {
            return;
        };

        // Whether to report it is decided once what follows the drop is
        // checked.
        let loan = self.conflicting_loan(loan_id, point);
        self.dropped.push(DroppedLoan {
            loan_id,
            local,
            diagnostic: Diagnostic::BorrowedWhenDropped(BorrowedWhenDropped::new(
                drop.span(),
                loan,
            )),
        });
    }

    /// Checks that the storage ending with the scope `scope_id` is not
    /// borrowed there: that of the variables the scope declares, and, for
    /// the root scope, that of the function inputs.
    async fn check_scope_end(&mut self, point: Point, scope_id: ScopeID, active: &ActiveLoans) {
        let function = self.function;
        let variables = function.declared_variables(scope_id).map(Local::Variable);
        let inputs = if scope_id == function.root_scope_id() {
            Some(owned_inputs(function, self.captures, self.solver).await)
        } else {
            None
        };

        for local in variables.chain(inputs.into_iter().flatten()) {
            let owns_place = |address: &Address| address.direct_local() == Some(local);
            let Some(loan_id) = self.first_conflict(active, |loan| owns_place(loan.address()))
            else {
                continue;
            };

            // A scope ends on each path that leaves it, and a loan is
            // reported for one of them only.
            if self.ended.insert((loan_id, local)) {
                self.report_storage_end(point, local, loan_id).await;
            }
        }
    }

    /// Reports the loan `loan_id` of the storage of `local`, which ends at
    /// `point` while the loan is live.
    async fn report_storage_end(&mut self, point: Point, local: Local, loan_id: LoanID) {
        let loan = self.conflicting_loan(loan_id, point);
        let returned_span =
            self.returned_span(self.constraints.get_loan(loan_id), point.block_id());

        // The scopes of a function are unwound right before it returns, so a
        // loan the returned value holds is live here.
        let diagnostic = match returned_span {
            Some(returned_span) => {
                Diagnostic::ReturnsBorrowOfLocal(ReturnsBorrowOfLocal::new(returned_span, loan))
            }
            None if self.is_temporary(local) => Diagnostic::TemporaryDroppedWhileBorrowed(
                TemporaryDroppedWhileBorrowed::new(self.local_span(local).await, loan),
            ),
            None => Diagnostic::DoesNotLiveLongEnough(DoesNotLiveLongEnough::new(
                self.local_span(local).await,
                loan,
            )),
        };
        self.diagnostics.push(diagnostic);
    }

    /// Returns whether `local` is a temporary the compiler introduced.
    fn is_temporary(&self, local: Local) -> bool {
        match local {
            Local::Variable(variable_id) => self.function.get_variable(variable_id).is_temporary(),
            Local::Parameter(_)
            | Local::LambdaParameter(_)
            | Local::OperationHandlerParameter(_)
            | Local::Capture(_) => false,
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

    /// Returns where the expression `expression_id` accesses its place: at
    /// the expression itself, or, for a captured operand, at the expression
    /// creating the nested function.
    fn access_site(&self, expression_id: IRExprID) -> AccessSite {
        match self.capture_sites.get(&expression_id) {
            Some(&capture_span) => AccessSite::new(capture_span, true),
            None => AccessSite::new(self.function.get_expression(expression_id).span(), false),
        }
    }

    /// Describes the loan `loan_id`, which an access at `point` conflicts
    /// with.
    fn conflicting_loan(&self, loan_id: LoanID, point: Point) -> ConflictingLoan {
        let loan = self.constraints.get_loan(loan_id);
        let borrow = match self.captured_loans.get(&loan_id) {
            Some(&capture_span) => AccessSite::new(capture_span, true),
            None => AccessSite::new(loan.span(), false),
        };

        ConflictingLoan::new(borrow, loan.mutability(), self.later_use(loan, point))
    }

    /// Returns the source of the value `block_id` returns, when its
    /// terminator is a return and that value holds `loan`.
    fn returned_span(&self, loan: &Loan, block_id: BlockID) -> Option<RelativeSpan> {
        let value =
            self.function.block_terminator(block_id).and_then(Terminator::returned_value)?;
        let value = self.function.get_expression(value);

        // The loan is returned when it flows into a region of the type of
        // the returned value.
        let point = self.function.terminator_point(block_id);
        let is_returned = self
            .traversal
            .regions_holding(loan, point)
            .any(|region| value.ty().recursive_iter().any(|ty| *ty == *region));

        is_returned.then(|| value.span())
    }

    /// Returns the declaration of `local`.
    async fn local_span(&self, local: Local) -> RelativeSpan {
        match local {
            Local::Variable(variable_id) => self.function.get_variable(variable_id).span(),
            Local::Parameter(parameter_id) => {
                self.solver.engine().get_parameter_map(self.solver.site()).await[parameter_id]
                    .span()
                    .expect("parameters of a function with a body are declared in source")
            }
            Local::LambdaParameter(parameter_id) => self
                .function
                .context()
                .assert_as_lambda_context()
                .get_parameter(parameter_id)
                .span(),
            Local::OperationHandlerParameter(parameter_id) => self
                .function
                .context()
                .assert_as_operation_handler_context()
                .get_parameter(parameter_id)
                .span(),
            Local::Capture(capture_id) => self
                .captures
                .expect("capture address roots should have a capture layout")
                .get_capture(capture_id)
                .span(),
        }
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
