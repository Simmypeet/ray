//! The constraints of building a value from other values, and of passing a
//! value along the control flow: tuples, closures, struct initializations,
//! phis and returns.

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Projection,
    cfg::Point,
    ir_expr::{
        IRExprID, closure::Closure, phi::Phi, struct_initialization::StructInitialization,
        tuple::Tuple,
    },
    ir_function::FunctionID,
};
use rayc_type::{
    subst::Subst,
    ty::{Ty, application::ClosureView},
    variance::Variance,
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Collects the constraints of `tuple`, whose value has type `ty`:
    /// `typeof(element) <: element type` for each element.
    pub(super) async fn collect_tuple(&mut self, point: Point, tuple: &Tuple, ty: &Interned<Ty>) {
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
    ///
    /// It also requires what the body of the closure requires of the
    /// lifetimes in its interface, with those lifetimes instantiated by the
    /// regions of `ty`; see [`Self::closure_instantiation`].
    pub(super) async fn collect_closure(
        &mut self,
        point: Point,
        closure: &Closure,
        ty: &Interned<Ty>,
    ) {
        let closure_ty =
            ty.as_closure_view().expect("a `Closure` expression always has a closure type");
        let captured = closure_ty
            .captured_tuple()
            .as_tuple_view()
            .expect("the captures of a closure type should be a tuple");

        // Each capture operand is stored in the closure value.
        for (operand, capture_ty) in closure.captures().iter().zip(captured.args()) {
            let operand_ty = self.function.get_expression(*operand).ty();
            self.relate(point, operand_ty, capture_ty, Variance::Covariant).await;
        }

        // The body was checked with the lifetimes of its interface as
        // universal regions, assuming what it requires of them. The closure
        // value may only exist when that holds of the regions it is created
        // with, as a callee may only be called when its where clause holds.
        let nested = self.nested;
        let mut requirements = nested.of(closure.function_id()).peekable();

        // early return if no requirements, to avoid computing instantiation
        if requirements.peek().is_none() {
            return;
        }

        let instantiation = self.closure_instantiation(closure.function_id(), &closure_ty).await;
        for requirement in requirements {
            let predicate = requirement.instantiate(&instantiation, self.solver.engine());

            // The body requires the predicate, not the closure expression.
            self.collect_outlives_blaming(point, &predicate, Some(requirement.span())).await;
        }
    }

    /// Returns the substitution of each external region of the closure body
    /// `function_id` with the region of `closure_ty` it stands for, where
    /// `closure_ty` is the type of a closure value created from the body.
    ///
    /// The interface of the body, the storage of its captures, its
    /// parameters, its return type and its effect, is what the closure type
    /// was built from. Renumbering gave each lifetime of the interface an
    /// external region, and each lifetime of `closure_ty` a region of the
    /// function creating the closure, so the two are equal up to lifetimes,
    /// and the lifetimes at the same position correspond.
    ///
    /// # Panics
    ///
    /// Panics if the interface of the body does not have the shape of
    /// `closure_ty`.
    async fn closure_instantiation(
        &self,
        function_id: FunctionID,
        closure_ty: &ClosureView<'_>,
    ) -> Subst {
        let engine = self.solver.engine();
        let context = self.ir.get_function(function_id).context().assert_as_lambda_context();
        let captures = self
            .ir
            .captures_for_function(function_id)
            .expect("a closure body should have a capture layout");
        let capture_slots = closure_ty
            .captured_tuple()
            .as_tuple_view()
            .expect("the captures of a closure type should be a tuple");

        // Pair each type of the interface of the body with the argument of
        // the closure type that was built from it.
        let capture_types = captures.iter().map(|(_, capture)| capture.storage_ty(engine));
        let parameter_types = context.parameters().map(|(_, parameter)| parameter.ty().clone());
        let signature = [
            (context.return_ty().clone(), closure_ty.return_type()),
            (context.effect().clone(), closure_ty.effect_row()),
        ];
        let interface = capture_types
            .zip(capture_slots.args())
            .chain(parameter_types.zip(closure_ty.params()))
            .chain(signature);

        // An external region stands for the lifetime at its position in the
        // closure type. A lifetime that is not external, `'static` or a
        // lifetime parameter, is the same in both functions already.
        let mut instantiation = Subst::new_empty();
        for (external_ty, instantiated_ty) in interface {
            let lifetimes = Ty::corresponding_lifetimes(&external_ty, instantiated_ty, engine)
                .await
                .expect("the interface of a closure body should have the shape of its type");

            for (lifetime, instantiated) in lifetimes {
                if let Some(external) = lifetime.as_external_region() {
                    instantiation.insert(external, instantiated);
                }
            }
        }

        instantiation
    }

    /// Collects the constraints of `phi`, whose value has type `ty`:
    /// `typeof(incoming value) <: ty` for each incoming value.
    ///
    /// Each is required at the terminator of the block the value comes from,
    /// since that is where the value is last live: the phi consumes it on the
    /// edge into its block.
    pub(super) async fn collect_phi(&mut self, phi: &Phi, ty: &Interned<Ty>) {
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
    pub(super) async fn collect_return(&mut self, point: Point, value: IRExprID) {
        let return_ty = self.function.return_ty(self.solver.site(), self.solver.engine()).await;

        let value_ty = self.function.get_expression(value).ty();
        self.relate(point, value_ty, &return_ty, Variance::Covariant).await;
    }

    /// Collects the constraints of `initialization`, whose struct value has
    /// type `ty`: `typeof(initializer) <: field type` for each field, with
    /// the field types instantiated by the arguments of `ty`.
    pub(super) async fn collect_struct_initialization(
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
        self.collect_instance_arguments(point, initialization.struct_id(), &substitution).await;
    }
}
