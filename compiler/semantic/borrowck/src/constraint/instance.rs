//! The constraints of using a dictionary: the trait reference it must
//! implement, what it was built from, and the drops it performs.

use qbice::storage::intern::Interned;
use rayc_ir::cfg::Point;
use rayc_semantic_element::drop_plan::{DropPlan, get_drop_plan};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    poly_var::{build_subst_from_args, get_enclosing_poly_var_maps},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView, args::Args},
    variance::Variance,
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Collects the constraints of dropping a value of type `value_ty` with
    /// the `Drop` dictionary `drop_instance`: `value_ty <: implementor`,
    /// where the dictionary implements `Drop[implementor]`.
    ///
    /// The value is moved into `Drop.drop`, so the loans it holds flow into
    /// the regions of the dictionary.
    pub(super) async fn collect_drop(
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

        // The value is passed to `Drop.drop` as any argument is to its
        // parameter, so its type only has to be a subtype of the implementor.
        // `collect_drop_dictionary` would require `Drop[value_ty]` exactly:
        // trait arguments are invariant, which also makes what the where
        // clause of the instance lets flow between the regions of the
        // dictionary flow back into the regions of the place.
        self.relate(point, value_ty, implementor, Variance::Covariant).await;

        self.collect_dictionary_requirements(point, drop_instance).await;
    }

    /// Requires, at `point`, each dictionary that `substitution` passes to
    /// an instance parameter of `symbol_id`, or of a declaration enclosing
    /// it, to implement the trait reference the parameter declares,
    /// instantiated with `substitution`.
    pub(super) async fn collect_instance_arguments(
        &mut self,
        point: Point,
        symbol_id: GlobalSymbolID,
        substitution: &Subst,
    ) {
        let poly_vars = self.solver.engine().get_enclosing_poly_var_maps(symbol_id).await;

        for (parameter_id, trait_ref) in poly_vars.instances() {
            // A parameter the substitution leaves out is not instantiated
            // here, as with the parameters of an enclosing declaration that
            // the current function shares.
            let Some(instance) = substitution.get(&parameter_id) else {
                continue;
            };

            let expected = trait_ref.apply_subst_or_clone(substitution, self.solver.engine());
            self.collect_instance(point, instance, &expected).await;
        }
    }

    /// Requires, at `point`, the dictionary `instance` to implement
    /// `expected`, and everything the dictionary was built from to hold.
    ///
    /// Type checking chose the dictionary with lifetimes erased, and
    /// renumbering then gave its lifetimes their own regions. Relating the
    /// trait reference it implements to `expected` is what ties those regions
    /// to the regions of the type arguments it was chosen for: without it, a
    /// loan would not flow through a type projected from the dictionary.
    pub(super) async fn collect_instance(
        &mut self,
        point: Point,
        instance: &Interned<Ty>,
        expected: &TraitRef,
    ) {
        // A dictionary without a trait reference is recovery from an invalid
        // declaration, which was reported already.
        let Ok(actual) = instance.instance_trait_ref(self.solver.engine()).await else {
            return;
        };

        // Type checking already made the two trait references equal modulo
        // lifetimes, so the relation only fails on an error, which was
        // reported already.
        if let Some(outlives) = self.solver.relate_trait_refs_without_unify(&actual, expected).await
        {
            for constraint in outlives.iter() {
                self.constraints.add(point, constraint);
            }
        }

        // A dictionary may be built from other dictionaries, to any depth.
        Box::pin(self.collect_dictionary_requirements(point, instance)).await;
    }

    /// Requires, at `point`, what the dictionary `instance` was built from:
    /// the where clause of its instance declaration, and each dictionary
    /// passed to it to implement the trait reference it stands for.
    #[allow(clippy::match_same_arms)]
    pub(super) async fn collect_dictionary_requirements(
        &mut self,
        point: Point,
        instance: &Interned<Ty>,
    ) {
        match &**instance {
            Ty::Application(application) => match application.view() {
                // A declared instance assumes its where clause and the trait
                // references of its own instance parameters.
                ApplicationView::Instance(view) => {
                    let substitution = self
                        .solver
                        .engine()
                        .build_subst_from_args(view.symbol_id(), view.args())
                        .await;
                    self.collect_where_clause(point, view.symbol_id(), &substitution).await;
                    self.collect_instance_arguments(point, view.symbol_id(), &substitution).await;
                }

                // One `Drop` dictionary per element of the tuple.
                ApplicationView::TupleDropInstance(view) => {
                    let elements = view
                        .tuple()
                        .as_tuple_view()
                        .expect("a tuple `Drop` instance is built for a tuple type");

                    self.collect_drop_dictionaries(
                        point,
                        elements.args(),
                        view.element_instances(),
                    )
                    .await;
                }

                // One `Drop` dictionary per capture of the closure.
                ApplicationView::ClosureDropInstance(view) => {
                    let captured = view
                        .closure()
                        .as_closure_view()
                        .and_then(|closure| closure.captured_tuple().as_tuple_view())
                        .expect("a closure `Drop` instance is built for a closure type");

                    self.collect_drop_dictionaries(
                        point,
                        captured.args(),
                        view.capture_instances(),
                    )
                    .await;
                }

                // One `Drop` dictionary per requirement of the generated
                // plan of the struct, instantiated with the arguments of the
                // struct type.
                ApplicationView::NominalDropInstance(view) => {
                    let nominal = view
                        .nominal()
                        .as_struct_view()
                        .expect("a nominal `Drop` instance is built for a struct type");
                    let plan = self.solver.engine().get_drop_plan(nominal.symbol_id()).await;

                    if let DropPlan::Generated(plan) = &*plan {
                        let substitution = nominal.create_subst(self.solver.engine()).await;
                        let engine = self.solver.engine().clone();

                        let droppeds = plan.requirements().iter().map(|requirement| {
                            requirement.apply_subst_or_clone(&substitution, &engine)
                        });

                        for (dropped, dictionary) in droppeds.zip(view.external_instances()) {
                            self.collect_drop_dictionary(point, &dropped, dictionary).await;
                        }
                    }
                }

                // These dictionaries are built from a type alone, not from
                // other dictionaries.
                ApplicationView::DefInstance(_) | ApplicationView::NoOpDropInstance(_) => {}

                // Not dictionaries.
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::Error => {}
            },

            // A dictionary the current function is given: whoever passed it
            // has proven what it requires.
            Ty::PolyVar(_) | Ty::SelfInstance(_) => {}

            // Not dictionaries.
            Ty::Inference(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => {}
        }
    }

    /// Requires, at `point`, each of `dictionaries` to implement `Drop` for
    /// the type of `droppeds` at the same position.
    pub(super) async fn collect_drop_dictionaries(
        &mut self,
        point: Point,
        droppeds: &[Interned<Ty>],
        dictionaries: &[Interned<Ty>],
    ) {
        for (dropped, dictionary) in droppeds.iter().zip(dictionaries) {
            self.collect_drop_dictionary(point, dropped, dictionary).await;
        }
    }

    /// Requires, at `point`, `dictionary` to implement `Drop[dropped]`.
    pub(super) async fn collect_drop_dictionary(
        &mut self,
        point: Point,
        dropped: &Interned<Ty>,
        dictionary: &Interned<Ty>,
    ) {
        let expected = TraitRef::new(
            self.solver.engine().get_core_item(CoreItem::DropTrait).await,
            Args::new([dropped.clone()], self.solver.engine()),
        );

        self.collect_instance(point, dictionary, &expected).await;
    }
}
