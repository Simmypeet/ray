//! The localized outlives constraints of an IR function.
//!
//! Polonius finds where a loan is live by walking a graph whose nodes are a
//! region at a point, written `'r@p`. This module collects the part of that
//! graph which the instructions of a function state: each outlives constraint
//! `'a: 'b` required by the instruction at `p` becomes an edge
//! `'a@p -> 'b@p`, along which loans flow from `'a` into `'b`. The liveness
//! edges between points are not stored; the traversal derives them from the
//! control-flow graph and region liveness, so the graph is never built in
//! full.
//!
//! It also records the loans the function issues: one per borrow of a place
//! the function may be kept from accessing, whose region is the lifetime of
//! the reference the borrow creates. A place behind a shared reference or a
//! raw pointer issues none; see [`LocalizedConstraints::loans`].
//!
//! These operations state constraints:
//!
//! - a borrow `&place` issues a loan and relates the type of the place to the
//!   pointee of the reference. When the place is behind references, it is a
//!   reborrow, and the references it goes through must outlive the loan.
//! - a load relates the type of the place to the type of the loaded value.
//! - a store relates the type of the stored value to the type of the place.
//! - a call relates the type of each argument to the type of its parameter, and
//!   the return type to the type of the call, with the callee's signature
//!   instantiated by the substitution of the call. It also requires the
//!   callee's where clause, instantiated the same way.
//! - a `perform` is a call of the signature of its operation.
//! - a call or a `perform` also relates the effect it introduces, the effect
//!   row of the callee instantiated the same way or the label of the performed
//!   effect, to the effect of the function it is in. Each label is related to
//!   the label of the function that handles it.
//! - a dictionary passed to an instance parameter, by a call, a `perform` or a
//!   struct initialization, must implement the trait reference the parameter
//!   declares. The trait reference it does implement is related to that one,
//!   which ties the regions of the dictionary to those of the type arguments
//!   beside it. The same is required of the dictionaries the dictionary was
//!   itself built from, along with the where clause of its instance.
//! - a struct initialization relates the type of each initializer to the type
//!   of its field, and requires the where clause of the struct, instantiated
//!   with the arguments of the struct type.
//! - a tuple relates the type of each element to its element of the tuple type.
//! - a closure relates the type of each capture operand to its element of the
//!   captured tuple of the closure type.
//! - a phi relates the type of each incoming value to the type of the phi, at
//!   the terminator of the block the value comes from.
//! - a `return` relates the type of the returned value to the return type of
//!   the function, at its terminator.
//! - a drop relates the type of the dropped value to the type its `Drop`
//!   dictionary implements the trait for, and requires what the dictionary was
//!   built from, as for a dictionary passed to a call.
//!
//! A where clause may also require a type parameter or a projection to
//! outlive a lifetime. That is recorded as a [`TypeTest`] rather than a
//! constraint, and checked by [`type_test`](crate::type_test).
//!
//! What a nested function requires of its creator is not collected yet: the
//! regions in the interface of a closure body, a handled body or an operation
//! handler are universal regions of that function, which nothing maps to the
//! regions of its creator so far. So a `handle` states no constraint.
//!
//! This is meant to run after [renumbering](crate::renumber), so that every
//! lifetime the constraints mention is a region or a universal lifetime.

use qbice::storage::intern::Interned;
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Local},
    cfg::{BlockID, Instruction, Point, Terminator},
    ir_expr::{IRExprID, IRExprKind},
    ir_function::IRFunction,
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

mod instance;
mod invocation;
mod place;
mod predicate;
mod value;

/// Identifies a loan issued in an IR function.
pub type LoanID = ID<Loan>;

/// A borrow issued in an IR function, as `&'r place` or `&'r mut place`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loan {
    /// The lifetime of the reference the borrow creates. The loan is live
    /// wherever it flows, through the constraint graph, into a live region.
    region: Interned<Ty>,

    /// The point of the borrow.
    point: Point,

    /// The borrowed place.
    address: Address,

    /// Whether the borrow is shared or mutable.
    mutability: Mutability,

    /// The source of the borrow expression.
    span: RelativeSpan,

    /// The number of projections of `address` up to the innermost struct
    /// that owns the borrowed place and has a declared `Drop` instance, if
    /// there is one. See [`Self::is_used_by_drop_of`].
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
    /// place.
    ///
    /// A drop uses the storage of the value it drops, so it conflicts with a
    /// loan of that storage or of a place around it. It does not go through
    /// the pointers the value holds, which drop nothing, unless a struct on
    /// the way from `dropped` to the pointer has a `Drop` instance declared
    /// for it: that implementation may use anything the struct can reach.
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
///
/// Whether it holds depends on the universal regions `'bound` must outlive,
/// so it is checked once every constraint is collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeTest {
    /// The type parameter or projection required to outlive `bound`.
    subject: Interned<Ty>,

    /// The lifetime `subject` must outlive.
    bound: Interned<Ty>,

    /// The point of the instruction requiring it.
    point: Point,
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
}

/// A region at a point: a node of the localized constraint graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LocalizedRegion {
    region: Interned<Ty>,
    point: Point,
}

/// The outlives constraints the instructions of an IR function require, each
/// at the point of its instruction, the type tests they require, and the
/// loans the function issues.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalizedConstraints {
    /// For each region at a point, the regions it must outlive there. These
    /// are the edges of the localized constraint graph that the instruction
    /// at the point states (rustc: `Locations::Single`). A region with no
    /// constraint at a point has no entry.
    edges: FxHashMap<LocalizedRegion, FxHashSet<Interned<Ty>>>,

    /// The type tests the instructions require, in the order they were
    /// collected.
    type_tests: Vec<TypeTest>,

    /// The loans issued by the borrows of the function.
    loans: Arena<Loan>,

    /// The loan issued by each borrow expression.
    loans_by_ref_of_id: FxHashMap<IRExprID, LoanID>,

    /// The loans of the places in each local, including the places reached
    /// through a dereference of it.
    loans_by_local: FxHashMap<Local, Vec<LoanID>>,
}

impl LocalizedConstraints {
    /// Collects the outlives constraints and the loans of every reachable
    /// instruction of `function`.
    ///
    /// `captures` is the capture layout of a nested function, and `None` for
    /// the definition function. `effect` is the effect row of the function,
    /// as [`IRFunctionMap::effect_of`] gives it. `solver` must be created at
    /// the definition the function belongs to.
    ///
    /// [`IRFunctionMap::effect_of`]: rayc_ir::ir_function::IRFunctionMap::effect_of
    pub async fn collect(
        function: &IRFunction,
        captures: Option<&CaptureMap>,
        effect: &Interned<Ty>,
        solver: &mut Solver,
    ) -> Self {
        let mut collector = ConstraintCollector {
            function,
            captures,
            effect,
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

    /// Iterates over the regions that `region` must outlive at `point`: the
    /// regions its loans flow into there.
    pub fn outlived_regions(
        &self,
        region: &Interned<Ty>,
        point: Point,
    ) -> impl Iterator<Item = &Interned<Ty>> {
        self.edges.get(&LocalizedRegion { region: region.clone(), point }).into_iter().flatten()
    }

    /// Iterates over every outlives constraint of the function, as the
    /// lesser region, the point requiring the constraint and the greater
    /// region of `'lesser: 'greater`, in unspecified order.
    pub fn outlives(&self) -> impl Iterator<Item = (&Interned<Ty>, Point, &Interned<Ty>)> {
        self.edges.iter().flat_map(|(lesser, greaters)| {
            greaters.iter().map(move |greater| (&lesser.region, lesser.point, greater))
        })
    }

    /// Iterates over the type tests of the function, in the order of the
    /// instructions requiring them.
    #[must_use]
    pub fn type_tests(&self) -> impl ExactSizeIterator<Item = &TypeTest> { self.type_tests.iter() }

    /// Iterates over the loans issued in the function, in unspecified order.
    ///
    /// A borrow of a place behind a shared reference or a raw pointer issues
    /// no loan. Nothing can be written, moved or mutably borrowed through a
    /// shared reference, so no access conflicts with such a borrow; the
    /// references it goes through are required to outlive it instead, which
    /// keeps the loans they came from live. The memory behind a raw pointer is
    /// not tracked at all.
    #[must_use]
    pub fn loans(&self) -> impl ExactSizeIterator<Item = (LoanID, &Loan)> { self.loans.iter() }

    /// Returns the loan identified by `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not identify a loan of this function.
    #[must_use]
    pub fn get_loan(&self, id: LoanID) -> &Loan { self.loans.get(id).expect("loan should exist") }

    /// Returns the loan issued by the borrow expression `expression_id`, or
    /// `None` when the expression is not a borrow, or issues no loan.
    #[must_use]
    pub fn loan_id_of_ref_of(&self, expression_id: IRExprID) -> Option<LoanID> {
        self.loans_by_ref_of_id.get(&expression_id).copied()
    }

    /// Returns the loan issued by the borrow expression `expression_id`, or
    /// `None` when the expression is not a borrow, or issues no loan.
    #[must_use]
    pub fn loan_of_ref_of(&self, expression_id: IRExprID) -> Option<&Loan> {
        let loan_id = self.loans_by_ref_of_id.get(&expression_id)?;
        self.loans.get(*loan_id)
    }

    /// Iterates over the loans of the places in `local`, including the places
    /// reached through a dereference of it, in unspecified order.
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

    /// Adds the edge `'lesser@point -> 'greater@point` for the predicate
    /// `'lesser: 'greater` between two lifetimes required at `point`.
    fn add(&mut self, point: Point, predicate: &OutlivesPredicate) {
        self.edges
            .entry(LocalizedRegion { region: predicate.lesser().clone(), point })
            .or_default()
            .insert(predicate.greater().clone());
    }

    /// Records the type test `subject: 'bound` required at `point`.
    fn add_type_test(&mut self, point: Point, subject: Interned<Ty>, bound: Interned<Ty>) {
        self.type_tests.push(TypeTest { subject, bound, point });
    }
}

/// Walks the instructions of an IR function for
/// [`LocalizedConstraints::collect`].
///
/// The rules are grouped by what they are about, each in its own submodule:
/// [`place`], [`value`], [`invocation`], [`instance`] and [`predicate`].
struct ConstraintCollector<'a> {
    function: &'a IRFunction,
    captures: Option<&'a CaptureMap>,

    /// The effect row of the function, which every effect introduced in it
    /// is a part of.
    effect: &'a Interned<Ty>,
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

            // Scopes require no constraint: a borrow still live when the
            // storage of its place ends is an invalidated loan, not an
            // outlives constraint.
            Instruction::ScopePush(_) | Instruction::ScopePop(_) => {}

            Instruction::ExprDiscard(discard) => {
                let value_ty = self.function.get_expression(discard.expression()).ty();
                self.collect_drop(point, value_ty, discard.drop_instance()).await;
            }
            Instruction::AddressDrop(drop) => {
                let Some(place_ty) = self.place_type(drop.address()).await else {
                    return;
                };

                self.collect_drop(point, &place_ty, drop.drop_instance()).await;
            }
        }
    }

    /// Collects the constraints of the terminator of `block_id`, at the
    /// point that stands for it.
    async fn collect_terminator(&mut self, block_id: BlockID) {
        match self.function.block_terminator(block_id) {
            Some(Terminator::Return(Some(value))) => {
                let point = self.function.terminator_point(block_id);
                self.collect_return(point, *value).await;
            }

            // A bare return gives back unit, which holds no region, and a
            // jump only reads a condition, if anything. The values a jump
            // passes to the phis of its target are related by those phis.
            Some(Terminator::Return(None) | Terminator::Jump(_) | Terminator::Conditional(_))
            | None => {}
        }
    }

    /// Collects the constraints of evaluating the expression `expression_id`
    /// at `point`.
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

            // The memory behind a raw pointer is not tracked, so the pointer
            // carries no region to relate with the reference it came from.
            IRExprKind::RefToPointer(_) => {}

            // The operands and the result are primitives, which hold no
            // region.
            IRExprKind::Binary(_) => {}

            // TODO: the capture operands are passed to the handled body and
            // to the operation handlers, whose capture layouts and return
            // type are made of their own universal regions. Relating them
            // needs those regions mapped to the regions of this function,
            // along with the outlives requirements of the nested bodies.
            IRExprKind::Handle(_) => {}
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
        // Type checking already made the two types equal modulo lifetimes,
        // so the relation only fails when one side is an error, which was
        // reported already.
        let relation = TyRelate::new(lesser.clone(), greater.clone(), variance);
        let Some(outlives) = self.solver.solve_without_unify(vec![relation]).await else {
            return;
        };

        for constraint in outlives.iter() {
            self.constraints.add(point, constraint);
        }
    }
}
