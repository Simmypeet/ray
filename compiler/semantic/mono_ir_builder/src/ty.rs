use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{
    MonoEffectInstance,
    ty::{
        AggregateKind, AggregateType, EffectOperation, FunctionSignature, HandlerLayout, MonoType,
        PointerMutability, PointerType, ReturnType,
    },
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{GlobalSymbolID, member::get_members};
use rayc_type::{
    poly_var::build_subst_from_args,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::{Mutability, Primitive, Ty, application::View as ApplicationView},
};

pub(crate) struct TypeLowerer {
    engine: TrackedEngine,
    pending_handlers: VecDeque<MonoEffectInstance>,
    queued_handlers: FxHashSet<MonoEffectInstance>,
    handler_layouts: FxHashMap<MonoEffectInstance, HandlerLayout>,
}

impl TypeLowerer {
    pub(crate) fn new(engine: TrackedEngine) -> Self {
        Self {
            engine,
            pending_handlers: VecDeque::new(),
            queued_handlers: FxHashSet::default(),
            handler_layouts: FxHashMap::default(),
        }
    }

    pub(crate) fn intern(&self, ty: MonoType) -> Interned<MonoType> { self.engine.intern(ty) }

    pub(crate) fn intern_types(
        &self,
        types: impl IntoIterator<Item = Interned<MonoType>>,
    ) -> Interned<[Interned<MonoType>]> {
        self.engine.intern_unsized(types.into_iter().collect::<Vec<_>>())
    }

    pub(crate) fn opaque_pointer(&self) -> Interned<MonoType> {
        self.intern(MonoType::OpaquePointer(PointerMutability::Const))
    }

    pub(crate) fn pointer(
        &self,
        pointee: Interned<MonoType>,
        mutability: PointerMutability,
    ) -> Interned<MonoType> {
        self.intern(MonoType::Pointer(PointerType::new(pointee, mutability)))
    }

    pub(crate) fn signature(
        &self,
        parameter_types: impl IntoIterator<Item = Interned<MonoType>>,
        return_type: Interned<MonoType>,
    ) -> FunctionSignature {
        FunctionSignature::new(
            self.intern_types(parameter_types),
            ReturnType::Value(self.intern_types([return_type])),
        )
    }

    /// Lowers the given type of kind star to a `MonoType`.
    pub(crate) async fn lower_type(
        &mut self,
        ty: &Interned<Ty>,
        substitution: &Subst,
    ) -> Interned<MonoType> {
        let ty = ty.apply_subst_or_clone(substitution, &self.engine);
        self.lower_concrete_type(&ty).await
    }

    async fn lower_concrete_type(&mut self, ty: &Interned<Ty>) -> Interned<MonoType> {
        match &**ty {
            Ty::Application(application) => match application.view() {
                ApplicationView::Primitive(primitive) => {
                    let ty = match primitive {
                        Primitive::Int32 => MonoType::Int32,
                        Primitive::Float32 => MonoType::Float32,
                        Primitive::Bool => MonoType::Bool,
                        Primitive::CInt => MonoType::CInt,
                        Primitive::CStr => MonoType::CStr,
                    };
                    self.intern(ty)
                }
                ApplicationView::Tuple(tuple) => {
                    if tuple.args().is_empty() {
                        return self.intern(MonoType::Unit);
                    }
                    let mut fields = Vec::with_capacity(tuple.args().len());

                    Box::pin(async {
                        for ty in tuple.args() {
                            fields.push((*self.lower_concrete_type(ty).await).clone());
                        }
                    })
                    .await;

                    self.intern(MonoType::Aggregate(AggregateType::new(
                        AggregateKind::Tuple,
                        fields,
                    )))
                }
                ApplicationView::Lambda(lambda) => {
                    Box::pin(async {
                        let mut parameters = vec![self.opaque_pointer()];

                        for parameter in lambda.parameter_types() {
                            parameters.push(self.lower_concrete_type(parameter).await);
                        }
                        for effect in self.lower_concrete_effects(lambda.effect_row()).await {
                            parameters.push(self.handler_pointer(effect));
                        }

                        let return_type = self.lower_concrete_type(lambda.return_type()).await;
                        let signature = self.signature(parameters, return_type);
                        self.intern(MonoType::Aggregate(AggregateType::new(
                            AggregateKind::Closure,
                            vec![
                                MonoType::FunctionPointer(signature),
                                MonoType::OpaquePointer(PointerMutability::Const),
                            ],
                        )))
                    })
                    .await
                }
                ApplicationView::Pointer(pointer) => {
                    let pointee_type = Box::pin(self.lower_concrete_type(pointer.pointee())).await;
                    let mutability = lower_mutability(pointer.mutability());
                    self.pointer(pointee_type, mutability)
                }
                ApplicationView::Error => {
                    panic!("compiler-internal invariant violation: error type reached MonoIR")
                }
            },
            Ty::Inference(_) | Ty::PolyVar(_) => {
                panic!(
                    "compiler-internal invariant violation: non-concrete type reached MonoIR: \
                     {ty:?}"
                )
            }
            Ty::EffectRow(_) => {
                panic!("compiler-internal invariant violation: effect row used as a value type")
            }
        }
    }

    pub(crate) async fn lower_effects(
        &mut self,
        effect: &Interned<Ty>,
        substitution: &Subst,
    ) -> Vec<MonoEffectInstance> {
        let effect =
            reduce_fully(effect.apply_subst_or_clone(substitution, &self.engine), &self.engine);
        self.lower_concrete_effects(&effect).await
    }

    /// Lowers the given concrete effect row to a list of `MonoEffectInstance`s.
    async fn lower_concrete_effects(&mut self, effect: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        let Ty::EffectRow(row) = &**effect else {
            panic!("compiler-internal invariant violation: function effect is not an effect row")
        };
        assert!(
            row.tail().is_none(),
            "compiler-internal invariant violation: open effect row reached MonoIR"
        );

        let mut effects = Vec::with_capacity(row.labels().len());
        for label in row.labels() {
            let subst = self
                .engine
                .build_subst_from_args(label.effect_symbol_id(), label.arguments())
                .await;

            let instance = MonoEffectInstance::new(label.effect_symbol_id(), subst);

            self.queue_handler(instance.clone());
            effects.push(instance);
        }
        effects.sort();
        effects.dedup();
        effects
    }

    pub(crate) fn effect_instance(
        &mut self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
        owner_substitution: &Subst,
    ) -> MonoEffectInstance {
        let mut substitution = substitution.clone();
        rayc_type::subst::MutSubstitutable::apply_mut_subst(
            &mut substitution,
            owner_substitution,
            &self.engine,
        );
        let instance = MonoEffectInstance::new(effect_id, substitution);
        self.queue_handler(instance.clone());
        instance
    }

    pub(crate) fn handler_pointer(&mut self, instance: MonoEffectInstance) -> Interned<MonoType> {
        self.queue_handler(instance.clone());
        let handler = self.intern(MonoType::EffectHandler(instance));
        self.pointer(handler, PointerMutability::Const)
    }

    fn queue_handler(&mut self, instance: MonoEffectInstance) {
        if self.queued_handlers.insert(instance.clone()) {
            self.pending_handlers.push_back(instance);
        }
    }

    async fn build_handler_layout(&mut self, instance: &MonoEffectInstance) -> HandlerLayout {
        let members = self.engine.get_members(instance.effect_id()).await;
        let mut operations = members
            .namable_members()
            .map(|operation| instance.effect_id().target_id.make_global(operation))
            .collect::<Vec<_>>();
        operations.sort_unstable();

        let mut lowered_operations = Vec::with_capacity(operations.len());
        for operation_id in operations {
            let parameters = self.engine.get_parameter_map(operation_id).await;
            let mut parameter_types = vec![self.opaque_pointer()];
            for (_, parameter) in parameters.iter() {
                parameter_types
                    .push(self.lower_type(parameter.ty(), instance.substitution()).await);
            }
            let return_type = self.engine.get_return_type(operation_id).await;
            let return_type = self.lower_type(&return_type, instance.substitution()).await;
            lowered_operations.push(EffectOperation::new(
                operation_id,
                self.signature(parameter_types, return_type),
            ));
        }
        HandlerLayout::new(instance.clone(), lowered_operations)
    }

    pub(crate) async fn ensure_handler_layout(
        &mut self,
        instance: &MonoEffectInstance,
    ) -> &HandlerLayout {
        if !self.handler_layouts.contains_key(instance) {
            let layout = self.build_handler_layout(instance).await;
            self.handler_layouts.insert(instance.clone(), layout);
        }
        self.handler_layouts.get(instance).expect("effect handler layout should have been inserted")
    }

    pub(crate) async fn finish_handler_layouts(&mut self) {
        while let Some(instance) = self.pending_handlers.pop_front() {
            if self.handler_layouts.contains_key(&instance) {
                continue;
            }
            let layout = self.build_handler_layout(&instance).await;
            self.handler_layouts.insert(instance, layout);
        }
    }

    pub(crate) fn take_handler_layouts(&mut self) -> Vec<HandlerLayout> {
        let layouts = std::mem::take(&mut self.handler_layouts);
        let mut layouts = layouts.into_values().collect::<Vec<_>>();
        layouts.sort_unstable();
        layouts
    }
}

pub(crate) const fn lower_mutability(mutability: Mutability) -> PointerMutability {
    match mutability {
        Mutability::Immutable => PointerMutability::Const,
        Mutability::Mutable => PointerMutability::Mut,
    }
}

fn reduce_fully(mut ty: Interned<Ty>, engine: &TrackedEngine) -> Interned<Ty> {
    while let Some(reduced) = ty.reduce(engine) {
        ty = reduced;
    }
    ty
}
