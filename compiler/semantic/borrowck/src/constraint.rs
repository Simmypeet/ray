//! The localized outlives constraints of an IR function: each `'a: 'b` required
//! by the instruction at `p` is an edge `'a@p -> 'b@p`. Also records the loans
//! the function issues and the type tests it requires.

use qbice::storage::intern::Interned;
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::{Address, Local},
    cfg::{BlockID, Instruction, Point, Terminator},
    ir_expr::{IRExprID, IRExprKind},
    ir_function::{FunctionID, IRFunction, IRFunctionMap},
    ir_lambda::CaptureMap,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::Solver;
use rayc_type::{
    constraint::ty_relate::TyRelate,
    ty::{Mutability, Ty},
    variance::Variance,
    where_clause::OutlivesPredicate,
};

use crate::requirement::NestedRequirements;

mod instance;
mod invocation;
mod nested;
mod place;
mod predicate;
mod value;

/// Identifies a loan issued in an IR function.
pub type LoanID = ID<Loan>;

/// A borrow issued in an IR function, as `&'r place` or `&'r mut place`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loan {
    /// The lifetime of the reference the borrow creates.
    region: Interned<Ty>,

    /// The point of the borrow.
    point: Point,

    /// The borrowed place.
    address: Address,

    /// Whether the borrow is shared or mutable.
    mutability: Mutability,

    /// The source of the borrow expression.
    span: RelativeSpan,

    /// The projection depth of the innermost struct owning the borrowed place
    /// that has a declared `Drop` instance, if there is one.
    declared_drop_depth: Option<usize>,
}

impl Loan {
    /// Returns the lifetime of the reference the borrow creates.
    #[must_use]
    pub const fn region(&self) -> &Interned<Ty> { &self.region }

    /// Returns the point of the borrow.
    #[must_use]
    pub const fn point(&self) -> Point { self.point }

    /// Returns the borrowed place.
    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    /// Returns whether the borrow is shared or mutable.
    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }

    /// Returns the source of the borrow expression.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    /// Returns whether dropping the value in `dropped` may use the borrowed
    /// place: its storage, or anything a struct with a declared `Drop` instance
    /// can reach.
    #[must_use]
    pub fn is_used_by_drop_of(&self, dropped: &Address) -> bool {
        if self.address.contains(dropped) || dropped.holds(&self.address) {
            return true;
        }

        dropped.contains(&self.address)
            && self.declared_drop_depth.is_some_and(|depth| dropped.projections().len() <= depth)
    }
}

/// A requirement `subject: 'bound` on a type parameter or a rigid projection,
/// which no outlives constraint between regions can express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeTest {
    /// The type parameter or projection required to outlive `bound`.
    subject: Interned<Ty>,

    /// The lifetime `subject` must outlive.
    bound: Interned<Ty>,

    /// The point of the instruction requiring it.
    point: Point,

    /// The source requiring it, when a nested function created at `point` does.
    blame: Option<RelativeSpan>,
}

impl TypeTest {
    /// Returns the type parameter or projection required to outlive the
    /// bound.
    #[must_use]
    pub const fn subject(&self) -> &Interned<Ty> { &self.subject }

    /// Returns the lifetime the subject must outlive.
    #[must_use]
    pub const fn bound(&self) -> &Interned<Ty> { &self.bound }

    /// Returns the point of the instruction requiring the test.
    #[must_use]
    pub const fn point(&self) -> Point { self.point }

    /// Returns the source requiring the test in `function`, if it has one.
    #[must_use]
    pub fn span(&self, function: &IRFunction) -> Option<RelativeSpan> {
        self.blame.or_else(|| function.point_span(self.point))
    }
}

/// An outlives constraint `'lesser: 'greater` that an IR function requires at
/// a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outlives<'a> {
    lesser: &'a Interned<Ty>,
    greater: &'a Interned<Ty>,
    point: Point,
    blame: Option<RelativeSpan>,
}

impl<'a> Outlives<'a> {
    /// Returns the region required to outlive the other.
    #[must_use]
    pub const fn lesser(&self) -> &'a Interned<Ty> { self.lesser }

    /// Returns the region the lesser region is required to outlive.
    #[must_use]
    pub const fn greater(&self) -> &'a Interned<Ty> { self.greater }

    /// Returns the point of the instruction requiring the constraint.
    #[must_use]
    pub const fn point(&self) -> Point { self.point }

    /// Returns the source requiring the constraint, when a nested function
    /// created at its point does.
    #[must_use]
    pub const fn blame(&self) -> Option<RelativeSpan> { self.blame }
}

/// A region at a point: a node of the localized constraint graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LocalizedRegion {
    region: Interned<Ty>,
    point: Point,
}

/// The outlives constraints, type tests and loans of an IR function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalizedConstraints {
    /// For each region at a point, the regions it must outlive there, each with
    /// the source requiring it when a nested function created there does.
    edges: FxHashMap<LocalizedRegion, FxHashMap<Interned<Ty>, Option<RelativeSpan>>>,

    /// The type tests the instructions require, in collection order.
    type_tests: Vec<TypeTest>,

    /// The loans issued by the borrows of the function.
    loans: Arena<Loan>,

    /// The loan issued by each borrow expression.
    loans_by_ref_of_id: FxHashMap<IRExprID, LoanID>,

    /// The loans of the places in each local, dereferences included.
    loans_by_local: FxHashMap<Local, Vec<LoanID>>,
}

impl LocalizedConstraints {
    /// Collects the constraints and loans of every reachable instruction of the
    /// function `function_id`, whose nested functions must be in `nested`.
    pub async fn collect(
        ir: &IRFunctionMap,
        function_id: FunctionID,
        effect: &Interned<Ty>,
        nested: &NestedRequirements,
        solver: &mut Solver,
    ) -> Self {
        let function = ir.get_function(function_id);
        let mut collector = ConstraintCollector {
            ir,
            function,
            captures: ir.captures_for_function(function_id),
            effect,
            nested,
            solver,
            constraints: Self::default(),
        };

        let reachables = function.reachables();
        for block_id in reachables.blocks() {
            for (point, instruction) in function.block_instructions_with_points(block_id) {
                collector.collect_instruction(point, instruction).await;
            }

            collector.collect_terminator(block_id).await;
        }

        collector.constraints
    }

    /// Iterates over the regions that `region` must outlive at `point`.
    pub fn outlived_regions(
        &self,
        region: &Interned<Ty>,
        point: Point,
    ) -> impl Iterator<Item = &Interned<Ty>> {
        self.edges
            .get(&LocalizedRegion { region: region.clone(), point })
            .into_iter()
            .flat_map(FxHashMap::keys)
    }

    /// Iterates over every outlives constraint, in unspecified order.
    pub fn outlives(&self) -> impl Iterator<Item = Outlives<'_>> {
        self.edges.iter().flat_map(|(lesser, greaters)| {
            greaters.iter().map(move |(greater, &blame)| Outlives {
                lesser: &lesser.region,
                greater,
                point: lesser.point,
                blame,
            })
        })
    }

    /// Iterates over the type tests, in the order they were collected.
    #[must_use]
    pub fn type_tests(&self) -> impl ExactSizeIterator<Item = &TypeTest> { self.type_tests.iter() }

    /// Iterates over the loans issued in the function, in unspecified order. A
    /// borrow behind a shared reference or a raw pointer issues none.
    #[must_use]
    pub fn loans(&self) -> impl ExactSizeIterator<Item = (LoanID, &Loan)> { self.loans.iter() }

    /// Returns the loan identified by `id`.
    #[must_use]
    pub fn get_loan(&self, id: LoanID) -> &Loan { self.loans.get(id).expect("loan should exist") }

    /// Returns the ID of the loan issued by the borrow `expression_id`, if any.
    #[must_use]
    pub fn loan_id_of_ref_of(&self, expression_id: IRExprID) -> Option<LoanID> {
        self.loans_by_ref_of_id.get(&expression_id).copied()
    }

    /// Returns the loan issued by the borrow `expression_id`, if any.
    #[must_use]
    pub fn loan_of_ref_of(&self, expression_id: IRExprID) -> Option<&Loan> {
        let loan_id = self.loans_by_ref_of_id.get(&expression_id)?;
        self.loans.get(*loan_id)
    }

    /// Iterates over the loans of the places in `local`, dereferences included.
    pub fn loans_of_local(&self, local: Local) -> impl Iterator<Item = LoanID> + '_ {
        self.loans_by_local.get(&local).into_iter().flatten().copied()
    }

    /// Records `loan`, issued by the borrow expression `expression_id`.
    fn issue_loan(&mut self, expression_id: IRExprID, loan: Loan) {
        // A loan is only issued for a place with a type, which an error
        // address does not have.
        let local = loan.address.local().expect("a loan should borrow a place of a local");

        let loan_id = self.loans.insert(loan);
        self.loans_by_ref_of_id.insert(expression_id, loan_id);
        self.loans_by_local.entry(local).or_default().push(loan_id);
    }

    /// Adds the edge `'lesser@point -> 'greater@point` for `predicate`.
    fn add(&mut self, point: Point, predicate: &OutlivesPredicate) {
        self.add_blaming(point, predicate, None);
    }

    /// As [`Self::add`], blaming the source `blame` when there is one.
    fn add_blaming(
        &mut self,
        point: Point,
        predicate: &OutlivesPredicate,
        blame: Option<RelativeSpan>,
    ) {
        let required_by = self
            .edges
            .entry(LocalizedRegion { region: predicate.lesser().clone(), point })
            .or_default()
            .entry(predicate.greater().clone())
            .or_default();

        // A constraint may be required more than once: the first source blamed
        // for it is kept.
        *required_by = required_by.or(blame);
    }

    /// Records the type test `subject: 'bound` required at `point`.
    fn add_type_test(
        &mut self,
        point: Point,
        subject: Interned<Ty>,
        bound: Interned<Ty>,
        blame: Option<RelativeSpan>,
    ) {
        self.type_tests.push(TypeTest { subject, bound, point, blame });
    }
}

/// Walks the instructions of an IR function for
/// [`LocalizedConstraints::collect`].
struct ConstraintCollector<'a> {
    /// The IR functions of the definition `function` belongs to.
    ir: &'a IRFunctionMap,
    function: &'a IRFunction,

    /// The capture layout of a nested function.
    captures: Option<&'a CaptureMap>,

    /// The effect row of the function.
    effect: &'a Interned<Ty>,

    /// What the nested functions that `function` creates require of it.
    nested: &'a NestedRequirements,
    solver: &'a mut Solver,
    constraints: LocalizedConstraints,
}

impl ConstraintCollector<'_> {
    /// Collects the constraints of the instruction at `point`.
    #[allow(clippy::match_same_arms)]
    async fn collect_instruction(&mut self, point: Point, instruction: &Instruction) {
        match instruction {
            Instruction::Expression(expression_id) => {
                self.collect_expression(point, *expression_id).await;
            }
            Instruction::Store(store) => self.collect_store(point, store).await,

            // A borrow live past the storage of its place is a conflict, not a
            // constraint.
            Instruction::ScopePush(_) | Instruction::ScopePop(_) => {}

            Instruction::ExprDiscard(discard) => {
                let value_ty = self.function.get_expression(discard.expression()).ty();
                self.collect_drop(point, value_ty, discard.drop_instance()).await;
            }
            Instruction::AddressDrop(drop) => {
                let Some(place_ty) = self.place_type(point, drop.address()).await else {
                    return;
                };

                self.collect_drop(point, &place_ty, drop.drop_instance()).await;
            }
        }
    }

    /// Collects the constraints of the terminator of `block_id`.
    async fn collect_terminator(&mut self, block_id: BlockID) {
        match self.function.block_terminator(block_id) {
            Some(Terminator::Return(Some(value))) => {
                let point = self.function.terminator_point(block_id);
                self.collect_return(point, *value).await;
            }

            // Unit holds no region, and the values a jump passes are related by
            // the phis of its target.
            Some(Terminator::Return(None) | Terminator::Jump(_) | Terminator::Conditional(_))
            | None => {}
        }
    }

    /// Collects the constraints of evaluating `expression_id` at `point`.
    #[allow(clippy::match_same_arms)]
    async fn collect_expression(&mut self, point: Point, expression_id: IRExprID) {
        let expression = self.function.get_expression(expression_id);

        match expression.kind() {
            IRExprKind::RefOf(ref_of) => {
                self.collect_borrow(point, expression_id, ref_of, expression.ty()).await;
            }
            IRExprKind::Load(load) => self.collect_load(point, load, expression.ty()).await,
            IRExprKind::Call(call) => self.collect_call(point, call, expression.ty()).await,
            IRExprKind::Perform(perform) => {
                self.collect_perform(point, perform, expression.ty()).await;
            }
            IRExprKind::Tuple(tuple) => self.collect_tuple(point, tuple, expression.ty()).await,
            IRExprKind::Closure(closure) => {
                self.collect_closure(point, closure, expression.ty()).await;
            }

            // The incoming values are related where they flow in, not here.
            IRExprKind::Phi(phi) => self.collect_phi(phi, expression.ty()).await,
            IRExprKind::StructInitialization(initialization) => {
                self.collect_struct_initialization(point, initialization, expression.ty()).await;
            }

            // Neither reads a place nor relates two values.
            IRExprKind::Error | IRExprKind::Literal(_) => {}

            // The memory behind a raw pointer is not tracked.
            IRExprKind::RefToPointer(_) => {}

            // Primitives hold no region.
            IRExprKind::Binary(_) => {}

            IRExprKind::Handle(handle) => {
                self.collect_handle(point, handle, expression.ty()).await;
            }
        }
    }

    /// Adds, at `point`, the outlives constraints of relating `lesser` to
    /// `greater` with `variance`.
    async fn relate(
        &mut self,
        point: Point,
        lesser: &Interned<Ty>,
        greater: &Interned<Ty>,
        variance: Variance,
    ) {
        // The two types only differ in lifetimes, so this only fails on an
        // error that was reported already.
        let relation = TyRelate::new(lesser.clone(), greater.clone(), variance);
        let Some(outlives) = self.solver.solve_without_unify(vec![relation]).await else {
            return;
        };

        for constraint in outlives.iter() {
            self.constraints.add(point, constraint);
        }
    }
}
