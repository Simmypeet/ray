//! The constraints of building a value from other values, and of passing a
//! value along the control flow: tuples, struct initializations, variants,
//! phis and returns.

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Projection,
    cfg::Point,
    ir_expr::{
        IRExprID, phi::Phi, struct_initialization::StructInitialization, tuple::Tuple,
        variant::Variant,
    },
};
use rayc_semantic_element::variant_payload_ty::get_variant_payload_ty;
use rayc_type::{subst::Substitutable, ty::Ty, variance::Variance};

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

    /// Collects the constraints of `phi`, whose value has type `ty`:
    /// `typeof(incoming value) <: ty`, at the terminator of the block each
    /// value comes from.
    pub(super) async fn collect_phi(&mut self, phi: &Phi, ty: &Interned<Ty>) {
        for (predecessor, value) in phi.incoming() {
            let point = self.function.terminator_point(predecessor);
            let value_ty = self.function.get_expression(value).ty();
            self.relate(point, value_ty, ty, Variance::Covariant).await;
        }
    }

    /// Collects the constraints of returning `value` at `point`: `typeof(value)
    /// <: return type`.
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

    /// Collects the constraints of `variant`, whose enum value has type `ty`:
    /// `typeof(payload) <: payload type`, with the payload type instantiated
    /// by the arguments of `ty`.
    pub(super) async fn collect_variant(
        &mut self,
        point: Point,
        variant: &Variant,
        ty: &Interned<Ty>,
    ) {
        let enum_ty = ty.as_enum_view().expect("a variant has an enum type");
        let substitution = enum_ty.create_subst(self.solver.engine()).await;

        if let Some(payload) = variant.payload() {
            let payload_ty = self
                .solver
                .engine()
                .get_variant_payload_ty(variant.variant_id())
                .await
                .expect("a variant constructed with a payload declares one")
                .apply_subst_or_clone(&substitution, self.solver.engine())
                .await;
            let value_ty = self.function.get_expression(payload).ty();
            self.relate(point, value_ty, &payload_ty, Variance::Covariant).await;
        }

        // A value of the enum type only exists when the where clause of the
        // enum holds, its inferred outlives predicates included.
        self.collect_where_clause(point, enum_ty.symbol_id(), &substitution).await;
        self.collect_instance_arguments(point, enum_ty.symbol_id(), &substitution).await;
    }
}
