//! The constraints of reading, writing and borrowing places, and the types
//! of the places an address selects.

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::{Address, Local, Projection},
    cfg::{Point, Store},
    ir_expr::{IRExprID, load::Load, ref_of::RefOf},
};
use rayc_semantic_element::{
    drop_plan::get_drop_plan, parameter::get_parameter_map, struct_body::get_struct_body,
};
use rayc_type::{
    subst::Substitutable,
    ty::{Mutability, Ty},
    variance::Variance,
    where_clause::OutlivesPredicate,
};

use super::{ConstraintCollector, Loan};

impl ConstraintCollector<'_> {
    /// Collects the constraints of the borrow `ref_of`, the expression
    /// `expression_id` of type `ty`, and issues its loan.
    pub(super) async fn collect_borrow(
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
            .place_type_with_derefs(point, ref_of.address(), |pointer| {
                dereferenced.push(pointer.clone());
            })
            .await
        else {
            return;
        };

        // The lifetime of `ty` is the loan's own region, so only the pointees
        // are related.
        let variance = reference.mutability().pointee_variance();
        self.relate(point, &place_ty, reference.pointee(), variance).await;

        self.collect_reborrow(point, reference.lifetime(), &dereferenced);

        // Only a place the function owns, or reaches through mutable
        // references alone, can be accessed in a way the borrow forbids.
        let is_tracked = dereferenced.iter().all(|pointer| {
            pointer
                .as_reference_view()
                .is_some_and(|reference| reference.mutability() == Mutability::Mutable)
        });
        if !is_tracked {
            return;
        }

        self.collect_environment_reborrow(point, ref_of.address(), reference.lifetime());

        let declared_drop_depth = self.declared_drop_depth(ref_of.address()).await;
        self.constraints.issue_loan(expression_id, Loan {
            declared_drop_depth,
            region: reference.lifetime().clone(),
            point,
            address: ref_of.address().clone(),
            mutability: reference.mutability(),
            span: self.function.get_expression(expression_id).span(),
        });
    }

    /// Returns the projection depth of the innermost struct with a declared
    /// `Drop` instance that owns the place `address` selects, if there is one.
    async fn declared_drop_depth(&self, address: &Address) -> Option<usize> {
        let mut ty = self.binding_type(address.local()?).await;
        let mut innermost = None;

        for (depth, &projection) in address.projections().iter().enumerate() {
            if projection.is_deref() {
                break;
            }

            // The outlives constraints of normalizing these prefixes are
            // collected with the type of the borrowed place.
            let base = self.solver.normalize(&ty).await;
            if let Some(struct_ty) = base.as_struct_view()
                && self.solver.engine().get_drop_plan(struct_ty.symbol_id()).await.is_explicit()
            {
                innermost = Some(depth);
            }

            ty = self.projected_type(&base, projection).await;
        }

        innermost
    }

    /// Requires the references in `dereferenced`, the pointers a borrowed place
    /// is reached through, to outlive the loan `region`: innermost first, up
    /// to the first shared reference or raw pointer.
    pub(super) fn collect_reborrow(
        &mut self,
        point: Point,
        region: &Interned<Ty>,
        dereferenced: &[Interned<Ty>],
    ) {
        for pointer in dereferenced.iter().rev() {
            let Some(reference) = pointer.as_reference_view() else {
                break;
            };

            for predicate in
                OutlivesPredicate::from_relation(reference.lifetime(), region, Variance::Covariant)
            {
                self.constraints.add(point, &predicate);
            }

            match reference.mutability() {
                Mutability::Immutable => break,
                Mutability::Mutable => {}
            }
        }
    }

    /// Requires the environment lifetime `'env` of an operation handler to
    /// outlive the loan `region` when `address` is rooted at a capture, which
    /// the handler reaches through `&'env mut Env`, as an `FnMut` closure does.
    fn collect_environment_reborrow(
        &mut self,
        point: Point,
        address: &Address,
        region: &Interned<Ty>,
    ) {
        let Some(environment) = self.function.environment_lifetime() else {
            return;
        };

        if address.local().is_some_and(Local::is_capture) {
            self.constraints
                .add(point, &OutlivesPredicate::new(environment.clone(), region.clone()));
        }
    }

    /// Collects the constraints of `load`, whose loaded value has type `ty`:
    /// `typeof(place) <: ty`.
    pub(super) async fn collect_load(&mut self, point: Point, load: &Load, ty: &Interned<Ty>) {
        let Some(place_ty) = self.place_type(point, load.address()).await else {
            return;
        };

        self.relate(point, &place_ty, ty, Variance::Covariant).await;
    }

    /// Collects the constraints of `store`: `typeof(value) <: typeof(place)`.
    pub(super) async fn collect_store(&mut self, point: Point, store: &Store) {
        let Some(place_ty) = self.place_type(point, store.address()).await else {
            return;
        };

        let value_ty = self.function.get_expression(store.expression()).ty();
        self.relate(point, value_ty, &place_ty, Variance::Covariant).await;
    }

    /// Returns the type of the place `address` selects, or `None` for an error
    /// address.
    pub(super) async fn place_type(
        &mut self,
        point: Point,
        address: &Address,
    ) -> Option<Interned<Ty>> {
        self.place_type_with_derefs(point, address, |_| {}).await
    }

    /// Returns the type of the place `address` selects, or `None` for an error
    /// address, calling `on_deref` with the type of each pointer dereferenced.
    pub(super) async fn place_type_with_derefs(
        &mut self,
        point: Point,
        address: &Address,
        mut on_deref: impl FnMut(&Interned<Ty>),
    ) -> Option<Interned<Ty>> {
        let mut ty = self.binding_type(address.local()?).await;

        // Normalize each prefix before inspecting its shape, since a field
        // or pointee may be an associated type.
        for &projection in address.projections() {
            let (base, outlives) = self.solver.normalize_with_outlives(&ty).await;
            for constraint in outlives.iter() {
                self.constraints.add(point, constraint);
            }

            if projection.is_deref() {
                on_deref(&base);
            }
            ty = self.projected_type(&base, projection).await;
        }

        Some(ty)
    }

    /// Returns the type of the component of the normalized type `base` that
    /// `projection` selects.
    pub(super) async fn projected_type(
        &self,
        base: &Interned<Ty>,
        projection: Projection,
    ) -> Interned<Ty> {
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
    pub(super) async fn binding_type(&self, local: Local) -> Interned<Ty> {
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
