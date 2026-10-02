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
//!   dictionary implements the trait for.
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
    address::{Address, Local, Projection},
    cfg::{BlockID, Instruction, Point, Store, Terminator},
    ir_expr::{
        IRExprID, IRExprKind, call::Call, closure::Closure, load::Load, perform::Perform, phi::Phi,
        ref_of::RefOf, struct_initialization::StructInitialization, tuple::Tuple,
    },
    ir_function::IRFunction,
    ir_lambda::CaptureMap,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::{
    parameter::get_parameter_map, return_type::get_return_type, struct_body::get_struct_body,
};
use rayc_solver::{Solver, givens::get_givens};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::{outlives::OutlivesConstraint, ty_relate::TyRelate},
    outlives::OutlivesComponent,
    subst::{Subst, Substitutable},
    ty::{Mutability, Ty},
    variance::Variance,
    where_clause::{OutlivesPredicate, PredicateKind},
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

    /// The source of the borrow expression.
    span: RelativeSpan,
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
    /// the definition function. `solver` must be created at the definition
    /// the function belongs to.
    pub async fn collect(
        function: &IRFunction,
        captures: Option<&CaptureMap>,
        solver: &mut Solver,
    ) -> Self {
        let mut collector =
            ConstraintCollector { function, captures, solver, constraints: Self::default() };

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

    /// Returns the loan issued by the borrow expression `expression_id`, or
    /// `None` when the expression is not a borrow, or borrows an error
    /// address.
    #[must_use]
    pub fn loan_id_of_ref_of(&self, expression_id: IRExprID) -> Option<LoanID> {
        self.loans_by_ref_of_id.get(&expression_id).copied()
    }

    /// Returns the loan issued by the borrow expression `expression_id`, or
    /// `None` when the expression is not a borrow, or borrows an error
    /// address.
    #[must_use]
    pub fn load_of_ref_of(&self, expression_id: IRExprID) -> Option<&Loan> {
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

    /// Collects the constraints of the borrow `ref_of`, the expression
    /// `expression_id` of type `ty`, and issues its loan.
    async fn collect_borrow(
        &mut self,
        point: Point,
        expression_id: IRExprID,
        ref_of: &RefOf,
        ty: &Interned<Ty>,
    ) {
        let reference =
            ty.as_reference_view().expect("a `RefOf` expression always has a reference type");

        let mut dereferenced = Vec::new();
        let Some(place_ty) = self
            .place_type_with_derefs(ref_of.address(), |pointer| dereferenced.push(pointer.clone()))
            .await
        else {
            return;
        };

        // `&'r place` has type `&'r typeof(place)`, which must be a subtype
        // of `ty`. The lifetime of `ty` is the loan's own region, so only the
        // pointees are related, with the variance of the reference's pointee.
        let variance = reference.mutability().pointee_variance();
        self.relate(point, &place_ty, reference.pointee(), variance).await;

        self.collect_reborrow(point, reference.lifetime(), &dereferenced);

        self.constraints.issue_loan(expression_id, Loan {
            region: reference.lifetime().clone(),
            point,
            address: ref_of.address().clone(),
            mutability: reference.mutability(),
            span: self.function.get_expression(expression_id).span(),
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
        let Some(place_ty) = self.place_type(load.address()).await else {
            return;
        };

        self.relate(point, &place_ty, ty, Variance::Covariant).await;
    }

    /// Collects the constraints of `store`: `typeof(value) <: typeof(place)`.
    async fn collect_store(&mut self, point: Point, store: &Store) {
        let Some(place_ty) = self.place_type(store.address()).await else {
            return;
        };

        let value_ty = self.function.get_expression(store.expression()).ty();
        self.relate(point, value_ty, &place_ty, Variance::Covariant).await;
    }

    /// Collects the constraints of `call`, whose result has type `ty`, as an
    /// invocation of the callee's signature, instantiated with the
    /// substitution of the call.
    async fn collect_call(&mut self, point: Point, call: &Call, ty: &Interned<Ty>) {
        let signature_id = call.target().signature_id();
        let substitution = call.target().signature_subst(self.solver.engine()).await;

        self.collect_invocation(point, signature_id, &substitution, call.arguments(), ty).await;
    }

    /// Collects the constraints of `perform`, whose result has type `ty`, as
    /// an invocation of the signature of its operation, instantiated with
    /// the substitution of the `perform`.
    ///
    /// Whichever handler runs the operation is checked against the same
    /// signature, so the operation stands for it here.
    async fn collect_perform(&mut self, point: Point, perform: &Perform, ty: &Interned<Ty>) {
        self.collect_invocation(
            point,
            perform.operation_id(),
            perform.substitution(),
            perform.arguments(),
            ty,
        )
        .await;
    }

    /// Collects the constraints of invoking the signature of `signature_id`,
    /// instantiated with `substitution`, with `arguments`, for a result of
    /// type `ty`: `typeof(argument) <: parameter type` for each argument, and
    /// `return type <: ty`.
    ///
    /// Renumbering gave the lifetimes of the substitution their own regions,
    /// which the parameters and the return type share: a loan passed in one
    /// argument flows through them into the other arguments and the result
    /// that mention the same type parameter.
    async fn collect_invocation(
        &mut self,
        point: Point,
        signature_id: GlobalSymbolID,
        substitution: &Subst,
        arguments: &[IRExprID],
        ty: &Interned<Ty>,
    ) {
        let engine = self.solver.engine().clone();

        // Each argument is passed to its parameter. A variadic call has more
        // arguments than parameters; the extra ones have no type to relate to.
        let parameters = engine.get_parameter_map(signature_id).await;
        for ((_, parameter), argument) in parameters.iter().zip(arguments) {
            let parameter_ty = parameter.ty().apply_subst_or_clone(substitution, &engine);
            let argument_ty = self.function.get_expression(*argument).ty();
            self.relate(point, argument_ty, &parameter_ty, Variance::Covariant).await;
        }

        // The returned value becomes the value of the call.
        let return_ty =
            engine.get_return_type(signature_id).await.apply_subst_or_clone(substitution, &engine);
        self.relate(point, &return_ty, ty, Variance::Covariant).await;

        // The callee assumes its where clause, so the call must prove it.
        self.collect_where_clause(point, signature_id, substitution).await;
    }

    /// Collects the constraints of `tuple`, whose value has type `ty`:
    /// `typeof(element) <: element type` for each element.
    async fn collect_tuple(&mut self, point: Point, tuple: &Tuple, ty: &Interned<Ty>) {
        let tuple_ty = ty.as_tuple_view().expect("a `Tuple` expression always has a tuple type");

        for (element, element_ty) in tuple.elements().iter().zip(tuple_ty.args()) {
            let value_ty = self.function.get_expression(*element).ty();
            self.relate(point, value_ty, element_ty, Variance::Covariant).await;
        }
    }

    /// Collects the constraints of `closure`, whose value has type `ty`:
    /// `typeof(capture operand) <: capture type` for each capture, where the
    /// capture types are the elements of the captured tuple of `ty`.
    ///
    /// The closure value then holds the loans of its captures, in the
    /// regions of its type, for as long as it is live.
    async fn collect_closure(&mut self, point: Point, closure: &Closure, ty: &Interned<Ty>) {
        let closure_ty =
            ty.as_closure_view().expect("a `Closure` expression always has a closure type");
        let captured = closure_ty
            .captured_tuple()
            .as_tuple_view()
            .expect("the captures of a closure type should be a tuple");

        for (operand, capture_ty) in closure.captures().iter().zip(captured.args()) {
            let operand_ty = self.function.get_expression(*operand).ty();
            self.relate(point, operand_ty, capture_ty, Variance::Covariant).await;
        }

        // TODO: the body of the closure may require its captures and its
        // signature to outlive one another, which is not required of the
        // closure type here yet.
    }

    /// Collects the constraints of `phi`, whose value has type `ty`:
    /// `typeof(incoming value) <: ty` for each incoming value.
    ///
    /// Each is required at the terminator of the block the value comes from,
    /// since that is where the value is last live: the phi consumes it on the
    /// edge into its block.
    async fn collect_phi(&mut self, phi: &Phi, ty: &Interned<Ty>) {
        for (predecessor, value) in phi.incoming() {
            let point = self.function.terminator_point(predecessor);
            let value_ty = self.function.get_expression(value).ty();
            self.relate(point, value_ty, ty, Variance::Covariant).await;
        }
    }

    /// Collects the constraints of returning `value` at `point`:
    /// `typeof(value) <: return type`.
    ///
    /// The lifetimes of the return type are universal, so a loan that flows
    /// into them escapes the function.
    async fn collect_return(&mut self, point: Point, value: IRExprID) {
        // A nested function stores its return type; the definition function
        // takes the one its definition declares.
        let return_ty = match self.function.context().nested_return_ty() {
            Some(return_ty) => return_ty.clone(),
            None => self.solver.engine().get_return_type(self.solver.site()).await,
        };

        let value_ty = self.function.get_expression(value).ty();
        self.relate(point, value_ty, &return_ty, Variance::Covariant).await;
    }

    /// Collects the constraints of dropping a value of type `value_ty` with
    /// the `Drop` dictionary `drop_instance`: `value_ty <: implementor`,
    /// where the dictionary implements `Drop[implementor]`.
    ///
    /// The value is moved into `Drop.drop`, so the loans it holds flow into
    /// the regions of the dictionary.
    async fn collect_drop(
        &mut self,
        point: Point,
        value_ty: &Interned<Ty>,
        drop_instance: &Interned<Ty>,
    ) {
        // A no-op dictionary reads nothing of the value.
        if drop_instance.is_no_op_drop_instance() {
            return;
        }

        // A dictionary without a trait reference is recovery from an invalid
        // declaration, which was reported already.
        let Ok(trait_ref) = drop_instance.instance_trait_ref(self.solver.engine()).await else {
            return;
        };
        let Some(implementor) = trait_ref.args().interned_iter().next() else {
            return;
        };

        self.relate(point, value_ty, implementor, Variance::Covariant).await;

        // TODO: the dictionary assumes the where clause of its instance,
        // which is not required here yet. The same holds for the
        // dictionaries in the substitution of a call.
    }

    /// Collects the constraints of `initialization`, whose struct value has
    /// type `ty`: `typeof(initializer) <: field type` for each field, with
    /// the field types instantiated by the arguments of `ty`.
    async fn collect_struct_initialization(
        &mut self,
        point: Point,
        initialization: &StructInitialization,
        ty: &Interned<Ty>,
    ) {
        for (&field_id, &initializer) in initialization.initializers() {
            let field_ty = self.projected_type(ty, Projection::Field(field_id)).await;
            let initializer_ty = self.function.get_expression(initializer).ty();
            self.relate(point, initializer_ty, &field_ty, Variance::Covariant).await;
        }

        // A value of the struct type only exists when the where clause of
        // the struct holds, its inferred outlives predicates included.
        let struct_ty = ty.as_struct_view().expect("a struct initialization has a struct type");
        let substitution = struct_ty.create_subst(self.solver.engine()).await;
        self.collect_where_clause(point, initialization.struct_id(), &substitution).await;
    }

    /// Requires, at `point`, the predicates that `symbol_id` assumes,
    /// instantiated with `substitution`: its where clause and implied bounds,
    /// and those of the declarations enclosing it.
    async fn collect_where_clause(
        &mut self,
        point: Point,
        symbol_id: GlobalSymbolID,
        substitution: &Subst,
    ) {
        let engine = self.solver.engine().clone();
        let predicates = engine.get_givens(symbol_id).await;

        for predicate in predicates.iter() {
            let predicate = predicate.apply_subst_or_clone(substitution, &engine);
            self.collect_predicate(point, &predicate).await;
        }
    }

    /// Adds, at `point`, the outlives constraints that proving the
    /// instantiated where-clause `predicate` requires.
    async fn collect_predicate(&mut self, point: Point, predicate: &PredicateKind) {
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

    /// Adds, at `point`, the outlives constraints of the instantiated
    /// predicate `subject: bound`: each lifetime in `subject` must outlive
    /// `bound`.
    async fn collect_outlives(&mut self, point: Point, predicate: &OutlivesPredicate) {
        let subject = self.solver.normalize(predicate.subject()).await;

        for component in Ty::outlives_components(&subject, self.solver.engine()).await {
            match component {
                OutlivesComponent::Region(region) => {
                    for constraint in OutlivesConstraint::from_relation(
                        &region,
                        predicate.bound(),
                        Variance::Covariant,
                    ) {
                        self.constraints.add(point, &constraint);
                    }
                }

                // TODO: a type parameter or a projection that must outlive
                // `bound` is a type test, to check against the outlives
                // environment once the constraint graph is complete.
                OutlivesComponent::Param(_) | OutlivesComponent::Projection(_) => {}
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
    async fn place_type(&self, address: &Address) -> Option<Interned<Ty>> {
        self.place_type_with_derefs(address, |_| {}).await
    }

    /// Returns the type of the place `address` selects, or `None` for an
    /// error address.
    ///
    /// `on_deref` is called with the type of every pointer the address
    /// dereferences, outermost first.
    async fn place_type_with_derefs(
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
