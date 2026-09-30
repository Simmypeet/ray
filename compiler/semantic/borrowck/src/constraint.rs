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
//! It also records the loans the function issues: one per borrow, whose
//! region is the lifetime of the reference the borrow creates.
//!
//! Only the primitive operations are handled so far:
//!
//! - a borrow `&place` issues a loan and relates the type of the place to the
//!   pointee of the reference. When the place is behind references, it is a
//!   reborrow, and the references it goes through must outlive the loan.
//! - a load relates the type of the place to the type of the loaded value.
//! - a store relates the type of the stored value to the type of the place.
//!
//! This is meant to run after [renumbering](crate::renumber), so that every
//! lifetime the constraints mention is a region or a universal lifetime.

use qbice::storage::intern::Interned;
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Local, Projection},
    cfg::{Instruction, Point, Store},
    ir_expr::{IRExprID, IRExprKind, load::Load, ref_of::RefOf},
    ir_function::IRFunction,
    ir_lambda::CaptureMap,
};
use rayc_semantic_element::{parameter::get_parameter_map, struct_body::get_struct_body};
use rayc_solver::Solver;
use rayc_type::{
    constraint::{outlives::OutlivesConstraint, ty_relate::TyRelate},
    subst::Substitutable,
    ty::{Mutability, Ty},
    variance::Variance,
};

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
}

/// A region at a point: a node of the localized constraint graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LocalizedRegion {
    region: Interned<Ty>,
    point: Point,
}

/// The outlives constraints the instructions of an IR function require, each
/// at the point of its instruction, and the loans the function issues.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalizedConstraints {
    /// For each region at a point, the regions it must outlive there. These
    /// are the edges of the localized constraint graph that the instruction
    /// at the point states (rustc: `Locations::Single`). A region with no
    /// constraint at a point has no entry.
    edges: FxHashMap<LocalizedRegion, FxHashSet<Interned<Ty>>>,

    /// The loans issued by the borrows of the function.
    loans: Arena<Loan>,
}

impl LocalizedConstraints {
    /// Collects the outlives constraints and the loans of every reachable
    /// instruction of `function`.
    ///
    /// `captures` is the capture layout of a nested function, and `None` for
    /// the definition function. `solver` must be created at the definition
    /// the function belongs to.
    pub async fn collect(
        function: &IRFunction,
        captures: Option<&CaptureMap>,
        solver: &mut Solver,
    ) -> Self {
        let mut collector =
            ConstraintCollector { function, captures, solver, constraints: Self::default() };

        // Terminators are not visited yet: relating a returned value to the
        // return type is left for later.
        let reachables = function.reachables();
        for block_id in reachables.blocks() {
            for (point, instruction) in function.block_instructions_with_points(block_id) {
                collector.collect_instruction(point, instruction).await;
            }
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

    /// Iterates over the loans issued in the function, in unspecified order.
    #[must_use]
    pub fn loans(&self) -> impl ExactSizeIterator<Item = (LoanID, &Loan)> { self.loans.iter() }

    /// Returns the loan identified by `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not identify a loan of this function.
    #[must_use]
    pub fn get_loan(&self, id: LoanID) -> &Loan { self.loans.get(id).expect("loan should exist") }

    /// Adds the edge `'lesser@point -> 'greater@point` for the constraint
    /// `'lesser: 'greater` required at `point`.
    fn add(&mut self, point: Point, constraint: &OutlivesConstraint) {
        self.edges
            .entry(LocalizedRegion { region: constraint.lesser().clone(), point })
            .or_default()
            .insert(constraint.greater().clone());
    }
}

/// Walks the instructions of an IR function for
/// [`LocalizedConstraints::collect`].
struct ConstraintCollector<'a> {
    function: &'a IRFunction,
    captures: Option<&'a CaptureMap>,
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

            // TODO: a drop requires the regions its `Drop` implementation
            // may use to be live, which is not handled yet.
            Instruction::ExprDiscard(_) | Instruction::AddressDrop(_) => {}
        }
    }

    /// Collects the constraints of evaluating the expression `expression_id`
    /// at `point`.
    #[allow(clippy::match_same_arms)]
    async fn collect_expression(&mut self, point: Point, expression_id: IRExprID) {
        let expression = self.function.get_expression(expression_id);

        match expression.kind() {
            IRExprKind::RefOf(ref_of) => self.collect_borrow(point, ref_of, expression.ty()).await,
            IRExprKind::Load(load) => self.collect_load(point, load, expression.ty()).await,

            // Neither reads a place nor relates two values.
            IRExprKind::Error | IRExprKind::Literal(_) => {}

            // TODO: the remaining expressions move their operands into a new
            // value, or pass them to a function, which is not handled yet.
            IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Call(_)
            | IRExprKind::Perform(_)
            | IRExprKind::Handle(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::Closure(_)
            | IRExprKind::StructInitialization(_) => {}
        }
    }

    /// Collects the constraints of the borrow `ref_of`, whose reference has
    /// type `ty`, and issues its loan.
    async fn collect_borrow(&mut self, point: Point, ref_of: &RefOf, ty: &Interned<Ty>) {
        let mut dereferenced = Vec::new();
        let Some(place_ty) =
            self.place_type(ref_of.address(), |pointer| dereferenced.push(pointer.clone())).await
        else {
            return;
        };

        // `&'r place` has type `&'r typeof(place)`, which must be a subtype
        // of `ty`. The lifetime of `ty` is the loan's own region, so only the
        // pointees are related, with the variance of the reference's pointee.
        let reference = ty.as_reference_view().expect("a borrow should have a reference type");
        let variance = reference.mutability().pointee_variance();
        self.relate(point, &place_ty, reference.pointee(), variance).await;

        self.collect_reborrow(point, reference.lifetime(), &dereferenced);

        self.constraints.loans.insert(Loan {
            region: reference.lifetime().clone(),
            point,
            address: ref_of.address().clone(),
            mutability: reference.mutability(),
        });
    }

    /// Requires the references that a borrowed place is reached through to
    /// outlive the loan `region`.
    ///
    /// `dereferenced` holds the type of every pointer dereferenced on the way
    /// to the place, outermost first. Borrowing `**p`, where
    /// `p: &'a mut &'b mut T`, borrows data that is only reachable for `'a`
    /// and `'b`, so both must outlive the new loan.
    ///
    /// The references are visited innermost first, stopping after the first
    /// shared one: the data behind a shared reference stays borrowed for its
    /// lifetime however that reference was reached, since it can be copied
    /// out of the references holding it. A raw pointer stops the walk too,
    /// since the memory behind it is not tracked.
    fn collect_reborrow(
        &mut self,
        point: Point,
        region: &Interned<Ty>,
        dereferenced: &[Interned<Ty>],
    ) {
        for pointer in dereferenced.iter().rev() {
            let Some(reference) = pointer.as_reference_view() else {
                break;
            };

            for constraint in
                OutlivesConstraint::from_relation(reference.lifetime(), region, Variance::Covariant)
            {
                self.constraints.add(point, &constraint);
            }

            match reference.mutability() {
                Mutability::Immutable => break,
                Mutability::Mutable => {}
            }
        }
    }

    /// Collects the constraints of `load`, whose loaded value has type `ty`:
    /// `typeof(place) <: ty`.
    async fn collect_load(&mut self, point: Point, load: &Load, ty: &Interned<Ty>) {
        let Some(place_ty) = self.place_type(load.address(), |_| {}).await else {
            return;
        };

        self.relate(point, &place_ty, ty, Variance::Covariant).await;
    }

    /// Collects the constraints of `store`: `typeof(value) <: typeof(place)`.
    async fn collect_store(&mut self, point: Point, store: &Store) {
        let Some(place_ty) = self.place_type(store.address(), |_| {}).await else {
            return;
        };

        let value_ty = self.function.get_expression(store.expression()).ty();
        self.relate(point, value_ty, &place_ty, Variance::Covariant).await;
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

    /// Returns the type of the place `address` selects, or `None` for an
    /// error address.
    ///
    /// `on_deref` is called with the type of every pointer the address
    /// dereferences, outermost first.
    async fn place_type(
        &self,
        address: &Address,
        mut on_deref: impl FnMut(&Interned<Ty>),
    ) -> Option<Interned<Ty>> {
        let mut ty = self.binding_type(address.local()?).await;

        // Normalize each prefix before inspecting its shape, since a field
        // or pointee may be an associated type.
        for &projection in address.projections() {
            let base = self.solver.normalize(&ty).await;
            if projection.is_deref() {
                on_deref(&base);
            }
            ty = self.projected_type(&base, projection).await;
        }

        Some(ty)
    }

    /// Returns the type of the component of the normalized type `base` that
    /// `projection` selects.
    ///
    /// # Panics
    ///
    /// Panics if `projection` does not match the shape of `base`.
    async fn projected_type(&self, base: &Interned<Ty>, projection: Projection) -> Interned<Ty> {
        match projection {
            Projection::Deref | Projection::RawDeref => base
                .as_dereferenceable()
                .unwrap_or_else(|| panic!("dereferenced type should be a pointer: {base:?}"))
                .pointee()
                .clone(),

            Projection::Tuple(index) => base
                .as_tuple_view()
                .and_then(|tuple| tuple.args().get(index))
                .unwrap_or_else(|| panic!("tuple projection {index} does not match {base:?}"))
                .clone(),

            Projection::Field(field_id) => {
                let struct_ty = base
                    .as_struct_view()
                    .unwrap_or_else(|| panic!("field projection does not match {base:?}"));
                let engine = self.solver.engine();
                let substitution = struct_ty.create_subst(engine).await;
                let body = engine.get_struct_body(struct_ty.symbol_id()).await;
                body[field_id].ty().apply_subst_or_clone(&substitution, engine)
            }
        }
    }

    /// Returns the type of the binding stored in `local`.
    async fn binding_type(&self, local: Local) -> Interned<Ty> {
        match local {
            Local::Variable(variable_id) => self.function.get_variable(variable_id).ty().clone(),
            Local::Parameter(parameter_id) => {
                self.solver.engine().get_parameter_map(self.solver.site()).await[parameter_id]
                    .ty()
                    .clone()
            }
            Local::LambdaParameter(parameter_id) => self
                .function
                .context()
                .assert_as_lambda_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            Local::OperationHandlerParameter(parameter_id) => self
                .function
                .context()
                .assert_as_operation_handler_context()
                .get_parameter(parameter_id)
                .ty()
                .clone(),
            Local::Capture(capture_id) => self
                .captures
                .expect("capture address roots should have a capture layout")
                .get_capture(capture_id)
                .storage_ty(self.solver.engine()),
        }
    }
}
