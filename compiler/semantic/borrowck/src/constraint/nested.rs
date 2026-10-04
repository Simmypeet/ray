//! The constraints of creating a nested function, a closure or a `handle`: its
//! [requirements](crate::requirement), with each external region instantiated
//! by the region of the creator it stands for.
//!
//! The instantiation comes from matching each type in the interface of the
//! nested function against the type its creator gives it, as an instance head
//! is matched against a trait reference. The match binds the external regions,
//! and requires the lifetimes of the two types to be equal anywhere else.

use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::Point,
    ir_expr::{
        IRExprID,
        closure::Closure,
        handle::{Handle, OperationHandler},
    },
    ir_function::FunctionID,
    ir_lambda::CaptureMapID,
};
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::{Ty, application::ClosureView},
    variance::Variance,
};

use super::ConstraintCollector;

/// The types in the interfaces of the nested functions created at one point,
/// each paired with the type their creator gives it.
struct Interface {
    pairs: Vec<(Interned<Ty>, Interned<Ty>)>,
}

impl Interface {
    const fn new() -> Self { Self { pairs: Vec::new() } }

    /// Pairs `interface_ty`, a type in the interface of a nested function,
    /// with `created_ty`, the type its creator gives it.
    fn pair(&mut self, interface_ty: &Interned<Ty>, created_ty: &Interned<Ty>) {
        self.pairs.push((interface_ty.clone(), created_ty.clone()));
    }
}

impl ConstraintCollector<'_> {
    /// Collects the constraints of `closure`, whose value has type `ty`:
    /// `typeof(capture operand) <: capture type`, and what its body requires.
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

        // The closure type is the interface of the body, instantiated.
        let interface = self.closure_interface(closure.function_id(), &closure_ty);
        let instantiation = self.instantiate(point, interface).await;

        self.collect_nested_requirements(point, closure.function_id(), &instantiation).await;
    }

    /// Pairs the interface of the closure body `function_id` with
    /// `closure_ty`.
    fn closure_interface(
        &self,
        function_id: FunctionID,
        closure_ty: &ClosureView<'_>,
    ) -> Interface {
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
        let pairs = capture_types
            .zip(capture_slots.args())
            .chain(parameter_types.zip(closure_ty.params()))
            .chain(signature);

        let mut interface = Interface::new();
        for (interface_ty, created_ty) in pairs {
            interface.pair(&interface_ty, created_ty);
        }

        interface
    }

    /// Collects the constraints of `handle`, whose value has type `ty`: the
    /// where clause of the effect, the residual effect, the drops of the
    /// handler captures, and what the body and the handlers require.
    pub(super) async fn collect_handle(
        &mut self,
        point: Point,
        handle: &Handle,
        ty: &Interned<Ty>,
    ) {
        // Handling an effect requires its where clause, as performing it does.
        self.collect_where_clause(point, handle.effect_id(), handle.substitution()).await;
        self.collect_instance_arguments(point, handle.effect_id(), handle.substitution()).await;

        // The residual effect is performed in the effect of this function.
        self.collect_introduced_effect(point, handle.residual_effect()).await;

        // The handlers only borrow their captures, so this function drops
        // them once the handled body returns.
        for (operand, drop_instance) in
            handle.handler_captures().iter().zip(handle.handler_capture_drops())
        {
            let operand_ty = self.function.get_expression(*operand).ty();
            self.collect_drop(point, operand_ty, drop_instance).await;
        }

        // The body and the handlers are created with the types of the
        // `handle`.
        let interface = self.handle_interface(handle, ty).await;
        let instantiation = self.instantiate(point, interface).await;

        let created = std::iter::once(handle.body().function_id())
            .chain(handle.handlers().iter().map(OperationHandler::function_id));
        for function_id in created {
            self.collect_nested_requirements(point, function_id, &instantiation).await;
        }
    }

    /// Pairs the interfaces of the body and the handlers of `handle`, whose
    /// value has type `ty`, with its types.
    async fn handle_interface(&self, handle: &Handle, ty: &Interned<Ty>) -> Interface {
        let engine = self.solver.engine();
        let mut interface = Interface::new();

        // The body returns the value of the `handle`, and runs in the
        // handled effect on top of the residual effect.
        let body = handle.body();
        let body_context =
            self.ir.get_function(body.function_id()).context().assert_as_thunk_context();
        let body_effect = handle.body_effect_row(engine).await;

        self.pair_captures(
            &mut interface,
            self.ir.capture_map_id(body.function_id()),
            body.captures(),
        );
        interface.pair(body_context.return_ty(), ty);
        interface.pair(body_context.effect(), &body_effect);

        // Each handler has the signature of its operation, instantiated by the
        // `handle`, and runs in the residual effect.
        for handler in handle.handlers() {
            let context = self
                .ir
                .get_function(handler.function_id())
                .context()
                .assert_as_operation_handler_context();

            let declared = engine.get_parameter_map(handler.operation_id()).await;
            for ((_, parameter), (_, declared)) in context.parameters().zip(declared.iter()) {
                let declared_ty = declared.ty().apply_subst_or_clone(handle.substitution(), engine);
                interface.pair(parameter.ty(), &declared_ty);
            }

            let return_ty = engine
                .get_return_type(handler.operation_id())
                .await
                .apply_subst_or_clone(handle.substitution(), engine);
            interface.pair(context.return_ty(), &return_ty);
            interface.pair(context.effect(), handle.residual_effect());
        }

        // The handlers share one capture layout, and so its external
        // regions.
        let handlers_shared_capture_map_id =
            self.ir.capture_map_id(handle.handlers()[0].function_id());
        self.pair_captures(
            &mut interface,
            handlers_shared_capture_map_id,
            handle.handler_captures(),
        );

        interface
    }

    /// Pairs the capture layout of `function_id` with the types of `operands`.
    fn pair_captures(
        &self,
        interface: &mut Interface,
        capture_map_id: CaptureMapID,
        operands: &[IRExprID],
    ) {
        let engine = self.solver.engine();
        let captures = self.ir.get_capture_map(capture_map_id);

        for ((_, capture), operand) in captures.iter().zip(operands) {
            let operand_ty = self.function.get_expression(*operand).ty();
            interface.pair(&capture.storage_ty(engine), operand_ty);
        }
    }

    /// Matches `interface` against the types of the creator, and returns the
    /// region of the creator that each external region stands for.
    ///
    /// Also adds, at `point`, the outlives constraints of the match: the two
    /// types of a pair must have equal lifetimes wherever the interface has a
    /// lifetime that is not external.
    async fn instantiate(&mut self, point: Point, interface: Interface) -> Subst {
        // The two types of a pair only differ in lifetimes, so this only fails
        // on an error that was reported already.
        let (instantiation, outlives) = self
            .solver
            .interface_match(interface.pairs)
            .await
            .expect("shape should match")
            .into_parts();

        for constraint in outlives.iter() {
            self.constraints.add(point, constraint);
        }

        instantiation
    }

    /// Requires, at `point`, what the nested function `function_id` requires of
    /// its creator.
    async fn collect_nested_requirements(
        &mut self,
        point: Point,
        function_id: FunctionID,
        instantiation: &Subst,
    ) {
        let nested = self.nested;

        for requirement in nested.of(function_id) {
            let predicate = requirement.instantiate(instantiation, self.solver.engine());

            // Blame the source in the nested function, not the expression
            // creating it.
            self.collect_outlives_blaming(point, &predicate, Some(requirement.span())).await;
        }
    }
}
