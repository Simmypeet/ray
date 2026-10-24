//! The constraints of invoking a signature: calls and `perform`s, and the
//! coroutines that suspend a call.

use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::Point,
    ir_expr::{IRExprID, call::Call, coroutine::Coroutine, perform::Perform},
};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    signature::{get_effect_row, get_return_type},
    subst::{Subst, Substitutable},
    ty::{Ty, application::CoroutineView},
    variance::Variance,
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Collects the constraints of `call`, whose result has type `ty`, as an
    /// invocation of the callee's signature.
    pub(super) async fn collect_call(&mut self, point: Point, call: &Call, ty: &Interned<Ty>) {
        let signature_id = call.target().signature_id();
        let substitution = call.target().signature_subst(self.solver.engine()).await;

        self.collect_invocation(point, signature_id, &substitution, call.arguments(), ty).await;

        // The callee performs the effect row it declares.
        let effect = self
            .solver
            .engine()
            .get_effect_row(signature_id)
            .await
            .apply_subst_or_clone(&substitution, self.solver.engine());
        self.collect_introduced_effect(point, &effect).await;

        // The dispatched dictionary already matches the signature instantiated
        // from it; only what it was built from is left to require.
        if let Some(instance) = call.target().dispatch_instance() {
            self.collect_dictionary_requirements(point, instance).await;
        }
    }

    /// Collects the constraints of `perform`, whose result has type `ty`, as an
    /// invocation of the signature of its operation.
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

        // The operation is performed in its effect, which no signature
        // declares.
        let effect = perform.effect_row(self.solver.engine()).await;
        self.collect_introduced_effect(point, &effect).await;
    }

    /// Collects the constraints of `coroutine`, whose value has the coroutine
    /// type `ty`: those of passing its arguments to the function it suspends,
    /// whose signature the arguments of `ty` instantiate.
    ///
    /// Nothing of the function runs yet, so no value is returned and no
    /// effect is performed here.
    pub(super) async fn collect_coroutine(
        &mut self,
        point: Point,
        coroutine: &Coroutine,
        ty: &Interned<Ty>,
    ) {
        let view = ty.as_coroutine_view().expect("a `Coroutine` expression has a coroutine type");
        let function_id = view.signature_id(self.solver.engine()).await;
        let substitution = view.create_subst(self.solver.engine()).await;

        self.collect_arguments(point, function_id, &substitution, coroutine.arguments()).await;

        // The function assumes its where clause and the trait references of
        // its instance parameters once it is resumed, so they must hold for
        // the coroutine to exist.
        self.collect_where_clause(point, function_id, &substitution).await;
        self.collect_instance_arguments(point, function_id, &substitution).await;

        // As for a call, the dictionary that a call through an instance
        // dispatches through must hold what it was built from.
        if let CoroutineView::TraitDef(view) = view {
            self.collect_dictionary_requirements(point, view.instance()).await;
        }
    }

    /// Collects the constraints of running, at `point`, something whose effect
    /// is `introduced`: `introduced <: effect of the function`, which ties the
    /// regions of its arguments to whoever handles that effect.
    pub(super) async fn collect_introduced_effect(
        &mut self,
        point: Point,
        introduced: &Interned<Ty>,
    ) {
        // The labels only differ in lifetimes, so this only fails on an error
        // that was reported already.
        let Some(outlives) =
            self.solver.relate_introduced_effect_without_unify(introduced, self.effect).await
        else {
            return;
        };

        for constraint in outlives.iter() {
            self.constraints.add(point, constraint);
        }
    }

    /// Collects the constraints of passing `arguments` to the parameters of
    /// `signature_id`, instantiated with `substitution`: `typeof(argument) <:
    /// parameter type` for each of them.
    async fn collect_arguments(
        &mut self,
        point: Point,
        signature_id: GlobalSymbolID,
        substitution: &Subst,
        arguments: &[IRExprID],
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
    }

    /// Collects the constraints of invoking the signature of `signature_id`,
    /// instantiated with `substitution`: `typeof(argument) <: parameter type`
    /// for each of `arguments`, and `return type <: ty`.
    pub(super) async fn collect_invocation(
        &mut self,
        point: Point,
        signature_id: GlobalSymbolID,
        substitution: &Subst,
        arguments: &[IRExprID],
        ty: &Interned<Ty>,
    ) {
        self.collect_arguments(point, signature_id, substitution, arguments).await;

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
