//! The constraints of resuming a coroutine: the continuation and the value
//! it is given, what it returns and performs, and the values a clause
//! receives when the coroutine suspends.

use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::Point,
    ir_expr::handle::{Handle, Yielded, YieldedValue},
};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{
    signature::get_return_type,
    subst::Substitutable,
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    variance::Variance,
};

use super::ConstraintCollector;

impl ConstraintCollector<'_> {
    /// Collects the constraints of `handle`, whose value, the one the
    /// coroutine completes with, has type `ty`.
    ///
    /// For a continuation of type `Continuation[r, c]`:
    ///
    /// - `typeof(value) <: r`;
    /// - the dictionary of the `handle` implements `Coroutine[c]`;
    /// - `Coroutine[c].Return <: ty`;
    /// - `Coroutine[c].Effect = {handled labels | forwarded effect}`, with the
    ///   arguments of the labels related invariantly;
    /// - the forwarded effect is performed in the effect of this function.
    pub(super) async fn collect_handle(
        &mut self,
        point: Point,
        handle: &Handle,
        ty: &Interned<Ty>,
    ) {
        let (resume_ty, coroutine_ty) = self.continuation_arguments(handle);

        // The coroutine continues with the value it is resumed with.
        let value_ty = self.function.get_expression(handle.value()).ty();
        self.relate(point, value_ty, &resume_ty, Variance::Covariant).await;

        // The dictionary tells what the coroutine returns and performs, so
        // its regions are tied to those of the coroutine that is resumed.
        let engine = self.solver.engine().clone();
        let instance = handle.coroutine_instance();
        let expected = TraitRef::new(
            engine.get_core_item(CoreItem::CoroutineTrait).await,
            Args::new([coroutine_ty], &engine),
        );
        self.collect_instance(point, instance, &expected).await;

        // The value the coroutine completes with becomes the value of the
        // expression.
        let return_ty = Ty::new_instance_associated(
            engine.get_core_item(CoreItem::CoroutineReturn).await,
            instance.clone(),
            [],
            &engine,
        );
        self.relate(point, &return_ty, ty, Variance::Covariant).await;

        // Every effect of the coroutine is either handled by a clause or
        // forwarded.
        let mut labels = Vec::with_capacity(handle.handled_effects().len());
        for (effect_id, handled) in handle.handled_effects() {
            labels.push(handled.label(effect_id, &engine).await);

            // Handling an effect requires its where clause, as performing
            // it does.
            self.collect_where_clause(point, effect_id, handled.substitution()).await;
            self.collect_instance_arguments(point, effect_id, handled.substitution()).await;
        }
        let coroutine_effect = Ty::new_instance_associated(
            engine.get_core_item(CoreItem::CoroutineEffect).await,
            instance.clone(),
            [],
            &engine,
        );
        let split_effect =
            Ty::new_effect_row(labels, Some(handle.forwarded_effect().clone()), &engine);
        self.relate(point, &coroutine_effect, &split_effect, Variance::Invariant).await;

        // A forwarded operation is performed in the effect of this function.
        self.collect_introduced_effect(point, handle.forwarded_effect()).await;
    }

    /// Collects the constraints of `yielded`, a value of type `ty` that a
    /// clause receives from the coroutine: `argument type <: ty` for an
    /// argument of the operation, and `Continuation[result type, c] <: ty`
    /// for the continuation, where `c` is the coroutine that was resumed.
    ///
    /// `point` is the terminator that resumes the coroutine, where the
    /// continuation it consumes is still live. Its regions flow from there
    /// into the value, which is live along the edge into the clause.
    ///
    /// The signature of the operation is instantiated with the arguments of
    /// the handled label.
    pub(super) async fn collect_yielded(
        &mut self,
        point: Point,
        yielded: &Yielded,
        ty: &Interned<Ty>,
    ) {
        let handle = self
            .function
            .get_expression(yielded.handle())
            .kind()
            .as_handle()
            .expect("a `Yielded` expression refers to its `Handle` expression");
        let engine = self.solver.engine().clone();
        let substitution = handle.handled_effect(yielded.effect_id(&engine).await).substitution();

        let given_ty = match yielded.value() {
            YieldedValue::Argument(index) => {
                let parameters = engine.get_parameter_map(yielded.operation_id()).await;
                let (_, parameter) = parameters
                    .iter()
                    .nth(index)
                    .expect("a yielded argument has a parameter of its operation");

                parameter.ty().apply_subst_or_clone(substitution, &engine)
            }

            // The rest of the coroutine waits for the result of the
            // operation. It is the same coroutine, so the continuation
            // holds every region the resumed one held.
            YieldedValue::Continuation => {
                let (_, coroutine_ty) = self.continuation_arguments(handle);
                let result_ty = engine
                    .get_return_type(yielded.operation_id())
                    .await
                    .apply_subst_or_clone(substitution, &engine);

                Ty::new_struct(
                    engine.get_core_item(CoreItem::ContinuationStruct).await,
                    Args::new([result_ty, coroutine_ty], &engine),
                    &engine,
                )
            }
        };

        self.relate(point, &given_ty, ty, Variance::Covariant).await;
    }

    /// Returns `r` and `c` of the `Continuation[r, c]` that `handle`
    /// consumes.
    ///
    /// # Panics
    ///
    /// Panics if the continuation does not have such a type. Only IR without
    /// errors is borrow checked, and type checking gives the continuation of
    /// a `handle` that type.
    fn continuation_arguments(&self, handle: &Handle) -> (Interned<Ty>, Interned<Ty>) {
        let continuation_ty = self.function.get_expression(handle.continuation()).ty();
        let Some([resume_ty, coroutine_ty]) =
            continuation_ty.as_struct_view().map(|continuation| continuation.args())
        else {
            panic!("the continuation of a `handle` should be a `Continuation[r, c]`");
        };

        (resume_ty.clone(), coroutine_ty.clone())
    }
}
