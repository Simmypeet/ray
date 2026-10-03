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
};
use rayc_type::{ty::Ty, variance::Variance};

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
