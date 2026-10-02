//! The constraints of invoking a signature: calls and `perform`s.

use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::Point,
    ir_expr::{IRExprID, call::Call, perform::Perform},
};
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
    variance::Variance,
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Collects the constraints of `call`, whose result has type `ty`, as an
    /// invocation of the callee's signature, instantiated with the
    /// substitution of the call.
    pub(super) async fn collect_call(&mut self, point: Point, call: &Call, ty: &Interned<Ty>) {
        let signature_id = call.target().signature_id();
        let substitution = call.target().signature_subst(self.solver.engine()).await;

        self.collect_invocation(point, signature_id, &substitution, call.arguments(), ty).await;

        // The signature was instantiated from the trait reference of the
        // dictionary the call dispatches through, so that dictionary matches
        // it already; only what it was built from is left to require.
        if let Some(instance) = call.target().dispatch_instance() {
            self.collect_dictionary_requirements(point, instance).await;
        }
    }

    /// Collects the constraints of `perform`, whose result has type `ty`, as
    /// an invocation of the signature of its operation, instantiated with
    /// the substitution of the `perform`.
    ///
    /// Whichever handler runs the operation is checked against the same
    /// signature, so the operation stands for it here.
    pub(super) async fn collect_perform(
        &mut self,
        point: Point,
        perform: &Perform,
        ty: &Interned<Ty>,
    ) {
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
    pub(super) async fn collect_invocation(
        &mut self,
        point: Point,
        signature_id: GlobalSymbolID,
        substitution: &Subst,
        arguments: &[IRExprID],
        ty: &Interned<Ty>,
    ) {
        // Each argument is passed to its parameter. A variadic call has more
        // arguments than parameters; the extra ones have no type to relate to.
        let parameters = self.solver.engine().get_parameter_map(signature_id).await;
        for ((_, parameter), argument) in parameters.iter().zip(arguments) {
            let parameter_ty =
                parameter.ty().apply_subst_or_clone(substitution, self.solver.engine());
            let argument_ty = self.function.get_expression(*argument).ty();

            self.relate(point, argument_ty, &parameter_ty, Variance::Covariant).await;
        }

        // The returned value becomes the value of the call.
        let return_ty = self
            .solver
            .engine()
            .get_return_type(signature_id)
            .await
            .apply_subst_or_clone(substitution, self.solver.engine());
        self.relate(point, &return_ty, ty, Variance::Covariant).await;

        // The callee assumes its where clause and the trait references of
        // its instance parameters, so the call must prove them.
        self.collect_where_clause(point, signature_id, substitution).await;
        self.collect_instance_arguments(point, signature_id, substitution).await;
    }
}
